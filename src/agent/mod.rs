//! Loop agéntico: orquesta las pasadas LLM → herramientas → LLM hasta tener
//! una respuesta final hablada, y modela la pausa por confirmación de voz
//! (`PendingConfirmation`) cuando una herramienta requiere aprobación.

pub mod confirm;
pub mod relevance;
mod turn;

pub use turn::{
    resume_agentic_turn, run_agentic_turn, AgentTurnResult, PendingConfirmation, TurnContext,
};

use std::sync::Arc;

use crate::audio::AudioPlayer;
use crate::tts::TtsProvider;

/// Sintetiza y reproduce una frase fuera del pipeline de streaming (fillers, preguntas de confirmación).
pub async fn speak(tts: &Arc<dyn TtsProvider>, player: &mut AudioPlayer, text: &str) {
    match tts.synthesize(text).await {
        Ok(chunk) => {
            if let Err(e) = player.play_chunk(&chunk).await {
                tracing::warn!(error = %e, "no se pudo reproducir la frase");
                return;
            }
            if let Err(e) = player.wait_until_drained().await {
                tracing::warn!(error = %e, "la reproducción no terminó a tiempo");
            }
        }
        Err(e) => tracing::warn!(error = %e, text, "no se pudo sintetizar la frase"),
    }
}
