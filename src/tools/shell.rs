//! Ejecución de comandos de shell arbitrarios: PowerShell en Windows
//! (`RunPowershell`), `sh -c` en Unix (`RunShell`, ver `tools/mod.rs` para
//! cuál se registra según la plataforma). Siempre requiere al menos
//! confirmación por voz; si el comando matchea un patrón de riesgo extremo
//! (borrado recursivo, apagado, registro/permisos peligrosos, etc.) exige
//! además el código de aceptación de riesgos. La clasificación es
//! determinista, acá en Rust.

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};

use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool, ToolOutput};

/// Patrones (case-insensitive) que elevan un comando de PowerShell a nivel
/// `Code`. Los de `agent.high_risk_patterns` de la config se suman a estos.
#[cfg(windows)]
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

/// Patrones (case-insensitive) que elevan un comando de shell Unix a nivel
/// `Code`. Los de `agent.high_risk_patterns` de la config se suman a estos.
#[cfg(unix)]
const DEFAULT_HIGH_RISK_UNIX: [&str; 8] = [
    r"\brm\s+(-[a-z]*r[a-z]*f|-[a-z]*f[a-z]*r)\b",
    r"\bmkfs(\.\w+)?\b",
    r"\bdd\s+if=",
    r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:", // fork bomb clásica
    r"\bchmod\s+(-r\s+)?777\s+/",
    r">\s*/dev/sd",
    r"\bshutdown\b|\bpoweroff\b|\breboot\b|\bhalt\b",
    r"\bmv\s+/\S*\s+/dev/null",
];

/// Compila los patrones de riesgo extremo (defaults + los extra de config)
/// y expone si un comando matchea alguno. Compartido por `RunPowershell` y
/// `RunShell`.
struct HighRiskMatcher {
    patterns: Vec<Regex>,
}

impl HighRiskMatcher {
    fn new(defaults: &[&str], extra_patterns: &[String]) -> Self {
        let patterns = defaults
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
        Self { patterns }
    }

    fn is_high_risk(&self, command: &str) -> bool {
        self.patterns.iter().any(|re| re.is_match(command))
    }
}

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
            format!(
                "El comando falló (código {:?}) sin salida.",
                output.status.code()
            )
        };
    }
    Ok(result)
}

#[cfg(windows)]
pub struct RunPowershell {
    high_risk: HighRiskMatcher,
}

#[cfg(windows)]
impl RunPowershell {
    pub fn new(extra_patterns: &[String]) -> Self {
        Self {
            high_risk: HighRiskMatcher::new(&DEFAULT_HIGH_RISK, extra_patterns),
        }
    }

    fn is_high_risk(&self, command: &str) -> bool {
        self.high_risk.is_high_risk(command)
    }
}

#[cfg(windows)]
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

/// Equivalente Unix de `RunPowershell`: ejecuta comandos vía `sh -c`. Misma
/// clasificación de riesgo determinista, con su propia lista de patrones
/// bash/coreutils en vez de cmdlets de PowerShell.
#[cfg(unix)]
pub struct RunShell {
    high_risk: HighRiskMatcher,
}

#[cfg(unix)]
impl RunShell {
    pub fn new(extra_patterns: &[String]) -> Self {
        Self {
            high_risk: HighRiskMatcher::new(&DEFAULT_HIGH_RISK_UNIX, extra_patterns),
        }
    }

    fn is_high_risk(&self, command: &str) -> bool {
        self.high_risk.is_high_risk(command)
    }
}

