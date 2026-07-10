//! Capa de herramientas del agente: cada `Tool` expone su nombre, schema de
//! parámetros y nivel de riesgo, y el `ToolRegistry` las agrupa para
//! ofrecérselas al LLM.
//!
//! Los resultados de `execute` son texto plano compacto en español (no JSON
//! crudo): un modelo local de 7B resume mejor texto ya legible. El registro
//! trunca cada resultado a `agent.max_tool_result_chars`.

pub mod apps;
pub mod files;
pub mod memory;
pub mod shell;
pub mod system_info;
pub mod volume;
pub mod web;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::AgentConfig;
use crate::errors::ToolError;
use crate::llm::ToolSpec;
use crate::memory::MemoryStore;

/// Cuánto peligro implica ejecutar una herramienta con ciertos argumentos.
/// Se evalúa de forma determinista en Rust — nunca lo decide el LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Lectura/consulta: se ejecuta directo.
    Safe,
    /// Modifica el sistema: requiere un "sí" por voz.
    Confirm,
    /// Riesgo extremo (borrado recursivo, apagado, registro...): requiere
    /// pronunciar el código de aceptación de riesgos.
    Code,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON Schema del objeto de parámetros (`{"type": "object", ...}`).
    fn parameters_schema(&self) -> Value;
    /// Clasifica el riesgo SEGÚN LOS ARGUMENTOS, no estático: p.ej.
    /// `run_powershell` es `Confirm` por defecto pero `Code` si el comando
    /// matchea patrones peligrosos.
    fn assess_risk(&self, args: &Value) -> RiskLevel;
    /// Descripción hablada de la acción (más los riesgos identificados, si
    /// los hay) para el diálogo de confirmación: "cerrar Chrome", "ejecutar
    /// el comando ... — esto borra archivos de forma irreversible".
    fn describe_action(&self, args: &Value) -> String;
    async fn execute(&self, args: Value) -> Result<String, ToolError>;
}

pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    max_result_chars: usize,
}

impl ToolRegistry {
    pub fn build(cfg: &AgentConfig, memory: Arc<MemoryStore>) -> Self {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        if cfg.enabled {
            tools.push(Arc::new(system_info::GetDatetime));
            tools.push(Arc::new(system_info::SystemStatus));
            tools.push(Arc::new(system_info::ListProcesses));
            tools.push(Arc::new(apps::OpenApp::new(&cfg.apps)));
            tools.push(Arc::new(apps::OpenUrl));
            tools.push(Arc::new(apps::CloseApp));
            tools.push(Arc::new(files::FindFiles::new(&cfg.files)));
            tools.push(Arc::new(files::OpenFile));
            tools.push(Arc::new(shell::RunPowershell::new(&cfg.high_risk_patterns)));
            tools.push(Arc::new(volume::GetVolume));
            tools.push(Arc::new(volume::SetVolume));
            tools.push(Arc::new(web::WebSearch::new(&cfg.web)));
            tools.push(Arc::new(web::FetchPage::new(&cfg.web)));
            tools.push(Arc::new(memory::Remember::new(memory.clone())));
            tools.push(Arc::new(memory::Recall::new(memory.clone())));
            tools.push(Arc::new(memory::Forget::new(memory.clone())));
        }
        tools.retain(|t| !cfg.disabled_tools.iter().any(|d| d == t.name()));
        Self {
            tools,
            max_result_chars: cfg.max_tool_result_chars,
        }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| ToolSpec {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Recorta un resultado en una frontera de carácter válida, anotando el
    /// corte para que el LLM sepa que falta contenido.
    pub fn truncate_result(&self, result: String) -> String {
        if result.chars().count() <= self.max_result_chars {
            return result;
        }
        let truncated: String = result.chars().take(self.max_result_chars).collect();
        format!("{truncated}\n(...resultado truncado)")
    }
}

/// Lee un argumento string obligatorio del objeto de argumentos.
pub fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidArgs(format!("falta el parámetro '{key}'")))
}
