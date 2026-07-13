//! Proveedor LLM en la nube vía OpenAI (GPT). La API es compatible con el
//! formato compartido en `openai_compatible.rs`.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::OpenAiConfig;
use crate::errors::LlmError;

use super::openai_compatible::OpenAiCompatibleProvider;
use super::{ChatMessage, LlmEvent, LlmProvider, ToolSpec};

const BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiProvider(OpenAiCompatibleProvider);

impl OpenAiProvider {
    pub fn new(config: &OpenAiConfig, request_timeout_secs: u64) -> Result<Self, LlmError> {
        Ok(Self(OpenAiCompatibleProvider::new(
            BASE_URL,
            &config.model,
            Some(config.api_key_env.clone()),
            request_timeout_secs,
            true,
        )?))
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
    ) -> Result<(), LlmError> {
        self.0.stream_chat(history, tools, tx).await
    }
}
