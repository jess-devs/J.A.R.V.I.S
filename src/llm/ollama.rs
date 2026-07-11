//! Proveedor LLM local vía Ollama (`POST /api/chat` con `stream: true`,
//! NDJSON línea por línea). Foco principal del proyecto.
//!
//! Tool calling: Ollama entrega cada tool call completo en un solo chunk
//! (`message.tool_calls[].function.arguments` ya es un objeto JSON), así que
//! no hace falta acumular fragmentos — solo generar un id sintético
//! ("call_{n}") porque Ollama no provee ids.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::config::OllamaConfig;
use crate::errors::LlmError;

use super::decode::Utf8StreamDecoder;
use super::{ChatMessage, LlmEvent, LlmProvider, Role, ToolCallRequest, ToolSpec};

pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    /// Solo para modelos con razonamiento (qwen3, deepseek-r1): `false`
    /// desactiva los tokens de "pensamiento" (que el TTS hablaría en voz
    /// alta). No enviar para modelos que no lo soportan (Ollama lo rechaza).
    think: Option<bool>,
}

impl OllamaProvider {
    pub fn new(config: &OllamaConfig, request_timeout_secs: u64) -> Self {
        let client = crate::http::client(Duration::from_secs(request_timeout_secs));
        Self {
            client,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
            think: config.think,
        }
    }
}

#[derive(Serialize)]
struct OllamaFunctionCall<'a> {
    name: &'a str,
    arguments: &'a serde_json::Value,
}

#[derive(Serialize)]
struct OllamaToolCall<'a> {
    function: OllamaFunctionCall<'a>,
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OllamaToolCall<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
}

#[derive(Serialize)]
struct OllamaToolFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
}

#[derive(Serialize)]
struct OllamaTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OllamaToolFunction<'a>,
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
}

#[derive(Deserialize)]
struct OllamaChunkFunction {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct OllamaChunkToolCall {
    function: OllamaChunkFunction,
}

#[derive(Deserialize)]
struct OllamaChunkMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<OllamaChunkToolCall>,
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
        Role::Tool => "tool",
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
    ) -> Result<(), LlmError> {
        let url = format!("{}/api/chat", self.base_url);
        let messages: Vec<OllamaMessage> = history
            .iter()
            .map(|m| OllamaMessage {
                role: role_str(m.role),
                content: &m.content,
                tool_calls: m
                    .tool_calls
                    .iter()
                    .map(|c| OllamaToolCall {
                        function: OllamaFunctionCall {
                            name: &c.name,
                            arguments: &c.arguments,
                        },
                    })
                    .collect(),
                tool_name: m.tool_name.as_deref(),
            })
            .collect();
        let body = OllamaChatRequest {
            model: &self.model,
            messages,
            stream: true,
            tools: tools
                .iter()
                .map(|t| OllamaTool {
                    kind: "function",
                    function: OllamaToolFunction {
                        name: &t.name,
                        description: &t.description,
                        parameters: &t.parameters,
                    },
                })
                .collect(),
            think: self.think,
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
            let text = response.text().await.unwrap_or_default();
            let message = format!("Ollama respondió {status}: {text}");
            let err = LlmError::UnexpectedResponse(message.clone());
            let _ = tx.send(Err(LlmError::UnexpectedResponse(message))).await;
            return Err(err);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut decoder = Utf8StreamDecoder::new();
        let mut call_counter = 0usize;

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
            decoder.feed(&chunk, &mut buffer);

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
                    if !msg.content.is_empty()
                        && tx.send(Ok(LlmEvent::TextDelta(msg.content))).await.is_err()
                    {
                        // El receptor cerró el channel (ej. la respuesta ya no interesa) — no es un error del proveedor.
                        return Ok(());
                    }
                    for call in msg.tool_calls {
                        let request = ToolCallRequest {
                            id: format!("call_{call_counter}"),
                            name: call.function.name,
                            arguments: call.function.arguments,
                        };
                        call_counter += 1;
                        if tx.send(Ok(LlmEvent::ToolCall(request))).await.is_err() {
                            return Ok(());
                        }
                    }
                }
                if parsed.done {
                    let _ = tx.send(Ok(LlmEvent::Done)).await;
                    return Ok(());
                }
            }
        }

        let _ = tx.send(Ok(LlmEvent::Done)).await;
        Ok(())
    }
}
