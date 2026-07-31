//! Control de reproducción multimedia vía teclas de medios simuladas
//! (`enigo`, cross-platform). Funciona con cualquier app que tenga la
//! sesión de medios del sistema activa (Spotify, navegador, etc.), sin API
//! keys ni integración por app. Reversible/bajo impacto → `Safe`, igual que
//! `set_volume`.

use async_trait::async_trait;
use enigo::Key;
use serde_json::{json, Value};

use crate::errors::ToolError;

use super::input::send_key;
use super::{required_str, RiskLevel, Tool, ToolOutput};

pub struct MediaControl;

#[async_trait]
impl Tool for MediaControl {
    fn name(&self) -> &'static str {
        "media_control"
    }

    fn description(&self) -> &'static str {
        "Controla la reproducción multimedia actual (Spotify, navegador, \
         etc.): pausar/reanudar, siguiente o anterior pista."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["play_pause", "next", "previous"],
                    "description": "Acción a realizar sobre la reproducción actual"
                }
            },
            "required": ["action"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("?");
        match action {
            "play_pause" => "pausar o reanudar la música".to_string(),
            "next" => "saltar a la siguiente canción".to_string(),
            "previous" => "volver a la canción anterior".to_string(),
            other => format!("ejecutar la acción de medios '{other}'"),
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let action = required_str(&args, "action")?;
        let (key, message) = match action {
            "play_pause" => (Key::MediaPlayPause, "Reproducción pausada o reanudada."),
            "next" => (Key::MediaNextTrack, "Pasando a la siguiente canción."),
            "previous" => (Key::MediaPrevTrack, "Volviendo a la canción anterior."),
            other => {
                return Err(ToolError::InvalidArgs(format!(
                    "acción de medios desconocida: '{other}'"
                )))
            }
        };
        send_key(key).await?;
        Ok(ToolOutput::text(message))
    }
}
