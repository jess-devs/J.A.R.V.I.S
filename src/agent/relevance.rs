//! Chequeo de relevancia para barge-in en modo `any_voice`: antes de cortar
//! a Jarvis de verdad, le pregunta al LLM configurado si lo que captó el
//! micrófono tiene sentido como algo dirigido a él, en una llamada mínima y
//! aparte del historial real de la conversación (se descarta después).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::confirm::{self, ConfirmDecision};
use crate::config::AgentConfig;
use crate::errors::LlmError;
use crate::llm::{ChatMessage, LlmEvent, LlmProvider};

const SYSTEM_PROMPT: &str = "Sos un clasificador. Te paso lo que un asistente de voz \
    llamado Jarvis venía diciendo y una frase que captó el micrófono mientras hablaba. \
    Tu única tarea es decidir si esa frase está dirigida a Jarvis (una orden, pregunta, \
    interrupción o continuación de lo que él decía) o si es parte de una conversación \
    con otra persona, sin relación con Jarvis. Respondé ÚNICAMENTE con una palabra: \
    \"sí\" o \"no\". Nada más.";

/// true si `interrupting_text` tiene sentido como algo dirigido a Jarvis
/// mientras decía `was_saying`. Si la consulta falla o tarda más que
/// `timeout`, devuelve `false` (no dirigido a él): Jarvis sigue hablando en
/// vez de cortarse a ciegas por una consulta que no llegó a responder.
pub async fn sounds_directed_at_jarvis(
    llm: &Arc<dyn LlmProvider>,
    was_saying: &str,
    interrupting_text: &str,
    agent_cfg: &AgentConfig,
    timeout: Duration,
) -> bool {
    let was_saying = if was_saying.trim().is_empty() {
        "(no se registró texto reciente)"
    } else {
        was_saying
    };
    let history = vec![
        ChatMessage::system(SYSTEM_PROMPT),
        ChatMessage::user(format!(
            "Jarvis venía diciendo: «{was_saying}»\n\
             Se escuchó por el micrófono: «{interrupting_text}»\n\
             ¿Está dirigido a Jarvis? Respondé sí o no."
        )),
    ];

    match tokio::time::timeout(timeout, collect_reply(llm.clone(), history)).await {
        Ok(Ok(reply)) => matches!(confirm::interpret(&reply, agent_cfg), ConfirmDecision::Yes),
        Ok(Err(error)) => {
            tracing::warn!(%error, "chequeo de relevancia de barge-in falló; se asume que no era para Jarvis");
            false
        }
        Err(_) => {
            tracing::warn!(
                timeout_secs = timeout.as_secs(),
                "chequeo de relevancia de barge-in no respondió a tiempo; se asume que no era para Jarvis"
            );
            false
        }
    }
}

/// Corre `stream_chat` en una tarea aparte y junta los `TextDelta` en un
/// string. Mismo patrón que `llm_task` en `pipeline::streaming`.
async fn collect_reply(llm: Arc<dyn LlmProvider>, history: Vec<ChatMessage>) -> Result<String, LlmError> {
    let (tx, mut rx) = mpsc::channel(8);
    let task = tokio::spawn(async move { llm.stream_chat(&history, &[], tx).await });

    let mut reply = String::new();
    while let Some(event) = rx.recv().await {
        match event? {
            LlmEvent::TextDelta(token) => reply.push_str(&token),
            LlmEvent::ToolCall(_) => {}
            LlmEvent::Done => break,
        }
    }
    task.await
        .map_err(|e| LlmError::UnexpectedResponse(format!("tarea de relevancia falló: {e}")))??;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::llm::ToolSpec;

    struct FakeLlm {
        reply: &'static str,
        delay: Duration,
    }

    #[async_trait]
    impl LlmProvider for FakeLlm {
        async fn stream_chat(
            &self,
            _history: &[ChatMessage],
            _tools: &[ToolSpec],
            tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
        ) -> Result<(), LlmError> {
            tokio::time::sleep(self.delay).await;
            let _ = tx.send(Ok(LlmEvent::TextDelta(self.reply.to_string()))).await;
            let _ = tx.send(Ok(LlmEvent::Done)).await;
            Ok(())
        }
    }

    fn cfg() -> AgentConfig {
        AgentConfig::default()
    }

    #[tokio::test]
    async fn respuesta_si_es_relevante() {
        let llm: Arc<dyn LlmProvider> = Arc::new(FakeLlm {
            reply: "sí",
            delay: Duration::from_millis(0),
        });
        assert!(
            sounds_directed_at_jarvis(&llm, "el clima de hoy es soleado", "para, jarvis", &cfg(), Duration::from_secs(1))
                .await
        );
    }

    #[tokio::test]
    async fn respuesta_no_no_es_relevante() {
        let llm: Arc<dyn LlmProvider> = Arc::new(FakeLlm {
            reply: "no, es una conversación con otra persona",
            delay: Duration::from_millis(0),
        });
        assert!(
            !sounds_directed_at_jarvis(&llm, "el clima de hoy es soleado", "y entonces le dije que no", &cfg(), Duration::from_secs(1))
                .await
        );
    }

    #[tokio::test]
    async fn timeout_no_es_relevante() {
        let llm: Arc<dyn LlmProvider> = Arc::new(FakeLlm {
            reply: "sí",
            delay: Duration::from_millis(200),
        });
        assert!(
            !sounds_directed_at_jarvis(&llm, "el clima de hoy es soleado", "para, jarvis", &cfg(), Duration::from_millis(20))
                .await
        );
    }
}
