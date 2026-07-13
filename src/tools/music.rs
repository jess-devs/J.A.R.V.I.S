//! Detiene la música de fondo que el propio Jarvis puso en el modo
//! bienvenida (ver `crate::audio::music`). Distinta de `media_control`, que
//! controla la sesión de medios del sistema (Spotify, navegador, etc.) y no
//! tiene forma de tocar el `Sink` de rodio que arma esta escena.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::audio::MusicShared;
use crate::errors::ToolError;

use super::{RiskLevel, Tool, ToolOutput};

pub struct StopMusic {
    pub shared: Arc<MusicShared>,
}

#[async_trait]
impl Tool for StopMusic {
    fn name(&self) -> &'static str {
        "stop_music"
    }

    fn description(&self) -> &'static str {
        "Detiene la música de fondo que puso el propio Jarvis (modo \
         bienvenida, disparado por doble aplauso). Para Spotify u otras \
         apps externas usa media_control, no esta tool."
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
        "detener la música de fondo".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        self.shared.stop();
        Ok(ToolOutput::text("Música detenida, señor."))
    }
}
