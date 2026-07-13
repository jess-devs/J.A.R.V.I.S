//! Cliente compartido para APIs compatibles con el formato de OpenAI
//! (`POST {base_url}/chat/completions`, streaming SSE con `data: {...}` y
//! terminador `data: [DONE]`). Lo usan `OpenAiProvider`, `DeepSeekProvider`
//! (API explícitamente compatible con el formato de OpenAI) y
//! `LmStudioProvider` (servidor local, normalmente sin autenticación — por
//! eso `api_key_env` es `Option<String>`, no `String`).
//!
//! Tool calling: en este protocolo los tool calls llegan fragmentados en
//! deltas (`choices[].delta.tool_calls[]` con `index`), con los `arguments`
//! como string JSON parcial. Se acumulan por `index` y se emiten como
//! `LlmEvent::ToolCall` completos recién al cierre del stream.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::errors::LlmError;

use super::decode::Utf8StreamDecoder;
use super::{ChatMessage, ImageBlock, LlmEvent, LlmProvider, Role, ToolCallRequest, ToolSpec};

pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key_env: Option<String>,
    /// Si el backend real detrás de esta base_url acepta bloques `image_url`
    /// en el content. DeepSeek comparte el formato de wire pero su API de
    /// chat es texto-puro y rechaza ese campo con un 400.
    supports_vision: bool,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key_env: Option<String>,
        request_timeout_secs: u64,
        supports_vision: bool,
    ) -> Result<Self, LlmError> {
        if let Some(env) = &api_key_env {
            if std::env::var(env).is_err() {
                return Err(LlmError::MissingApiKey(env.clone()));
            }
        }
        let client = crate::http::client(Duration::from_secs(request_timeout_secs));
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key_env,
            supports_vision,
        })
    }
}

#[derive(Serialize)]
struct RequestFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct RequestToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: RequestFunctionCall,
}

#[derive(Serialize)]
struct ChatCompletionMessage<'a> {
    role: &'a str,
    content: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<RequestToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

/// Bloque `image_url` en formato OpenAI (data URI inline). Solo se manda si
/// el provider (`supports_vision`) lo acepta — DeepSeek comparte este
/// cliente pero su API no soporta imágenes, así que ahí se degrada a texto
/// con una nota, en vez de mandar un content que el backend rechaza.
fn image_content(content: &str, images: &[ImageBlock], supports_vision: bool) -> serde_json::Value {
    if images.is_empty() {
        return serde_json::Value::String(content.to_string());
    }
    if !supports_vision {
        return serde_json::Value::String(format!(
            "{content} (nota: este proveedor de LLM no admite imágenes; no fue posible \
             analizar el contenido visual adjunto.)"
        ));
    }
    let mut blocks = vec![serde_json::json!({ "type": "text", "text": content })];
    blocks.extend(images.iter().map(|img| {
        serde_json::json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{};base64,{}", img.media_type, img.base64_data) }
        })
    }));
    serde_json::Value::Array(blocks)
}

#[derive(Serialize)]
struct RequestToolFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
}

#[derive(Serialize)]
struct RequestTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: RequestToolFunction<'a>,
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatCompletionMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<RequestTool<'a>>,
}

#[derive(Deserialize)]
struct DeltaFunctionCall {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct DeltaToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<DeltaFunctionCall>,
}

#[derive(Deserialize)]
struct ChatCompletionDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<DeltaToolCall>,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    delta: ChatCompletionDelta,
}

#[derive(Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<ChatCompletionChoice>,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// Acumula los fragmentos de un tool call streameado por `index`.
#[derive(Default)]
struct PartialCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

fn flush_partial_calls(
    partials: BTreeMap<u32, PartialCall>,
) -> Vec<Result<ToolCallRequest, LlmError>> {
    partials
        .into_values()
        .map(|p| {
            let arguments: serde_json::Value = if p.arguments.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&p.arguments).map_err(|e| {
                    LlmError::UnexpectedResponse(format!(
                        "arguments de tool call no parseables ({}): {e}",
                        p.name
                    ))
                })?
            };
            Ok(ToolCallRequest {
                id: p.id.unwrap_or_else(|| format!("call_{}", p.name)),
                name: p.name,
                arguments,
            })
        })
        .collect()
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
    ) -> Result<(), LlmError> {
        let api_key = match &self.api_key_env {
            Some(env) => {
                Some(std::env::var(env).map_err(|_| LlmError::MissingApiKey(env.clone()))?)
            }
            None => None,
        };

        let messages: Vec<ChatCompletionMessage> = history
            .iter()
            .map(|m| ChatCompletionMessage {
                role: role_str(m.role),
                content: image_content(&m.content, &m.images, self.supports_vision),
                tool_calls: m
                    .tool_calls
                    .iter()
                    .map(|c| RequestToolCall {
                        id: c.id.clone(),
                        kind: "function",
                        function: RequestFunctionCall {
                            name: c.name.clone(),
                            arguments: c.arguments.to_string(),
                        },
                    })
                    .collect(),
                tool_call_id: m.tool_call_id.as_deref(),
            })
            .collect();

        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatCompletionRequest {
            model: &self.model,
            messages,
            stream: true,
            tools: tools
                .iter()
                .map(|t| RequestTool {
                    kind: "function",
                    function: RequestToolFunction {
                        name: &t.name,
                        description: &t.description,
                        parameters: &t.parameters,
                    },
                })
                .collect(),
        };

        let mut request = self.client.post(&url).json(&body);
        if let Some(api_key) = api_key {
            request = request.bearer_auth(api_key);
        }
        let response = request.send().await.map_err(LlmError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let message = format!("{url} respondió {status}: {text}");
            let _ = tx
                .send(Err(LlmError::UnexpectedResponse(message.clone())))
                .await;
            return Err(LlmError::UnexpectedResponse(message));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut decoder = Utf8StreamDecoder::new();
        let mut partial_calls: BTreeMap<u32, PartialCall> = BTreeMap::new();

        async fn finish(
            partials: BTreeMap<u32, PartialCall>,
            tx: &mpsc::Sender<Result<LlmEvent, LlmError>>,
        ) {
            for call in flush_partial_calls(partials) {
                let event = call.map(LlmEvent::ToolCall);
                if tx.send(event).await.is_err() {
                    return;
                }
            }
            let _ = tx.send(Ok(LlmEvent::Done)).await;
        }

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
                if data == "[DONE]" {
                    finish(partial_calls, &tx).await;
                    return Ok(());
                }

                let parsed: ChatCompletionChunk = match serde_json::from_str(data) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                for choice in parsed.choices {
                    if let Some(content) = choice.delta.content {
                        if !content.is_empty()
                            && tx.send(Ok(LlmEvent::TextDelta(content))).await.is_err()
                        {
                            return Ok(());
                        }
                    }
                    for delta in choice.delta.tool_calls {
                        let partial = partial_calls.entry(delta.index).or_default();
                        if let Some(id) = delta.id {
                            partial.id = Some(id);
                        }
                        if let Some(function) = delta.function {
                            if let Some(name) = function.name {
                                partial.name.push_str(&name);
                            }
                            if let Some(arguments) = function.arguments {
                                partial.arguments.push_str(&arguments);
                            }
                        }
                    }
                }
            }
        }

        finish(partial_calls, &tx).await;
        Ok(())
    }
}