#[cfg(unix)]
#[async_trait]
impl Tool for RunShell {
    fn name(&self) -> &'static str {
        "run_shell"
    }

    fn description(&self) -> &'static str {
        "Ejecuta un comando de shell (sh -c) en la computadora y devuelve su \
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
                    "description": "Comando de shell a ejecutar"
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
            let intro = summary
                .map(|s| s.to_string())
                .unwrap_or_else(|| "ejecutar una acción avanzada en el sistema".to_string());
            format!(
                "{intro}. El comando exacto es: {command}. Atención: puede ser \
                 destructivo o irreversible"
            )
        } else {
            summary
                .map(str::to_string)
                .unwrap_or_else(|| "ejecutar un comando en el sistema".to_string())
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let command = required_str(&args, "command")?;
        let result = crate::platform::run_shell_command(command).await?;
        Ok(ToolOutput::text(result))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn tool() -> RunPowershell {
        RunPowershell::new(&[])
    }

    fn risk_of(tool: &RunPowershell, command: &str) -> RiskLevel {
        tool.assess_risk(&json!({ "command": command }))
    }

    #[test]
    fn comando_benigno_es_confirm() {
        let t = tool();
        assert_eq!(risk_of(&t, "Get-Process"), RiskLevel::Confirm);
        assert_eq!(
            risk_of(&t, "New-Item -ItemType Directory -Path prueba"),
            RiskLevel::Confirm
        );
    }

    #[test]
    fn borrado_recursivo_es_code() {
        let t = tool();
        assert_eq!(
            risk_of(&t, "Remove-Item C:\\temp -Recurse -Force"),
            RiskLevel::Code
        );
        assert_eq!(risk_of(&t, "rm -rf /algo"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "del /s C:\\temp"), RiskLevel::Code);
    }

    #[test]
    fn apagado_y_reinicio_son_code() {
        let t = tool();
        assert_eq!(risk_of(&t, "Stop-Computer -Force"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "Restart-Computer"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "shutdown /s /t 0"), RiskLevel::Code);
    }

    #[test]
    fn registro_de_windows_es_code() {
        let t = tool();
        assert_eq!(
            risk_of(&t, "reg add HKLM\\Software\\Test /v X /d 1"),
            RiskLevel::Code
        );
        assert_eq!(
            risk_of(
                &t,
                "Set-ItemProperty -Path HKLM:\\Software\\Test -Name X -Value 1"
            ),
            RiskLevel::Code
        );
    }

    #[test]
    fn formateo_y_diskpart_son_code() {
        let t = tool();
        assert_eq!(risk_of(&t, "Format-Volume -DriveLetter D"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "diskpart /s script.txt"), RiskLevel::Code);
    }

    #[test]
    fn clasificacion_es_case_insensitive() {
        let t = tool();
        assert_eq!(
            risk_of(&t, "REMOVE-ITEM C:\\temp -RECURSE"),
            RiskLevel::Code
        );
    }

    #[test]
    fn patrones_extra_de_config_se_suman_a_los_default() {
        let t = RunPowershell::new(&["net\\s+user".to_string()]);
        assert_eq!(risk_of(&t, "net user hacker /add"), RiskLevel::Code);
        // Los defaults se siguen aplicando igual.
        assert_eq!(risk_of(&t, "rm -rf /algo"), RiskLevel::Code);
    }
}

#[cfg(all(test, unix))]
mod shell_tests {
    use super::*;

    fn tool() -> RunShell {
        RunShell::new(&[])
    }

    fn risk_of(tool: &RunShell, command: &str) -> RiskLevel {
        tool.assess_risk(&json!({ "command": command }))
    }

    #[test]
    fn comando_benigno_es_confirm() {
        let t = tool();
        assert_eq!(risk_of(&t, "ls -la"), RiskLevel::Confirm);
        assert_eq!(risk_of(&t, "mkdir prueba"), RiskLevel::Confirm);
    }

    #[test]
    fn borrado_recursivo_es_code() {
        let t = tool();
        assert_eq!(risk_of(&t, "rm -rf /algo"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "rm -fr /algo"), RiskLevel::Code);
    }

    #[test]
    fn apagado_reinicio_y_formateo_son_code() {
        let t = tool();
        assert_eq!(risk_of(&t, "sudo shutdown -h now"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "reboot"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "mkfs.ext4 /dev/sdb1"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "dd if=/dev/zero of=/dev/sda"), RiskLevel::Code);
    }

    #[test]
    fn fork_bomb_es_code() {
        let t = tool();
        assert_eq!(risk_of(&t, ":(){ :|:& };:"), RiskLevel::Code);
    }

    #[test]
    fn chmod_777_root_es_code() {
        let t = tool();
        assert_eq!(risk_of(&t, "chmod -R 777 /"), RiskLevel::Code);
    }

    #[test]
    fn patrones_extra_de_config_se_suman_a_los_default() {
        let t = RunShell::new(&["userdel\\s+-r".to_string()]);
        assert_eq!(risk_of(&t, "userdel -r hacker"), RiskLevel::Code);
        assert_eq!(risk_of(&t, "rm -rf /algo"), RiskLevel::Code);
    }
}

/// Corre en las dos plataformas, a diferencia de `shell_tests` (solo unix).
#[cfg(test)]
mod nombre_de_tool_tests {
    use super::*;

    /// El prompt de sistema nombra la tool de shell vía
    /// `platform::os_and_shell_tool`. Si ese nombre y el de la tool que de
    /// verdad se registra se separan, el modelo llama a algo que no existe —
    /// que es exactamente lo que pasaba diciendo "run_powershell" en Linux.
    #[test]
    fn el_nombre_del_prompt_coincide_con_la_tool_registrada() {
        #[cfg(windows)]
        let registrada = RunPowershell::new(&[]);
        #[cfg(unix)]
        let registrada = RunShell::new(&[]);

        assert_eq!(registrada.name(), crate::platform::os_and_shell_tool().1);
    }
}
