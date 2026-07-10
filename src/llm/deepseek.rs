//! Proveedor LLM en la nube vía DeepSeek. Su API es explícitamente
//! compatible con el formato de OpenAI (mismo `openai_compatible.rs`), solo
//! cambia la base URL, el modelo y la variable de entorno de la API key.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::DeepSeekConfig;
use crate::errors::LlmError;

use super::openai_compatible::OpenAiCompatibleProvider;
use super::{ChatMessage, LlmEvent, LlmProvider, ToolSpec};

const BASE_URL: &str = "https://api.deepseek.com";

pub struct DeepSeekProvider(OpenAiCompatibleProvider);

impl DeepSeekProvider {
    pub fn new(config: &DeepSeekConfig, request_timeout_secs: u64) -> Result<Self, LlmError> {
        Ok(Self(OpenAiCompatibleProvider::new(
            BASE_URL,
            &config.model,
            &config.api_key_env,
            request_timeout_secs,
        )?))
    }
}

#[async_trait]
impl LlmProvider for DeepSeekProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
    ) -> Result<(), LlmError> {
        self.0.stream_chat(history, tools, tx).await
    }
}
