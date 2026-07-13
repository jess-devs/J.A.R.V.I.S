//! Abrir aplicaciones, sitios web y cerrar aplicaciones. `open_app` primero
//! prueba los alias de `config.yaml`; si no hay alias, busca con matching
//! tolerante (substring + tokens + Levenshtein) entre los accesos directos
//! del Menú Inicio y el Escritorio, y si tampoco hay match confiado cae al
//! comportamiento clásico de pasarle el nombre literal a `cmd /C start`
//! (PATH + App Paths del registro). `open_url` lanza con `cmd /C start` el
//! navegador por defecto. `close_app` mata procesos por nombre y por eso
//! requiere confirmación.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::AppsConfig;
use crate::errors::ToolError;
use crate::wake::{levenshtein, normalize_phrase};

use super::{required_str, RiskLevel, Tool, ToolOutput};

/// Por debajo de esto un match no es lo bastante bueno para lanzarse solo.
const CONFIDENT_THRESHOLD: f32 = 0.6;
/// Margen mínimo sobre el segundo candidato para no considerarlo ambiguo.
const AMBIGUITY_MARGIN: f32 = 0.15;
/// Por debajo de esto ni siquiera vale la pena sugerirlo en el error.
const SUGGESTION_THRESHOLD: f32 = 0.3;

/// Un acceso directo indexado desde el Menú Inicio o el Escritorio.
struct AppEntry {
    /// Nombre del archivo sin extensión: para mensajes y TTS, nunca una ruta.
    display: String,
    /// Normalizado (minúsculas, sin tildes) para comparar.
    normalized: String,
    /// Tokens normalizados, para matching por palabras sueltas.
    tokens: Vec<String>,
    /// Ruta al .lnk/.url. Lanzarla resuelve su destino, igual que un doble
    /// click, sin necesidad de parsear el shortcut.
    path: PathBuf,
}

/// Carpetas donde Windows deja accesos directos de apps instaladas.
fn default_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        roots.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(program_data) = std::env::var("ProgramData") {
        roots.push(PathBuf::from(program_data).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(user_profile).join("Desktop"));
    }
    if let Ok(public) = std::env::var("PUBLIC") {
        roots.push(PathBuf::from(public).join("Desktop"));
    }
    roots
}

/// Recorre las carpetas de accesos directos. Sin caché ni presupuesto de
/// tiempo: son carpetas chicas (cientos de archivos, no todo el disco), así
/// que se escanean frescas en cada llamada — evita servir resultados
/// desactualizados si el usuario acaba de instalar algo.
fn scan_shortcuts(extra_roots: &[PathBuf]) -> Vec<AppEntry> {
    let mut roots = default_search_roots();
    roots.extend(extra_roots.iter().cloned());

    let mut entries = Vec::new();
    for root in roots {
        for entry in walkdir::WalkDir::new(&root).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if ext != "lnk" && ext != "url" {
                continue;
            }
            let Some(display) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let normalized = normalize_phrase(display);
            let tokens = normalized.split_whitespace().map(String::from).collect();
            entries.push(AppEntry {
                display: display.to_string(),
                normalized,
                tokens,
                path: path.to_path_buf(),
            });
        }
    }
    entries
}

/// Puntúa qué tan bien `entry` responde a lo que dijo el usuario: 1.0 exacto,
/// 0.6-0.95 si el nombre completo del acceso directo contiene la consulta,
/// hasta 0.55 por solape de tokens (tolerando errores de transcripción con
/// Levenshtein). 0.0 si no hay ninguna relación.
fn score(query_norm: &str, query_tokens: &[String], entry: &AppEntry) -> f32 {
    if entry.normalized == query_norm {
        return 1.0;
    }
    if query_norm.len() >= 3 && entry.normalized.contains(query_norm) {
        let coverage = (query_norm.len() as f32 / entry.normalized.len() as f32).min(1.0);
        return 0.6 + 0.35 * coverage;
    }
    if query_tokens.is_empty() {
        return 0.0;
    }
    let matched = query_tokens
        .iter()
        .filter(|qt| {
            entry
                .tokens
                .iter()
                .any(|et| et == *qt || levenshtein(qt, et) <= 1)
        })
        .count();
    0.55 * (matched as f32 / query_tokens.len() as f32)
}

/// Lanza `target` (ruta o nombre) con `start`, que no bloquea y resuelve
/// igual que lo haría el usuario en Win+R o con doble click.
async fn launch(target: &str, display: &str) -> Result<ToolOutput, ToolError> {
    let status = tokio::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(target)
        .status()
        .await
        .map_err(|e| ToolError::Execution(format!("no se pudo lanzar '{target}': {e}")))?;
    if status.success() {
        Ok(ToolOutput::text(format!("Aplicación '{display}' lanzada.")))
    } else {
        Err(ToolError::Execution(format!(
            "Windows no pudo lanzar '{display}'."
        )))
    }
}

