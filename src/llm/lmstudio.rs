//! Proveedor LLM local vía LM Studio: servidor de inferencia compatible con
//! la API de OpenAI, pensado como alternativa a Ollama cuando este resulta
//! lento. Reutiliza `OpenAiCompatibleProvider` porque el formato de wire es
//! idéntico al de OpenAI/DeepSeek. A diferencia de esos dos, `base_url` y
//! `api_key_env` salen de la config en vez de estar hardcodeados (como
//! Ollama), porque normalmente no hace falta autenticación y el puerto
//! puede variar.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::LmStudioConfig;
use crate::errors::LlmError;

use super::openai_compatible::OpenAiCompatibleProvider;
use super::{ChatMessage, LlmEvent, LlmProvider, ToolSpec};

pub struct LmStudioProvider(OpenAiCompatibleProvider);

impl LmStudioProvider {
    pub fn new(config: &LmStudioConfig, request_timeout_secs: u64) -> Result<Self, LlmError> {
        Ok(Self(OpenAiCompatibleProvider::new(
            &config.base_url,
            &config.model,
            config.api_key_env.clone(),
            request_timeout_secs,
            true,
        )?))
    }
}

#[async_trait]
impl LlmProvider for LmStudioProvider {
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
    ) -> Result<(), LlmError> {
        self.0.stream_chat(history, tools, tx).await
    }
}
