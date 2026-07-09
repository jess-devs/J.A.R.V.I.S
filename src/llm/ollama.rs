//! Proveedor LLM local vía Ollama (`POST /api/chat` con `stream: true`,
//! NDJSON línea por línea). Foco principal del proyecto.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::config::OllamaConfig;
use crate::errors::LlmError;

use super::{ChatMessage, LlmProvider, Role};

pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig, request_timeout_secs: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(request_timeout_secs))
            .build()
            .expect("configuración de cliente reqwest válida");
        Self {
            client,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
        }
    }
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage<'a>>,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaChunkMessage {
    content: String,
}

#[derive(Deserialize)]
struct OllamaChatChunk {
    message: Option<OllamaChunkMessage>,
    #[serde(default)]
    done: bool,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tx: mpsc::Sender<Result<String, LlmError>>,
    ) -> Result<(), LlmError> {
        let url = format!("{}/api/chat", self.base_url);
        let messages: Vec<OllamaMessage> = history
            .iter()
            .map(|m| OllamaMessage {
                role: role_str(m.role),
                content: &m.content,
            })
            .collect();
        let body = OllamaChatRequest {
            model: &self.model,
            messages,
            stream: true,
        };

        let response = match self.client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(_) => {
                let err = LlmError::OllamaUnreachable {
                    base_url: self.base_url.clone(),
                };
                let _ = tx
                    .send(Err(LlmError::OllamaUnreachable {
                        base_url: self.base_url.clone(),
                    }))
                    .await;
                return Err(err);
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let err = LlmError::UnexpectedResponse(format!("Ollama respondió {status}"));
            let _ = tx
                .send(Err(LlmError::UnexpectedResponse(format!(
                    "Ollama respondió {status}"
                ))))
                .await;
            return Err(err);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let message = e.to_string();
                    let _ = tx
                        .send(Err(LlmError::UnexpectedResponse(message.clone())))
                        .await;
                    return Err(LlmError::UnexpectedResponse(message));
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer.drain(..=pos);
                if line.is_empty() {
                    continue;
                }
                let parsed: OllamaChatChunk = match serde_json::from_str(&line) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::UnexpectedResponse(e.to_string()))).await;
                        continue;
                    }
                };
                if let Some(msg) = parsed.message {
                    if !msg.content.is_empty() && tx.send(Ok(msg.content)).await.is_err() {
                        // El receptor cerró el channel (ej. la respuesta ya no interesa) — no es un error del proveedor.
                        return Ok(());
                    }
                }
                if parsed.done {
                    return Ok(());
                }
            }
        }

        Ok(())
    }
}