pub struct OpenApp {
    aliases: HashMap<String, String>,
    extra_search_roots: Vec<PathBuf>,
}

impl OpenApp {
    pub fn new(cfg: &AppsConfig) -> Self {
        // Claves normalizadas a minúsculas para matchear lo transcrito.
        let aliases = cfg
            .aliases
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect();
        Self {
            aliases,
            extra_search_roots: cfg.extra_search_roots.clone(),
        }
    }

    fn resolve_alias(&self, name: &str) -> Option<String> {
        let key = name.trim().to_lowercase();
        self.aliases.get(&key).cloned()
    }
}

#[async_trait]
impl Tool for OpenApp {
    fn name(&self) -> &'static str {
        "open_app"
    }

    fn description(&self) -> &'static str {
        "Abre una aplicación por su nombre, p.ej. 'notepad', 'chrome', \
         'spotify', 'roblox', 'discord'. No hace falta el nombre exacto del \
         ejecutable: busca entre los accesos directos del Menú Inicio y el \
         Escritorio con coincidencia tolerante."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Nombre de la aplicación tal como lo dijo el usuario"
                }
            },
            "required": ["name"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let name = args.get("name").and_then(Value::as_str).unwrap_or("?");
        format!("abrir {name}")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let name = required_str(&args, "name")?;

        if let Some(alias_target) = self.resolve_alias(name) {
            return launch(&alias_target, &alias_target).await;
        }

        let query_norm = normalize_phrase(name);
        let query_tokens: Vec<String> = query_norm.split_whitespace().map(String::from).collect();
        let extra_roots = self.extra_search_roots.clone();
        let entries = tokio::task::spawn_blocking(move || scan_shortcuts(&extra_roots))
            .await
            .unwrap_or_default();

        let mut scored: Vec<(f32, &AppEntry)> = entries
            .iter()
            .map(|entry| (score(&query_norm, &query_tokens, entry), entry))
            .filter(|(s, _)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((best_score, best)) = scored.first() {
            let confident = *best_score >= CONFIDENT_THRESHOLD
                && scored
                    .get(1)
                    .is_none_or(|(second, _)| best_score - second >= AMBIGUITY_MARGIN);
            if confident {
                return launch(&best.path.display().to_string(), &best.display).await;
            }
        }

        // Sin match confiado: comportamiento clásico, pasar el nombre
        // literal a `start` (cubre comandos de Windows y apps con entrada en
        // App Paths que no tienen acceso directo indexado).
        let literal = name.trim().to_lowercase();
        match launch(&literal, &literal).await {
            ok @ Ok(_) => ok,
            Err(_) => {
                let suggestions: Vec<&str> = scored
                    .iter()
                    .filter(|(s, _)| *s >= SUGGESTION_THRESHOLD)
                    .take(3)
                    .map(|(_, entry)| entry.display.as_str())
                    .collect();
                if suggestions.is_empty() {
                    Err(ToolError::Execution(format!(
                        "No encontré ninguna aplicación llamada '{name}'. Si querías abrir un \
                         sitio web, usa open_url; si es un programa, puede que no esté instalado \
                         o tenga otro nombre. Díselo al usuario en vez de reintentar."
                    )))
                } else {
                    Err(ToolError::Execution(format!(
                        "No encontré ninguna aplicación llamada '{name}', pero hay accesos \
                         directos parecidos: {}. Pregúntale al usuario cuál quiso decir en vez \
                         de reintentar por tu cuenta.",
                        suggestions.join(", ")
                    )))
                }
            }
        }
    }
}

pub struct OpenUrl;

impl OpenUrl {
    /// Normaliza a una URL http(s) segura. Antepone https:// a dominios sin
    /// esquema y rechaza esquemas peligrosos (file:, javascript:, etc.).
    fn normalize_url(raw: &str) -> Result<String, ToolError> {
        let raw = raw.trim();
        let lower = raw.to_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Ok(raw.to_string());
        }
        if raw.contains("://") {
            return Err(ToolError::InvalidArgs(format!(
                "solo se permiten URLs http o https, no '{raw}'"
            )));
        }
        // Sin esquema: asumir https si parece un dominio (tiene un punto).
        if raw.contains('.') && !raw.contains(' ') {
            Ok(format!("https://{raw}"))
        } else {
            Err(ToolError::InvalidArgs(format!(
                "'{raw}' no parece una URL válida"
            )))
        }
    }
}

