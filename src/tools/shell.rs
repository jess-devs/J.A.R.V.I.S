//! Ejecución de comandos PowerShell arbitrarios. Siempre requiere al menos
//! confirmación por voz; si el comando matchea un patrón de riesgo extremo
//! (borrado recursivo, apagado, registro, etc.) exige además el código de
//! aceptación de riesgos. La clasificación es determinista, acá en Rust.

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};

use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool, ToolOutput};

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

/// Ejecuta un comando de PowerShell y devuelve su salida formateada
/// (stdout + stderr, o un mensaje si no hubo salida). Compartida por
/// `RunPowershell` y por `ScriptedTool` (`scripted.rs`) para recetas
/// `Powershell`.
pub async fn run_powershell_command(command: &str) -> Result<String, ToolError> {
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
         salida. Solo para tareas que ninguna otra herramienta cubre (para \
         abrir webs usa open_url, para apps open_app). Incluye SIEMPRE el \
         campo summary: el sistema lo lee al usuario al pedir confirmación."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Comando de PowerShell a ejecutar"
                },
                "summary": {
                    "type": "string",
                    "description": "Descripción breve y natural de qué hace el comando, \
                                    en español hablado, para leérsela al usuario. \
                                    Ej: 'crear una carpeta llamada prueba en el escritorio'."
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
        let summary = args
            .get("summary")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if self.is_high_risk(command) {
            // Alto riesgo: se lee el comando literal para consentimiento
            // informado, además del resumen si lo hay.
            let intro = summary
                .map(|s| s.to_string())
                .unwrap_or_else(|| "ejecutar una acción avanzada en el sistema".to_string());
            format!(
                "{intro}. El comando exacto es: {command}. Atención: puede ser \
                 destructivo o irreversible"
            )
        } else {
            // Riesgo normal: descripción natural, nunca el comando crudo.
            summary
                .map(str::to_string)
                .unwrap_or_else(|| "ejecutar un comando en el sistema".to_string())
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let command = required_str(&args, "command")?;
        let result = run_powershell_command(command).await?;
        Ok(ToolOutput::text(result))
    }
}
