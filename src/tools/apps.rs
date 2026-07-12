//! Abrir aplicaciones, sitios web y cerrar aplicaciones. `open_app` y
//! `open_url` lanzan con `cmd /C start`, que hereda la resolución de Windows
//! (PATH + App Paths del registro para apps; navegador por defecto para
//! URLs). `close_app` mata procesos por nombre y por eso requiere
//! confirmación.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::AppsConfig;
use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool, ToolOutput};

pub struct OpenApp {
    aliases: HashMap<String, String>,
}

impl OpenApp {
    pub fn new(cfg: &AppsConfig) -> Self {
        // Claves normalizadas a minúsculas para matchear lo transcrito.
        let aliases = cfg
            .aliases
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect();
        Self { aliases }
    }

    fn resolve(&self, name: &str) -> String {
        let key = name.trim().to_lowercase();
        self.aliases.get(&key).cloned().unwrap_or(key)
    }
}

#[async_trait]
impl Tool for OpenApp {
    fn name(&self) -> &'static str {
        "open_app"
    }

    fn description(&self) -> &'static str {
        "Abre una aplicación por su nombre de ejecutable o alias, p.ej. \
         'notepad', 'chrome', 'spotify', 'calc', 'explorer'."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Nombre del ejecutable o alias de la aplicación"
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
        let target = self.resolve(name);
        // `start` no bloquea y resuelve como lo haría el usuario en Win+R.
        let status = tokio::process::Command::new("cmd")
            .args(["/C", "start", "", &target])
            .status()
            .await
            .map_err(|e| ToolError::Execution(format!("no se pudo lanzar '{target}': {e}")))?;
        if status.success() {
            Ok(ToolOutput::text(format!("Aplicación '{target}' lanzada.")))
        } else {
            Err(ToolError::Execution(format!(
                "No encontré ninguna aplicación llamada '{target}'. Si querías abrir un \
                 sitio web, usa open_url; si es un programa, puede que no esté instalado o \
                 tenga otro nombre. Díselo al usuario en vez de reintentar."
            )))
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
