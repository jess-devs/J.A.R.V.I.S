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

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::audio::AudioPlayer;
use crate::config::PipelineConfig;
use crate::errors::{JarvisError, LlmError};
use crate::llm::{ChatMessage, LlmProvider};
use crate::text::SentenceChunker;
use crate::tts::{AudioChunk, TtsProvider};

pub async fn run_streaming_response(
    llm: Arc<dyn LlmProvider>,
    tts: Arc<dyn TtsProvider>,
    player: &mut AudioPlayer,
    history: &[ChatMessage],
    cfg: &PipelineConfig,
) -> Result<String, JarvisError> {
    let (token_tx, mut token_rx) = mpsc::channel::<Result<String, LlmError>>(32);
    let (phrase_tx, mut phrase_rx) = mpsc::channel::<String>(8);
    let (audio_tx, mut audio_rx) = mpsc::channel::<AudioChunk>(4);

    let llm_task = tokio::spawn({
        let llm = llm.clone();
        let history = history.to_vec();
        async move { llm.stream_chat(&history, token_tx).await }
    });

    let max_phrase_chars = cfg.max_phrase_chars;
    let min_phrase_chars = cfg.min_phrase_chars;
    let chunker_task = tokio::spawn(async move {
        let mut chunker = SentenceChunker::new(max_phrase_chars, min_phrase_chars);
        let mut full = String::new();
        'outer: while let Some(token) = token_rx.recv().await {
            let token = token?;
            full.push_str(&token);
            for phrase in chunker.push(&token) {
                if phrase_tx.send(phrase).await.is_err() {
                    break 'outer;
                }
            }
        }
        if let Some(last) = chunker.finish() {
            let _ = phrase_tx.send(last).await;
        }
        Ok::<String, LlmError>(full)
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
    player.wait_until_drained().await;

    let full_reply = chunker_task
        .await
        .map_err(|e| JarvisError::Pipeline(format!("tarea de troceo de frases falló: {e}")))??;
    llm_task
        .await
        .map_err(|e| JarvisError::Pipeline(format!("tarea de streaming del LLM falló: {e}")))??;
    synth_task
        .await
        .map_err(|e| JarvisError::Pipeline(format!("tarea de síntesis falló: {e}")))?;

    Ok(full_reply)
}
