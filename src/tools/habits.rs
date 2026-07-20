//! Tools para que el LLM registre la decisión del usuario tras una
//! sugerencia de personalidad adaptativa (ver `crate::habits`). Solo se
//! llaman en respuesta al evento sintético que el orquestador manda cuando
//! el scanner detecta un patrón — nunca por iniciativa propia del modelo.
//! Ambas son `Safe`: la aprobación real ya la dio el usuario por voz en la
//! conversación, la tool solo la deja registrada.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::errors::ToolError;
use crate::habits::HabitStore;
use crate::memory::MemoryStore;

use super::{required_str, RiskLevel, Tool, ToolOutput};

fn suggestion_id(args: &Value) -> Result<i64, ToolError> {
    args.get("suggestion_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| ToolError::InvalidArgs("falta el parámetro 'suggestion_id'".to_string()))
}

pub struct AcceptSuggestion {
    habits: Arc<HabitStore>,
    memory: Arc<MemoryStore>,
}

impl AcceptSuggestion {
    pub fn new(habits: Arc<HabitStore>, memory: Arc<MemoryStore>) -> Self {
        Self { habits, memory }
    }
}

#[async_trait]
impl Tool for AcceptSuggestion {
    fn name(&self) -> &'static str {
        "accept_suggestion"
    }

    fn description(&self) -> &'static str {
        "Registra que el usuario aceptó una sugerencia de adaptación que el \
         sistema te propuso (evento de patrón de uso detectado). Solo la \
         usas tras ese evento, con el suggestion_id que vino en él."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "suggestion_id": {
                    "type": "integer",
                    "description": "El id de la sugerencia, tal como vino en el evento del sistema"
                },
                "adaptation": {
                    "type": "string",
                    "description": "Frase autocontenida que describe la preferencia aceptada, \
                        p.ej. 'al usuario le gusta que le proponga abrir spotify por la mañana'"
                }
            },
            "required": ["suggestion_id", "adaptation"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "registrar una adaptación aceptada".to_string()
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let id = suggestion_id(&args)?;
        let adaptation = required_str(&args, "adaptation")?;
        if !self.habits.resolve(id, true).await? {
            return Ok(ToolOutput::text("No encuentro esa sugerencia, señor."));
        }
        self.memory
            .remember(
                adaptation,
                Some("adaptacion"),
                Some("el usuario aceptó esta sugerencia de adaptación tras detectarse un patrón de uso"),
            )
            .await?;
        Ok(ToolOutput::text(format!(
            "Adaptación registrada: {adaptation}"
        )))
    }
}

pub struct DismissSuggestion {
    habits: Arc<HabitStore>,
}

impl DismissSuggestion {
    pub fn new(habits: Arc<HabitStore>) -> Self {
        Self { habits }
    }
}

#[async_trait]
impl Tool for DismissSuggestion {
    fn name(&self) -> &'static str {
        "dismiss_suggestion"
    }

    fn description(&self) -> &'static str {
        "Registra que el usuario rechazó una sugerencia de adaptación que el \
         sistema te propuso. Solo la usas tras ese evento, con el \
         suggestion_id que vino en él."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "suggestion_id": {
                    "type": "integer",
                    "description": "El id de la sugerencia, tal como vino en el evento del sistema"
                }
            },
            "required": ["suggestion_id"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "descartar una sugerencia de adaptación".to_string()
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let id = suggestion_id(&args)?;
        if !self.habits.resolve(id, false).await? {
            return Ok(ToolOutput::text("No encuentro esa sugerencia, señor."));
        }
        Ok(ToolOutput::text("Entendido, no volveré a sugerirlo."))
    }
}
