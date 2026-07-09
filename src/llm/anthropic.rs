//! Proveedor LLM en la nube vía Anthropic (Claude). `POST /v1/messages` con
//! streaming SSE. Anthropic no acepta mensajes `role: system` dentro del
//! array `messages` — van aparte, en el campo `system` de nivel superior.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::config::AnthropicConfig;
use crate::errors::LlmError;

use super::{ChatMessage, LlmProvider, Role};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;

pub struct AnthropicProvider {
    client: reqwest::Client,
    model: String,
    api_key_env: String,
}

impl AnthropicProvider {
    pub fn new(config: &AnthropicConfig, request_timeout_secs: u64) -> Result<Self, LlmError> {
        if std::env::var(&config.api_key_env).is_err() {
            return Err(LlmError::MissingApiKey(config.api_key_env.clone()));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(request_timeout_secs))
            .build()
            .expect("configuración de cliente reqwest válida");
        Ok(Self {
            client,
            model: config.model.clone(),
            api_key_env: config.api_key_env.clone(),
        })
    }
}

#[derive(Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: Vec<AnthropicMessage<'a>>,
    stream: bool,
}

#[derive(Deserialize)]
struct AnthropicDelta {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicApiError {
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicEvent {
    ContentBlockDelta { delta: AnthropicDelta },
    MessageStop,
    Error { error: AnthropicApiError },
    #[serde(other)]
    Other,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "user", // no debería llegar acá: se filtra antes
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tx: mpsc::Sender<Result<String, LlmError>>,
    ) -> Result<(), LlmError> {
        let api_key = std::env::var(&self.api_key_env)
            .map_err(|_| LlmError::MissingApiKey(self.api_key_env.clone()))?;

        let system = history
            .iter()
            .find(|m| m.role == Role::System)
            .map(|m| m.content.as_str());
        let messages: Vec<AnthropicMessage> = history
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| AnthropicMessage {
                role: role_str(m.role),
                content: &m.content,
            })
            .collect();

        let body = AnthropicRequest {
            model: &self.model,
            max_tokens: MAX_TOKENS,
            system,
            messages,
            stream: true,
        };

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(LlmError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let message = format!("Anthropic respondió {status}: {text}");
            let _ = tx
                .send(Err(LlmError::UnexpectedResponse(message.clone())))
                .await;
            return Err(LlmError::UnexpectedResponse(message));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(LlmError::Network)?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer.drain(..=pos);

                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }

                let event: AnthropicEvent = match serde_json::from_str(data) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                match event {
                    AnthropicEvent::ContentBlockDelta { delta } => {
                        if let Some(text) = delta.text {
                            if !text.is_empty() && tx.send(Ok(text)).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    AnthropicEvent::MessageStop => return Ok(()),
                    AnthropicEvent::Error { error } => {
                        let message = format!("Anthropic devolvió un error: {}", error.message);
                        let _ = tx
                            .send(Err(LlmError::UnexpectedResponse(message.clone())))
                            .await;
                        return Err(LlmError::UnexpectedResponse(message));
                    }
                    AnthropicEvent::Other => {}
                }
            }
        }

        Ok(())
    }
}
