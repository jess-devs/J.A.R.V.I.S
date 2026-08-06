//! Primitivas de lanzamiento de procesos específicas de plataforma. Cada
//! función tiene una sola implementación activa según `cfg` (Windows vs.
//! Unix), así el resto del código (`tools/apps.rs`, `tools/files.rs`,
//! `tools/shell.rs`) no necesita `#[cfg]` disperso — solo llama a
//! `platform::open_target`/`run_shell_command` sin preguntarse el SO.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

use std::path::{Path, PathBuf};

/// Carpetas estándar del usuario: etiqueta hablada, clave XDG y nombre por
/// defecto. Windows y macOS mantienen los nombres en inglés en disco aunque
/// el sistema esté en español (la traducción es solo de presentación), así
/// que ahí alcanza el nombre por defecto; el único SO que las renombra de
/// verdad es Linux, y para eso está la clave XDG.
const STANDARD_FOLDERS: [(&str, &str, &str); 6] = [
    ("escritorio", "XDG_DESKTOP_DIR", "Desktop"),
    ("descargas", "XDG_DOWNLOAD_DIR", "Downloads"),
    ("documentos", "XDG_DOCUMENTS_DIR", "Documents"),
    ("música", "XDG_MUSIC_DIR", "Music"),
    ("imágenes", "XDG_PICTURES_DIR", "Pictures"),
    ("vídeos", "XDG_VIDEOS_DIR", "Videos"),
];

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Lee `~/.config/user-dirs.dirs`, donde los escritorios Linux guardan las
/// rutas reales (`XDG_DOWNLOAD_DIR="$HOME/Descargas"`). No se usan las
/// variables de entorno equivalentes porque el escritorio no las exporta a
/// procesos que no lanzó él.
#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_user_dirs(home: &Path) -> std::collections::HashMap<String, PathBuf> {
    let path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("user-dirs.dirs");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return std::collections::HashMap::new();
    };
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim().trim_matches('"');
            let resolved = match value.strip_prefix("$HOME/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(value),
            };
            Some((key.trim().to_string(), resolved))
        })
        .collect()
}

/// Carpetas estándar del usuario ya resueltas, como `(etiqueta, ruta)`.
///
/// Solo devuelve las que existen de verdad en el disco: una ruta inventada
/// metida en el prompt es peor que no decir nada, porque el modelo la usa
/// con confianza y `open_file` falla.
pub fn user_folders() -> Vec<(&'static str, PathBuf)> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let xdg = xdg_user_dirs(&home);

    let mut out = vec![("carpeta personal", home.clone())];
    for (label, _xdg_key, default_name) in STANDARD_FOLDERS {
        #[cfg(all(unix, not(target_os = "macos")))]
        let path = xdg
            .get(_xdg_key)
            .cloned()
            .unwrap_or_else(|| home.join(default_name));
        #[cfg(any(windows, target_os = "macos"))]
        let path = home.join(default_name);

        out.push((label, path));
    }

    // Un escritorio configurado como `$HOME/` (habitual en escritorios sin
    // carpeta Escritorio) resolvería al home y le diría al modelo que
    // "abrí el escritorio" es abrir la carpeta personal. Se descartan las
    // que no existen, las que colapsan sobre el home y las repetidas: en
    // los tres casos la etiqueta promete algo que no es.
    let mut vistas = std::collections::HashSet::new();
    out.retain(|(label, path)| {
        path.is_dir()
            && (*label == "carpeta personal" || path != &home)
            && vistas.insert(path.clone())
    });
    out
}

/// Bloque de contexto con las rutas de arriba. Sin esto el modelo no puede
/// saber si la carpeta de descargas es `Downloads` o `Descargas`, y termina
/// sondeando el disco con la tool de shell en vez de usar `open_file`.
pub fn folders_context() -> String {
    let folders = user_folders();
    if folders.is_empty() {
        return String::new();
    }
    let listado = folders
        .iter()
        .map(|(label, path)| format!("{label}: {}", path.display()))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "Carpetas de este equipo, con su ruta real ({listado}). Usá open_file con la ruta \
         completa para abrir cualquiera de ellas; no hace falta buscarlas ni adivinar el nombre."
    )
}

/// Nombre del SO y de la tool de shell que existe en él. El registro de tools
/// ya es `cfg`-dependiente (`run_powershell` en Windows, `run_shell` en
/// Unix), pero el modelo no tiene forma de saber en cuál está corriendo: sin
/// decírselo propone comandos del SO equivocado.
pub fn os_and_shell_tool() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("Windows", "run_powershell")
    } else if cfg!(target_os = "macos") {
        ("macOS", "run_shell")
    } else {
        ("Linux", "run_shell")
    }
}

/// Bloque de contexto que se inyecta en el prompt de sistema. Es un hecho
/// fijo de la corrida, no algo que el modelo deba consultar ni responder, así
/// que va en el prompt y no en una tool.
pub fn os_context() -> String {
    let (os, shell_tool) = os_and_shell_tool();
    format!(
        "Estás corriendo en {os}. La única herramienta de shell que existe en este sistema es \
         {shell_tool}; cualquier otra (PowerShell en Linux, bash en Windows) no está disponible. \
         Nunca propongas ni ejecutes comandos de otro sistema operativo, y si no estás seguro de \
         que un comando exista acá, decilo en vez de intentarlo."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una ruta inventada en el prompt es peor que no decir nada: el modelo
    /// la usa con confianza y `open_file` falla con "la ruta no existe".
    #[test]
    fn solo_devuelve_carpetas_que_existen() {
        for (label, path) in user_folders() {
            assert!(
                path.is_dir(),
                "{label} apunta a algo que no existe: {path:?}"
            );
        }
    }

    /// Cada etiqueta tiene que apuntar a algo distinto. Un escritorio
    /// configurado como `$HOME/` colapsaba sobre la carpeta personal, y el
    /// modelo terminaba abriendo el home creyendo que abría el escritorio.
    #[test]
    fn ninguna_etiqueta_apunta_al_mismo_lugar_que_otra() {
        let folders = user_folders();
        let mut vistas = std::collections::HashSet::new();
        for (label, path) in &folders {
            assert!(
                vistas.insert(path.clone()),
                "{label} apunta a {path:?}, que ya tiene otra etiqueta"
            );
        }
    }

    /// La `carpeta personal` siempre debería estar: si falta, o no hay HOME
    /// o la resolución se rompió entera.
    #[test]
    fn incluye_la_carpeta_personal() {
        if home_dir().is_some() {
            let folders = user_folders();
            assert!(folders
                .iter()
                .any(|(label, _)| *label == "carpeta personal"));
        }
    }

    #[test]
    fn el_contexto_nombra_open_file_y_las_rutas() {
        let ctx = folders_context();
        if !ctx.is_empty() {
            assert!(ctx.contains("open_file"));
            for (label, _) in user_folders() {
                assert!(ctx.contains(label), "falta la etiqueta {label}");
            }
        }
    }
}
