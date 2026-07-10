//! Ejecución de comandos PowerShell arbitrarios. Siempre requiere al menos
//! confirmación por voz; si el comando matchea un patrón de riesgo extremo
//! (borrado recursivo, apagado, registro, etc.) exige además el código de
//! aceptación de riesgos. La clasificación es determinista, acá en Rust.

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};

use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool};

/// Patrones (case-insensitive) que elevan un comando a nivel `Code`. Los de
/// `agent.high_risk_patterns` de la config se suman a estos.
const DEFAULT_HIGH_RISK: [&str; 11] = [
    r"remove-item[^|;]*-recurse",
    r"\brm\s+(-rf|-fr|-r\s+-f)\b",
    r"\bdel\b[^|;]*/s",
    r"\bformat(-volume)?\b",
    r"stop-computer|restart-computer|\bshutdown\b",
    r"\breg\s+(add|delete)\b|set-itemproperty[^|;]*hklm",
    r"set-executionpolicy",
    r"c:\\windows|system32",
    r"\bdiskpart\b",
    r"cipher\s+/w",
    r"\bdisable-\w",
];

pub struct RunPowershell {
    high_risk: Vec<Regex>,
}

impl RunPowershell {
    pub fn new(extra_patterns: &[String]) -> Self {
        let high_risk = DEFAULT_HIGH_RISK
            .iter()
            .map(|p| (*p).to_string())
            .chain(extra_patterns.iter().cloned())
            .filter_map(|p| match Regex::new(&format!("(?i){p}")) {
                Ok(re) => Some(re),
                Err(e) => {
                    tracing::warn!(pattern = %p, error = %e, "patrón de riesgo inválido, se ignora");
                    None
                }
            })
            .collect();
        Self { high_risk }
    }

    fn is_high_risk(&self, command: &str) -> bool {
        self.high_risk.iter().any(|re| re.is_match(command))
    }
}

#[async_trait]
impl Tool for RunPowershell {
    fn name(&self) -> &'static str {
        "run_powershell"
    }

    fn description(&self) -> &'static str {
        "Ejecuta un comando de PowerShell en la computadora y devuelve su \
         salida. Para acciones que ninguna otra herramienta cubre. El sistema \
         pedirá confirmación al usuario por su cuenta."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Comando de PowerShell a ejecutar"
                }
            },
            "required": ["command"]
        })
    }

    fn assess_risk(&self, args: &Value) -> RiskLevel {
        let command = args.get("command").and_then(Value::as_str).unwrap_or("");
        if self.is_high_risk(command) {
            RiskLevel::Code
        } else {
            RiskLevel::Confirm
        }
    }

    fn describe_action(&self, args: &Value) -> String {
        let command = args.get("command").and_then(Value::as_str).unwrap_or("?");
        // El comando se lee literal en voz alta: el usuario debe saber
        // exactamente qué va a ejecutarse antes de aprobarlo.
        if self.is_high_risk(command) {
            format!(
                "ejecutar el comando {command} — atención: este comando puede ser \
                 destructivo o irreversible"
            )
        } else {
            format!("ejecutar el comando {command}")
        }
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let command = required_str(&args, "command")?;
        let output = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", command])
            .output()
            .await
            .map_err(|e| ToolError::Execution(format!("no se pudo lanzar PowerShell: {e}")))?;

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
                format!("El comando falló (código {:?}) sin salida.", output.status.code())
            };
        }
        Ok(result)
    }
}
