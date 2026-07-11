//! Proveedor LLM en la nube vía Anthropic (Claude). `POST /v1/messages` con
//! streaming SSE. Anthropic no acepta mensajes `role: system` dentro del
//! array `messages` — van aparte, en el campo `system` de nivel superior.
//!
//! Tool calling: los tool calls van como bloques `tool_use` dentro del
//! content del assistant, y los resultados como bloques `tool_result`
//! dentro de un mensaje `user`. Todos los resultados de un mismo turno
//! deben ir juntos en el user message inmediatamente siguiente, por eso
//! los mensajes `Role::Tool` consecutivos se fusionan en uno solo.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::config::AnthropicConfig;
use crate::errors::LlmError;

use super::decode::Utf8StreamDecoder;
use super::{ChatMessage, LlmEvent, LlmProvider, Role, ToolCallRequest, ToolSpec};

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
        let client = crate::http::client(Duration::from_secs(request_timeout_secs));
        Ok(Self {
            client,
            model: config.model.clone(),
            api_key_env: config.api_key_env.clone(),
        })
    }
}

/// Convierte el historial neutro al array `messages` de Anthropic,
/// fusionando mensajes `Tool` consecutivos en un solo `user` con múltiples
/// bloques `tool_result`.
fn build_messages(history: &[ChatMessage]) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    let mut pending_results: Vec<Value> = Vec::new();

    fn flush_results(messages: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if !pending.is_empty() {
            messages.push(json!({ "role": "user", "content": std::mem::take(pending) }));
        }
    }

    for m in history {
        match m.role {
            Role::System => continue, // va en el campo `system` de nivel superior
            Role::Tool => {
                pending_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.as_deref().unwrap_or_default(),
                    "content": m.content,
                }));
            }
            Role::User => {
                flush_results(&mut messages, &mut pending_results);
                messages.push(json!({ "role": "user", "content": m.content }));
            }
            Role::Assistant => {
                flush_results(&mut messages, &mut pending_results);
                if m.tool_calls.is_empty() {
                    messages.push(json!({ "role": "assistant", "content": m.content }));
                } else {
                    let mut blocks: Vec<Value> = Vec::new();
                    if !m.content.trim().is_empty() {
                        blocks.push(json!({ "type": "text", "text": m.content }));
                    }
                    for call in &m.tool_calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": call.arguments,
                        }));
                    }
                    messages.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
        }
    }
    flush_results(&mut messages, &mut pending_results);
    messages
}

#[derive(Deserialize)]
struct AnthropicDelta {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicApiError {
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicEvent {
    ContentBlockStart {
        content_block: AnthropicContentBlock,
    },
    ContentBlockDelta {
        delta: AnthropicDelta,
    },
    ContentBlockStop,
    MessageStop,
    Error {
        error: AnthropicApiError,
    },
    #[serde(other)]
    Other,
}

/// Tool call en construcción durante el streaming (entre
/// `content_block_start` y `content_block_stop`).
struct PartialToolUse {
    id: String,
    name: String,
    input_json: String,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
    ) -> Result<(), LlmError> {
        let api_key = std::env::var(&self.api_key_env)
            .map_err(|_| LlmError::MissingApiKey(self.api_key_env.clone()))?;

        // El system va como array de bloques: el primero (prompt base +
        // memorias) es estable entre turnos y se marca como breakpoint de
        // prompt caching; los siguientes (fecha/hora) cambian por turno y
        // quedan fuera del prefijo cacheado. Si el prefijo no llega al
        // mínimo cacheable, la API ignora el marcador sin error.
        let system_blocks: Vec<Value> = history
            .iter()
            .filter(|m| m.role == Role::System && !m.content.is_empty())
            .enumerate()
            .map(|(i, m)| {
                if i == 0 {
                    json!({
                        "type": "text",
                        "text": m.content,
                        "cache_control": { "type": "ephemeral" },
                    })
                } else {
                    json!({ "type": "text", "text": m.content })
                }
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": build_messages(history),
            "stream": true,
        });
        if !system_blocks.is_empty() {
            body["system"] = json!(system_blocks);
        }
        if !tools.is_empty() {
            let mut specs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            if let Some(last) = specs.last_mut() {
                // Los tools van antes del system en el prefijo cacheable:
                // este breakpoint cachea las definiciones de herramientas.
                last["cache_control"] = json!({ "type": "ephemeral" });
            }
            body["tools"] = json!(specs);
        }

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
        let mut decoder = Utf8StreamDecoder::new();
        let mut current_tool: Option<PartialToolUse> = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(LlmError::Network)?;
            decoder.feed(&chunk, &mut buffer);

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
                    AnthropicEvent::ContentBlockStart { content_block } => {
                        if content_block.kind == "tool_use" {
                            current_tool = Some(PartialToolUse {
                                id: content_block.id.unwrap_or_default(),
                                name: content_block.name.unwrap_or_default(),
                                input_json: String::new(),
                            });
                        }
                    }
                    AnthropicEvent::ContentBlockDelta { delta } => {
                        if let Some(text) = delta.text {
                            if !text.is_empty()
                                && tx.send(Ok(LlmEvent::TextDelta(text))).await.is_err()
                            {
                                return Ok(());
                            }
                        }
                        if let Some(partial) = delta.partial_json {
                            if let Some(tool) = current_tool.as_mut() {
                                tool.input_json.push_str(&partial);
                            }
                        }
                    }
                    AnthropicEvent::ContentBlockStop => {
                        if let Some(tool) = current_tool.take() {
                            let arguments: Value = if tool.input_json.trim().is_empty() {
                                json!({})
                            } else {
                                match serde_json::from_str(&tool.input_json) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        let message = format!(
                                            "input de tool_use no parseable ({}): {e}",
                                            tool.name
                                        );
                                        let _ = tx
                                            .send(Err(LlmError::UnexpectedResponse(
                                                message.clone(),
                                            )))
                                            .await;
                                        return Err(LlmError::UnexpectedResponse(message));
                                    }
                                }
                            };
                            let request = ToolCallRequest {
                                id: tool.id,
                                name: tool.name,
                                arguments,
                            };
                            if tx.send(Ok(LlmEvent::ToolCall(request))).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    AnthropicEvent::MessageStop => {
                        let _ = tx.send(Ok(LlmEvent::Done)).await;
                        return Ok(());
                    }
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

        let _ = tx.send(Ok(LlmEvent::Done)).await;
        Ok(())
    }
}
