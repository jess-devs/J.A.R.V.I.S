//! Cliente compartido para APIs compatibles con el formato de OpenAI
//! (`POST {base_url}/chat/completions`, streaming SSE con `data: {...}` y
//! terminador `data: [DONE]`). Lo usan tanto `OpenAiProvider` como
//! `DeepSeekProvider` (la API de DeepSeek es explícitamente compatible con
//! el formato de OpenAI, solo cambia `base_url`/modelo/API key).

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::errors::LlmError;

use super::{ChatMessage, LlmProvider, Role};

pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key_env: String,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key_env: impl Into<String>,
        request_timeout_secs: u64,
    ) -> Result<Self, LlmError> {
        let api_key_env = api_key_env.into();
        if std::env::var(&api_key_env).is_err() {
            return Err(LlmError::MissingApiKey(api_key_env));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(request_timeout_secs))
            .build()
            .expect("configuración de cliente reqwest válida");
        Ok(Self {
            client,
            base_url: base_url.into(),
            model: model.into(),
            api_key_env,
        })
    }
}

#[derive(Serialize)]
struct ChatCompletionMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatCompletionMessage<'a>>,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatCompletionDelta {
    #[serde(default)]
    content: Option<String>,
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
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tx: mpsc::Sender<Result<String, LlmError>>,
    ) -> Result<(), LlmError> {
        let api_key = std::env::var(&self.api_key_env)
            .map_err(|_| LlmError::MissingApiKey(self.api_key_env.clone()))?;

        let messages: Vec<ChatCompletionMessage> = history
            .iter()
            .map(|m| ChatCompletionMessage {
                role: role_str(m.role),
                content: &m.content,
            })
            .collect();

        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatCompletionRequest {
            model: &self.model,
            messages,
            stream: true,
        };

        let response = self
            .client
            .post(&url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(LlmError::Network)?;

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
                if data == "[DONE]" {
                    return Ok(());
                }

                let parsed: ChatCompletionChunk = match serde_json::from_str(data) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                for choice in parsed.choices {
                    if let Some(content) = choice.delta.content {
                        if !content.is_empty() && tx.send(Ok(content)).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
