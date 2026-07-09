//! Interfaz común de LLM: `OllamaProvider` (local, foco principal) y
//! `AnthropicProvider`/`OpenAiProvider`/`DeepSeekProvider` (nube).

pub mod anthropic;
pub mod deepseek;
pub mod ollama;
pub mod openai;
mod openai_compatible;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::{Config, LlmProviderKind};
use crate::errors::LlmError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Genera una respuesta en streaming: cada fragmento de texto se envía
    /// por `tx` a medida que llega del modelo. Retorna cuando terminó de
    /// generar toda la respuesta (o si `tx` fue cerrado del otro lado).
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tx: mpsc::Sender<Result<String, LlmError>>,
    ) -> Result<(), LlmError>;
}

pub fn build_provider(config: &Config) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let timeout = config.llm.request_timeout_secs;
    match config.llm.provider {
        LlmProviderKind::Ollama => Ok(Arc::new(ollama::OllamaProvider::new(
            &config.llm.ollama,
            timeout,
        ))),
        LlmProviderKind::Anthropic => Ok(Arc::new(anthropic::AnthropicProvider::new(
            &config.llm.anthropic,
            timeout,
        )?)),
        LlmProviderKind::Openai => Ok(Arc::new(openai::OpenAiProvider::new(
            &config.llm.openai,
            timeout,
        )?)),
        LlmProviderKind::Deepseek => Ok(Arc::new(deepseek::DeepSeekProvider::new(
            &config.llm.deepseek,
            timeout,
        )?)),
    }
}
