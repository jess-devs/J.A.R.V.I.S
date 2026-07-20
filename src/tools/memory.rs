//! Herramientas de memoria persistente: recordar, buscar y olvidar hechos
//! entre sesiones. `forget` borra datos → requiere confirmación por voz.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::errors::ToolError;
use crate::memory::MemoryStore;

use super::{required_str, RiskLevel, Tool, ToolOutput};

pub struct Remember {
    store: Arc<MemoryStore>,
}

impl Remember {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for Remember {
    fn name(&self) -> &'static str {
        "remember"
    }

    fn description(&self) -> &'static str {
        "Guarda un hecho en la memoria permanente para futuras sesiones \
         (preferencias, fechas, datos del usuario). Redacta el hecho completo \
         y autocontenido, p.ej. 'el cumpleaños del usuario es el 3 de marzo'. \
         Indica siempre en 'reason' qué dijo o hizo el usuario que motivó \
         guardarlo."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "El hecho a recordar, completo y autocontenido"
                },
                "category": {
                    "type": "string",
                    "description": "Categoría opcional: personal, preferencias, trabajo..."
                },
                "reason": {
                    "type": "string",
                    "description": "Por qué guardás esto: qué dijo o hizo el usuario que lo \
                        motivó, p.ej. 'el usuario me pidió recordarlo' o 'lo mencionó al \
                        pedir música'"
                }
            },
            "required": ["content", "reason"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let content = args.get("content").and_then(Value::as_str).unwrap_or("?");
        format!("recordar que {content}")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let content = required_str(&args, "content")?;
        let category = args.get("category").and_then(Value::as_str);
        let reason = args
            .get("reason")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        self.store.remember(content, category, reason).await?;
        Ok(ToolOutput::text(format!("Memorizado: {content}")))
    }
}

pub struct Recall {
    store: Arc<MemoryStore>,
}

impl Recall {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for Recall {
    fn name(&self) -> &'static str {
        "recall"
    }

    fn description(&self) -> &'static str {
        "Busca en la memoria permanente hechos guardados en sesiones \
         anteriores que no aparezcan ya en tu contexto. Cada memoria incluye \
         el motivo por el que se guardó: usa esta herramienta cuando el \
         usuario pregunte por qué recordás algo, y respondé solo con el \
         motivo guardado."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Palabras clave a buscar en las memorias"
                }
            },
            "required": ["query"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("?");
        format!("buscar '{query}' en la memoria")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let query = required_str(&args, "query")?;
        let memories = self.store.recall(query, 10).await?;
        if memories.is_empty() {
            return Ok(ToolOutput::text(format!(
                "No hay memorias que coincidan con '{query}'."
            )));
        }
        let mut out = String::from("Memorias encontradas:\n");
        for m in memories {
            let cat = m
                .category
                .as_deref()
                .map(|c| format!(" [{c}]"))
                .unwrap_or_default();
            let motivo = match m.reason.as_deref() {
                Some(r) => format!(" — motivo: {r}"),
                None => " — motivo: no quedó registrado".to_string(),
            };
            out.push_str(&format!(
                "- {}{cat} (guardado: {}){motivo}\n",
                m.content, m.created_at
            ));
        }
        Ok(ToolOutput::text(out))
    }
}

pub struct Forget {
    store: Arc<MemoryStore>,
}

impl Forget {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for Forget {
    fn name(&self) -> &'static str {
        "forget"
    }

    fn description(&self) -> &'static str {
        "Borra de la memoria permanente los hechos que coincidan con las \
         palabras clave. El sistema pedirá confirmación al usuario."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Palabras clave de las memorias a borrar"
                }
            },
            "required": ["query"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // Borra datos del usuario: confirmación por voz.
        RiskLevel::Confirm
    }

    fn describe_action(&self, args: &Value) -> String {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("?");
        format!("borrar de la memoria lo relacionado con '{query}'")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let query = required_str(&args, "query")?;
        let deleted = self.store.forget(query).await?;
        Ok(ToolOutput::text(if deleted == 0 {
            format!("No había memorias que coincidieran con '{query}'.")
        } else {
            format!("Borradas {deleted} memorias relacionadas con '{query}'.")
        }))
    }
}
