use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::errors::ToolError;

/// Cuánto se espera a que el manejador del SO termine antes de darlo por
/// despachado. `xdg-open` no siempre vuelve enseguida: si el manejador que
/// elige no estaba corriendo, se queda vivo mientras dure la app que lanzó.
/// Esperar su salida colgaba la tool hasta `agent.tool_timeout_secs` y
/// devolvía un error de timeout con la carpeta ya abierta en pantalla.
const LAUNCH_GRACE: Duration = Duration::from_secs(2);

/// Una app lanzada hereda los descriptores de Jarvis y escupe su cháchara en
/// el log (nautilus avisando de Tracker y del display server, por ejemplo).
/// Eso no es diagnóstico de Jarvis y ensucia la salida donde se leen los
/// turnos de la conversación; el estado de la app se sabe por su código de
/// salida, no por lo que imprima.
fn silence(cmd: &mut Command) {
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
}

enum Launch {
    /// Salió bien, o sigue vivo sosteniendo lo que abrió.
    Dispatched,
    /// Terminó con error, o no se pudo ni lanzar.
    Failed,
}

async fn launch(mut command: Command) -> Launch {
    let Ok(mut child) = command.spawn() else {
        return Launch::Failed;
    };
    match tokio::time::timeout(LAUNCH_GRACE, child.wait()).await {
        Ok(Ok(status)) if status.success() => Launch::Dispatched,
        // Sigue corriendo pasado el margen: ya despachó y ahora sostiene la
        // app. Se lo deja suelto (tokio no lo mata al soltar el `Child`).
        Err(_) => Launch::Dispatched,
        _ => Launch::Failed,
    }
}

/// Lanza `target` (ruta, URL o nombre de comando) con el manejador por
/// defecto del SO: `open` en macOS, `xdg-open` en el resto de Unix. Si eso
/// falla (por ejemplo porque `target` es el nombre de una app sin asociación
/// de MIME/URL, como "spotify" dicho por voz en vez de una ruta o una URL),
/// cae a intentar lanzarlo directo como comando de PATH — cubre el caso
/// común de `open_app` sin escaneo de accesos directos (ese escaneo es
/// Windows-only por ahora, ver `tools/apps.rs`).
///
/// No espera a que el manejador termine: ver `LAUNCH_GRACE`.
pub async fn open_target(target: &str) -> Result<(), ToolError> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut cmd = Command::new(opener);
    cmd.arg(target);
    silence(&mut cmd);

    // Una ruta existente o una URL no pueden ser además un ejecutable del
    // PATH, así que no hay fallback que esperar: apenas se lanzó, se
    // despachó. Esto deja el caso común (`open_file`, `open_url`) sin
    // latencia agregada.
    if Path::new(target).exists() || target.contains("://") {
        return match cmd.spawn() {
            Ok(_) => Ok(()),
            Err(e) => Err(ToolError::Execution(format!(
                "no se pudo lanzar '{target}': {e}"
            ))),
        };
    }

    // Un nombre suelto ("spotify" dicho por voz) sí puede ser un comando del
    // PATH: acá vale esperar lo justo para saber si el manejador lo rechazó.
    if matches!(launch(cmd).await, Launch::Dispatched) {
        return Ok(());
    }
    let mut fallback = Command::new(target);
    silence(&mut fallback);
    match fallback.spawn() {
        Ok(_) => Ok(()),
        Err(e) => Err(ToolError::Execution(format!(
            "no se pudo lanzar '{target}': {e}"
        ))),
    }
}

/// Ejecuta un comando de shell (`sh -c`) y devuelve su salida formateada
/// (stdout + stderr, o un mensaje si no hubo salida). Equivalente Unix de
/// `run_powershell_command` (ver `tools/shell.rs`).
pub async fn run_shell_command(command: &str) -> Result<String, ToolError> {
    let output = Command::new("sh")
        .args(["-c", command])
        .output()
        .await
        .map_err(|e| ToolError::Execution(format!("no se pudo lanzar el shell: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut result = String::new();
    if !stdout.trim().is_empty() {
        result.push_str(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("(stderr) {}", stderr.trim()));
    }
    if result.is_empty() {
        result = if output.status.success() {
            "Comando ejecutado sin salida.".to_string()
        } else {
            format!(
                "El comando falló (código {:?}) sin salida.",
                output.status.code()
            )
        };
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// El bug real: `xdg-open` sigue vivo mientras dure el gestor de archivos
    /// que abrió, y esperarlo colgaba `open_file` hasta el timeout de la tool
    /// aunque la carpeta ya estuviera abierta.
    #[tokio::test]
    async fn un_lanzador_que_no_termina_se_da_por_despachado() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");

        let inicio = Instant::now();
        assert!(matches!(launch(cmd).await, Launch::Dispatched));
        assert!(
            inicio.elapsed() < LAUNCH_GRACE + Duration::from_secs(2),
            "esperó de más: {:?}",
            inicio.elapsed()
        );
    }

    /// Un manejador que rechaza el target tiene que reportarse como fallo,
    /// para que `open_target` pruebe lanzarlo como comando del PATH.
    #[tokio::test]
    async fn un_lanzador_que_falla_se_reporta_como_fallo() {
        assert!(matches!(
            launch(Command::new("false")).await,
            Launch::Failed
        ));
        assert!(matches!(
            launch(Command::new("comando-que-no-existe-jarvis")).await,
            Launch::Failed
        ));
    }
}
