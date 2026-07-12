//! Volumen maestro de Windows vía Core Audio (`IAudioEndpointVolume`).
//! Acción reversible al instante → `Safe`, sin confirmación.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::errors::ToolError;

use super::{RiskLevel, Tool, ToolOutput};

/// Ejecuta `f` sobre el endpoint de volumen del dispositivo de salida por
/// defecto, con COM inicializado en un hilo bloqueante.
async fn with_endpoint_volume<T, F>(f: F) -> Result<T, ToolError>
where
    T: Send + 'static,
    F: FnOnce(
            &windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
        ) -> windows::core::Result<T>
        + Send
        + 'static,
{
    tokio::task::spawn_blocking(move || {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        unsafe {
            // Puede devolver S_FALSE si el hilo ya estaba inicializado; da igual.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| ToolError::Execution(format!("COM: {e}")))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| ToolError::Execution(format!("sin dispositivo de salida: {e}")))?;
            let volume: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| ToolError::Execution(format!("endpoint de volumen: {e}")))?;
            f(&volume).map_err(|e| ToolError::Execution(format!("volumen: {e}")))
        }
    })
    .await
    .map_err(|e| ToolError::Execution(e.to_string()))?
}

pub struct GetVolume;

#[async_trait]
impl Tool for GetVolume {
    fn name(&self) -> &'static str {
        "get_volume"
    }

    fn description(&self) -> &'static str {
        "Consulta el volumen maestro actual de la computadora (0 a 100) y si \
         está silenciada."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "consultar el volumen".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let (level, muted) = with_endpoint_volume(|v| unsafe {
            let level = v.GetMasterVolumeLevelScalar()?;
            let muted = v.GetMute()?.as_bool();
            Ok((level, muted))
        })
        .await?;
        let pct = (level * 100.0).round() as u32;
        Ok(ToolOutput::text(if muted {
            format!("El volumen está en {pct}% pero la salida está silenciada.")
        } else {
            format!("El volumen está en {pct}%.")
        }))
    }
}

pub struct SetVolume;

#[async_trait]
impl Tool for SetVolume {
    fn name(&self) -> &'static str {
        "set_volume"
    }

    fn description(&self) -> &'static str {
        "Ajusta el volumen maestro de la computadora. Parámetro percent: 0 a \
         100. También quita el silencio si estaba activado."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "percent": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 100,
                    "description": "Nivel de volumen deseado (0-100)"
                }
            },
            "required": ["percent"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // Reversible al instante: no vale la pena una confirmación.
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let pct = args.get("percent").and_then(Value::as_u64).unwrap_or(0);
        format!("poner el volumen al {pct} por ciento")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let pct = args
            .get("percent")
            .and_then(Value::as_u64)
            .ok_or_else(|| ToolError::InvalidArgs("falta el parámetro 'percent'".to_string()))?
            .min(100);
        let scalar = pct as f32 / 100.0;
        with_endpoint_volume(move |v| unsafe {
            v.SetMasterVolumeLevelScalar(scalar, std::ptr::null())?;
            v.SetMute(false, std::ptr::null())?;
            Ok(())
        })
        .await?;
        Ok(ToolOutput::text(format!("Volumen ajustado al {pct}%.")))
    }
}