#[async_trait]
impl Tool for OpenUrl {
    fn name(&self) -> &'static str {
        "open_url"
    }

    fn description(&self) -> &'static str {
        "Abre un sitio web en el navegador por defecto del usuario. Úsala para \
         mostrar cualquier página (YouTube, Google, etc.). NO uses run_powershell \
         ni open_app para abrir webs."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL o dominio a abrir, p.ej. 'youtube.com' o 'https://google.com'"
                }
            },
            "required": ["url"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let url = args.get("url").and_then(Value::as_str).unwrap_or("?");
        // Solo el dominio, para que suene natural si alguna vez se hablara.
        let host = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or(url);
        format!("abrir {host}")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let raw = required_str(&args, "url")?;
        let url = Self::normalize_url(raw)?;
        let status = tokio::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(&url)
            .status()
            .await
            .map_err(|e| ToolError::Execution(format!("no se pudo abrir '{url}': {e}")))?;
        if status.success() {
            Ok(ToolOutput::text(format!("Abriendo {url} en el navegador.")))
        } else {
            Err(ToolError::Execution(format!(
                "Windows no pudo abrir la URL '{url}'."
            )))
        }
    }
}

pub struct CloseApp;

#[async_trait]
impl Tool for CloseApp {
    fn name(&self) -> &'static str {
        "close_app"
    }

    fn description(&self) -> &'static str {
        "Cierra una aplicación matando todos sus procesos por nombre \
         (coincidencia parcial, sin distinguir mayúsculas), p.ej. 'notepad', \
         'chrome'."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Nombre (o parte del nombre) del proceso a cerrar"
                }
            },
            "required": ["name"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // Mata procesos: puede perder trabajo sin guardar.
        RiskLevel::Confirm
    }

    fn describe_action(&self, args: &Value) -> String {
        let name = args.get("name").and_then(Value::as_str).unwrap_or("?");
        format!("cerrar todos los procesos de {name}")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let name = required_str(&args, "name")?.to_lowercase();
        if name.len() < 3 {
            return Err(ToolError::InvalidArgs(
                "el nombre debe tener al menos 3 caracteres para no cerrar procesos de más"
                    .to_string(),
            ));
        }
        tokio::task::spawn_blocking(move || {
            use sysinfo::System;
            let sys = System::new_all();
            let mut killed = 0usize;
            for process in sys.processes().values() {
                let pname = process.name().to_string_lossy().to_lowercase();
                if pname.contains(&name) && process.kill() {
                    killed += 1;
                }
            }
            if killed == 0 {
                format!("No encontré ningún proceso en ejecución que coincida con '{name}'.")
            } else {
                format!("Cerrados {killed} procesos que coincidían con '{name}'.")
            }
        })
        .await
        .map(|text| Ok(ToolOutput::text(text)))
        .unwrap_or_else(|e| Err(ToolError::Execution(e.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(display: &str) -> AppEntry {
        let normalized = normalize_phrase(display);
        let tokens = normalized.split_whitespace().map(String::from).collect();
        AppEntry {
            display: display.to_string(),
            normalized,
            tokens,
            path: PathBuf::from(display),
        }
    }

    fn scored(query: &str, display: &str) -> f32 {
        let query_norm = normalize_phrase(query);
        let query_tokens: Vec<String> = query_norm.split_whitespace().map(String::from).collect();
        score(&query_norm, &query_tokens, &entry(display))
    }

    #[test]
    fn nombre_corto_matchea_acceso_directo_mas_largo() {
        // El caso que reportó el usuario: "roblox" no es el nombre literal
        // del acceso directo ("Roblox Player"), pero debe reconocerlo.
        assert!(scored("roblox", "Roblox Player") >= CONFIDENT_THRESHOLD);
    }

    #[test]
    fn coincidencia_exacta_puntua_maximo() {
        assert_eq!(scored("spotify", "Spotify"), 1.0);
    }

    #[test]
    fn tolera_tildes_y_mayusculas() {
        assert!(scored("bloc de notas", "Bloc De Notas") >= CONFIDENT_THRESHOLD);
    }

    #[test]
    fn tolera_error_de_transcripcion_por_token() {
        // "visual studio code" con una letra mal transcrita en un token: no
        // hay substring exacto, pero el solape de tokens (con Levenshtein)
        // basta para sugerirlo aunque no para lanzarlo sin preguntar.
        let s = scored("visual studio cade", "Visual Studio Code");
        assert!(s >= SUGGESTION_THRESHOLD);
        assert!(s < CONFIDENT_THRESHOLD);
    }

    #[test]
    fn nombre_no_relacionado_no_matchea() {
        assert_eq!(scored("roblox", "Microsoft Word"), 0.0);
    }

    #[test]
    fn consulta_muy_corta_no_hace_substring_matching_espurio() {
        // Evita que una sola letra "matchee" cualquier acceso directo.
        assert_eq!(scored("a", "Adobe Acrobat"), 0.0);
    }
}
