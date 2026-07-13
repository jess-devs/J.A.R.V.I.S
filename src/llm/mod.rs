//! Interfaz común de LLM: `OllamaProvider`/`LmStudioProvider` (locales) y
//! `AnthropicProvider`/`OpenAiProvider`/`DeepSeekProvider` (nube).
//!
//! El streaming se modela como eventos (`LlmEvent`): texto incremental para
//! el TTS y tool calls completos para el loop agéntico. Cada adapter de
//! provider es responsable de acumular los fragmentos de tool calls de su
//! protocolo — el consumidor solo ve `ToolCall` ya completos y parseados.

pub mod anthropic;
mod decode;
pub mod deepseek;
pub mod lmstudio;
pub mod model_select;
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
    /// Resultado de una herramienta ejecutada, en respuesta a un tool call
    /// del assistant. Siempre lleva `tool_call_id`.
    Tool,
}

/// Un tool call completo pedido por el modelo. Los providers que no dan id
/// (Ollama) lo generan como "call_{n}".
#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    /// Objeto JSON completo, ya parseado.
    pub arguments: serde_json::Value,
}

/// Imagen adjunta a un mensaje (típicamente un resultado de `take_screenshot`).
/// Solo lo renderizan Anthropic, DeepSeek y los proveedores locales (Ollama,
/// LM Studio); el resto de los proveedores simplemente ignoran este campo.
#[derive(Debug, Clone)]
pub struct ImageBlock {
    /// MIME type, ej. "image/png".
    pub media_type: String,
    /// Datos de la imagen codificados en base64 (sin el prefijo data:...).
    pub base64_data: String,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Solo en mensajes Assistant que pidieron herramientas.
    pub tool_calls: Vec<ToolCallRequest>,
    /// Solo en mensajes Tool: el id del call al que responde.
    pub tool_call_id: Option<String>,
    /// Solo en mensajes Tool: el nombre de la herramienta (Ollama lo usa en
    /// lugar de ids).
    pub tool_name: Option<String>,
    /// Imágenes adjuntas (ej. un screenshot devuelto por una tool). Vacío en
    /// la inmensa mayoría de los mensajes.
    pub images: Vec<ImageBlock>,
}

impl ChatMessage {
    fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            images: Vec::new(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }

    pub fn assistant_with_tools(content: impl Into<String>, calls: Vec<ToolCallRequest>) -> Self {
        Self {
            tool_calls: calls,
            ..Self::new(Role::Assistant, content)
        }
    }

    pub fn tool_result(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            tool_name: Some(tool_name.into()),
            ..Self::new(Role::Tool, content)
        }
    }

    pub fn tool_result_with_images(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
        images: Vec<ImageBlock>,
    ) -> Self {
        Self {
            images,
            ..Self::tool_result(call_id, tool_name, content)
        }
    }
}

/// Eventos del stream de una respuesta del LLM.
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// Fragmento de texto de la respuesta hablada.
    TextDelta(String),
    /// Tool call COMPLETO (nombre + argumentos ya parseados).
    ToolCall(ToolCallRequest),
    /// El modelo terminó el turno.
    Done,
}

/// Definición de una herramienta en formato neutro; cada provider la mapea
/// a su propio esquema de request.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema del objeto de parámetros (`{"type": "object", ...}`).
    pub parameters: serde_json::Value,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Genera una respuesta en streaming: cada evento se envía por `tx` a
    /// medida que llega del modelo. Con `tools` vacío se comporta como un
    /// chat de texto plano. Retorna cuando terminó de generar toda la
    /// respuesta (o si `tx` fue cerrado del otro lado).
    async fn stream_chat(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
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
        LlmProviderKind::LmStudio => Ok(Arc::new(lmstudio::LmStudioProvider::new(
            &config.llm.lmstudio,
            timeout,
        )?)),
    }
}
