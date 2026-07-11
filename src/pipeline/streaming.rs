//! El núcleo del diseño: LLM emite tokens → se trocean en frases → se
//! sintetiza cada frase → se reproduce en orden — todo como tareas tokio
//! encadenadas por channels, para que la síntesis de la frase N+1 pueda
//! avanzar mientras la N está sonando.
//!
//! El orden de reproducción queda garantizado sin necesitar un buffer de
//! reordenamiento: cada salto de la cadena (token→frase→audio) tiene un solo
//! productor y un solo consumidor, y `synth_task` sintetiza las frases
//! estrictamente una por vez (el worker Piper es un único proceso
//! secuencial), así que el orden de llegada a `audio_tx` es siempre el orden
//! del texto original.
//!
//! Con herramientas: todo el texto que emite el modelo se habla siempre
//! (incluido un eventual preámbulo antes de un tool call), y los tool calls
//! se acumulan y devuelven en `TurnOutput` para que el loop agéntico los
//! ejecute — este módulo no ejecuta nada.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::audio::AudioPlayer;
use crate::config::PipelineConfig;
use crate::errors::{JarvisError, LlmError};
use crate::llm::{ChatMessage, LlmEvent, LlmProvider, ToolCallRequest, ToolSpec};
use crate::text::SentenceChunker;
use crate::tts::{AudioChunk, TtsProvider};

/// Resultado de una pasada por el LLM: lo que se habló y lo que pidió ejecutar.
#[derive(Debug)]
pub struct TurnOutput {
    pub spoken_text: String,
    pub tool_calls: Vec<ToolCallRequest>,
}

pub async fn run_speaking_turn(
    llm: Arc<dyn LlmProvider>,
    tts: Arc<dyn TtsProvider>,
    player: &mut AudioPlayer,
    history: &[ChatMessage],
    tools: Arc<Vec<ToolSpec>>,
    cfg: &PipelineConfig,
) -> Result<TurnOutput, JarvisError> {
    let (event_tx, mut event_rx) = mpsc::channel::<Result<LlmEvent, LlmError>>(32);
    let (phrase_tx, mut phrase_rx) = mpsc::channel::<String>(8);
    let (audio_tx, mut audio_rx) = mpsc::channel::<AudioChunk>(4);

    let llm_task = tokio::spawn({
        let llm = llm.clone();
        let history = history.to_vec();
        async move { llm.stream_chat(&history, &tools, event_tx).await }
    });

    let max_phrase_chars = cfg.max_phrase_chars;
    let min_phrase_chars = cfg.min_phrase_chars;
    let chunker_task = tokio::spawn(async move {
        let mut chunker = SentenceChunker::new(max_phrase_chars, min_phrase_chars);
        let mut full = String::new();
        let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
        'outer: while let Some(event) = event_rx.recv().await {
            match event? {
                LlmEvent::TextDelta(token) => {
                    full.push_str(&token);
                    for phrase in chunker.push(&token) {
                        if phrase_tx.send(phrase).await.is_err() {
                            break 'outer;
                        }
                    }
                }
                LlmEvent::ToolCall(call) => tool_calls.push(call),
                LlmEvent::Done => break,
            }
        }
        if let Some(last) = chunker.finish() {
            let _ = phrase_tx.send(last).await;
        }
        Ok::<(String, Vec<ToolCallRequest>), LlmError>((full, tool_calls))
    });

    let synth_task = tokio::spawn(async move {
        while let Some(phrase) = phrase_rx.recv().await {
            let phrase = crate::text::strip_markdown_for_speech(&phrase);
            if phrase.trim().is_empty() {
                continue;
            }
            match tts.synthesize(&phrase).await {
                Ok(chunk) => {
                    if audio_tx.send(chunk).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "síntesis fallida, se aborta el resto de la respuesta");
                    break;
                }
            }
        }
    });

    while let Some(chunk) = audio_rx.recv().await {
        player.play_chunk(&chunk).await?;
    }
    player.wait_until_drained().await?;

    let (spoken_text, tool_calls) = chunker_task
        .await
        .map_err(|e| JarvisError::Pipeline(format!("tarea de troceo de frases falló: {e}")))??;
    llm_task
        .await
        .map_err(|e| JarvisError::Pipeline(format!("tarea de streaming del LLM falló: {e}")))??;
    synth_task
        .await
        .map_err(|e| JarvisError::Pipeline(format!("tarea de síntesis falló: {e}")))?;

    Ok(TurnOutput {
        spoken_text,
        tool_calls,
    })
}
