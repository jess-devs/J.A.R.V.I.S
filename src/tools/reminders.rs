//! Recordatorios: crear, listar y cancelar. El LLM calcula `trigger_at` en
//! ISO 8601 usando la fecha/hora actual que ya recibe en el contexto de cada
//! turno (ver `system_info::fecha_hora_es`) — no hay parsing de lenguaje
//! natural en Rust.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::RemindersConfig;
use crate::errors::ToolError;
use crate::reminders::ReminderStore;

use super::{required_str, RiskLevel, Tool, ToolOutput};

pub struct CreateReminder {
    store: Arc<ReminderStore>,
    max_active: usize,
}

impl CreateReminder {
    pub fn new(store: Arc<ReminderStore>, cfg: &RemindersConfig) -> Self {
        Self {
            store,
            max_active: cfg.max_active,
        }
    }
}

#[async_trait]
impl Tool for CreateReminder {
    fn name(&self) -> &'static str {
        "create_reminder"
    }

    fn description(&self) -> &'static str {
        "Crea un recordatorio que Jarvis dirá por voz en el momento indicado. \
         Calcula trigger_at en formato ISO 8601 ('AAAA-MM-DDTHH:MM:SS') a \
         partir de la fecha/hora actual del contexto."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Qué decir cuando llegue el momento"
                },
                "trigger_at": {
                    "type": "string",
                    "description": "Momento en que debe dispararse, formato ISO 8601 (AAAA-MM-DDTHH:MM:SS), hora local"
                }
            },
            "required": ["text", "trigger_at"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let text = args.get("text").and_then(Value::as_str).unwrap_or("?");
        format!("crear un recordatorio: {text}")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let text = required_str(&args, "text")?;
        let trigger_at = required_str(&args, "trigger_at")?;
        self.store.create(text, trigger_at, self.max_active).await?;
        Ok(ToolOutput::text(format!(
            "Recordatorio guardado para {trigger_at}: {text}"
        )))
    }
}

pub struct ListReminders {
    store: Arc<ReminderStore>,
}

impl ListReminders {
    pub fn new(store: Arc<ReminderStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ListReminders {
    fn name(&self) -> &'static str {
        "list_reminders"
    }

    fn description(&self) -> &'static str {
        "Lista los recordatorios activos (aún no disparados ni cancelados)."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "listar los recordatorios activos".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let reminders = self.store.list_active().await?;
        if reminders.is_empty() {
            return Ok(ToolOutput::text("No hay recordatorios activos."));
        }
        let mut out = String::from("Recordatorios activos:\n");
        for r in reminders {
            out.push_str(&format!("- [{}] {}\n", r.trigger_at, r.text));
        }
        Ok(ToolOutput::text(out))
    }
}

pub struct CancelReminder {
    store: Arc<ReminderStore>,
}

impl CancelReminder {
    pub fn new(store: Arc<ReminderStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for CancelReminder {
    fn name(&self) -> &'static str {
        "cancel_reminder"
    }

    fn description(&self) -> &'static str {
        "Cancela los recordatorios activos cuyo texto coincida con la \
         búsqueda. El sistema pedirá confirmación al usuario."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Palabras clave del recordatorio a cancelar"
                }
            },
            "required": ["query"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Confirm
    }

    fn describe_action(&self, args: &Value) -> String {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("?");
        format!("cancelar el recordatorio relacionado con '{query}'")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let query = required_str(&args, "query")?;
        let cancelled = self.store.cancel_matching(query).await?;
        Ok(ToolOutput::text(if cancelled == 0 {
            format!("No había recordatorios activos relacionados con '{query}'.")
        } else {
            format!("Cancelados {cancelled} recordatorios relacionados con '{query}'.")
        }))
    }
}
