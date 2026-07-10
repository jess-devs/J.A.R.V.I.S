//! Abrir y cerrar aplicaciones. `open_app` lanza con `cmd /C start`, que
//! hereda la resolución de Windows (PATH + App Paths del registro): "chrome",
//! "notepad" o "spotify" funcionan sin rutas. `close_app` mata procesos por
//! nombre y por eso requiere confirmación.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::AppsConfig;
use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool};

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

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let name = required_str(&args, "name")?;
        let target = self.resolve(name);
        // `start` no bloquea y resuelve como lo haría el usuario en Win+R.
        let status = tokio::process::Command::new("cmd")
            .args(["/C", "start", "", &target])
            .status()
            .await
            .map_err(|e| ToolError::Execution(format!("no se pudo lanzar '{target}': {e}")))?;
        if status.success() {
            Ok(format!("Aplicación '{target}' lanzada."))
        } else {
            Err(ToolError::Execution(format!(
                "Windows no encontró ninguna aplicación llamada '{target}'."
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

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
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
        .map(Ok)
        .unwrap_or_else(|e| Err(ToolError::Execution(e.to_string())))
    }
}
