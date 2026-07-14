//! Modo silencio: el usuario pide (con cualquier fraseo) que Jarvis se quede
//! callado. No apaga el micrófono ni la transcripción — solo avisa al
//! orquestador (vía `flag`) que no debe reabrir la ventana de atención al
//! terminar este turno (ver `Orchestrator::finish_turn`), así que a partir de
//! acá Jarvis exige de nuevo su nombre para cualquier respuesta, sin límite
//! de tiempo, hasta que lo llamen.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::errors::ToolError;

use super::{RiskLevel, Tool, ToolOutput};

pub struct EnterSilenceMode {
    pub flag: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for EnterSilenceMode {
    fn name(&self) -> &'static str {
        "enter_silence_mode"
    }

    fn description(&self) -> &'static str {
        "Actívala cuando el usuario pida, con cualquier fraseo (no hace falta \
         que diga literalmente 'guarda silencio'), que Jarvis se quede \
         callado, deje de hablar o deje de responder por un rato. Tras \
         llamarla, Jarvis deja de reaccionar a cualquier frase hasta que lo \
         vuelvan a llamar por su nombre; en ese momento el comportamiento \
         normal se restablece solo."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "entrar en modo silencio".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        self.flag.store(true, Ordering::SeqCst);
        Ok(ToolOutput::text(
            "Modo silencio activado: no volveré a responder hasta que me llames por mi nombre.",
        ))
    }
}
