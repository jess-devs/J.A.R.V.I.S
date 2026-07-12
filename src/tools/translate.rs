//! Traducción de texto vía el endpoint público (no oficial, sin API key) de
//! Google Translate — mismo criterio que `web.rs`: scraping/endpoint público
//! en vez de depender de una API key. Solo lectura → `Safe`.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

use crate::config::TranslateConfig;
use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool, ToolOutput};

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Translate {
    client: reqwest::Client,
    cfg: TranslateConfig,
}

impl Translate {
    pub fn new(cfg: &TranslateConfig) -> Self {
        Self {
            client: crate::http::client(FETCH_TIMEOUT),
            cfg: cfg.clone(),
        }
    }
}

#[async_trait]
impl Tool for Translate {
    fn name(&self) -> &'static str {
        "translate"
    }

    fn description(&self) -> &'static str {
        "Traduce un texto de un idioma a otro. Usa códigos de idioma cortos \
         (es, en, fr, pt, de...) o 'auto' para detectar el idioma de origen."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Texto a traducir"
                },
                "target_lang": {
                    "type": "string",
                    "description": "Código del idioma destino (ej. 'en', 'es'). Si se omite, usa el idioma por defecto configurado."
                },
                "source_lang": {
                    "type": "string",
                    "description": "Código del idioma de origen. Si se omite, se detecta automáticamente."
                }
            },
            "required": ["text"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let target = args
            .get("target_lang")
            .and_then(Value::as_str)
            .unwrap_or(&self.cfg.default_target_lang);
        format!("traducir el texto a '{target}'")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let text = required_str(&args, "text")?;
        let target = args
            .get("target_lang")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.cfg.default_target_lang);
        let source = args
            .get("source_lang")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("auto");

        let encoded_text: String = url::form_urlencoded::byte_serialize(text.as_bytes()).collect();
        let url = format!(
            "https://translate.googleapis.com/translate_a/single?client=gtx&sl={source}&tl={target}&dt=t&q={encoded_text}"
        );
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("error de red: {e}")))?;
        if !response.status().is_success() {
            return Err(ToolError::Execution(format!(
                "el traductor respondió {}",
                response.status()
            )));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("respuesta inesperada del traductor: {e}")))?;

        // Formato: [[[ "traducido", "original", null, null, ...], ...], ...]
        let translated: String = body
            .get(0)
            .and_then(Value::as_array)
            .map(|segments| {
                segments
                    .iter()
                    .filter_map(|seg| seg.get(0).and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        if translated.trim().is_empty() {
            return Ok(ToolOutput::text(format!(
                "No se pudo traducir el texto a '{target}'."
            )));
        }
        Ok(ToolOutput::text(translated))
    }
}
