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

use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::audio::AudioPlayer;
use crate::config::PipelineConfig;
use crate::echo_gate::EchoGate;
use crate::errors::{JarvisError, LlmError};
use crate::llm::{ChatMessage, LlmEvent, LlmProvider, ToolCallRequest, ToolSpec};
use crate::text::SentenceChunker;
use crate::tts::{AudioChunk, TtsProvider};

/// Resultado de una pasada por el LLM: lo que se habló y lo que pidió ejecutar.
#[derive(Debug)]
pub struct TurnOutput {
    pub spoken_text: String,
    pub tool_calls: Vec<ToolCallRequest>,
    /// true si `cancel` se disparó antes de terminar de hablar. En ese caso
    /// `spoken_text` es solo lo que alcanzó a sonar (no la respuesta
    /// completa del LLM) y `tool_calls` siempre viene vacío.
    pub interrupted: bool,
}

pub async fn run_speaking_turn(
    llm: Arc<dyn LlmProvider>,
    tts: Arc<dyn TtsProvider>,
    player: &mut AudioPlayer,
    history: &[ChatMessage],
    tools: Arc<Vec<ToolSpec>>,
    cfg: &PipelineConfig,
    cancel: CancellationToken,
    echo_gate: Arc<Mutex<EchoGate>>,
    mut pause_rx: watch::Receiver<bool>,
) -> Result<TurnOutput, JarvisError> {
    let turn_start = Instant::now();
    let (event_tx, mut event_rx) = mpsc::channel::<Result<LlmEvent, LlmError>>(32);
    let (phrase_tx, mut phrase_rx) = mpsc::channel::<String>(8);
    let (audio_tx, mut audio_rx) = mpsc::channel::<(String, AudioChunk)>(4);

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
                    if audio_tx.send((phrase, chunk)).await.is_err() {
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

    // Cada iteración vigila `cancel` dos veces: mientras espera la próxima
    // frase sintetizada, y de nuevo mientras esa frase suena — así el corte
    // es rápido tanto si la interrupción llega entre frases como a mitad de
    // una. `player.play_chunk` es seguro de soltar a mitad de poll: lo que
    // ya se empujó al ring buffer queda ahí, y `player.stop()` lo descarta.
    //
    // `pause_rx` es un escalón intermedio antes de cancelar de verdad (ver
    // barge-in en `orchestrator.rs`): mientras está en `true`, este loop deja
    // de sacar frases nuevas de `audio_rx` (sin tocar `player` ni abortar las
    // tareas), así que lo que ya sonaba termina de reproducirse con
    // normalidad y el resto queda en pausa por backpressure. Si se reanuda
    // (`false`), el turno sigue exactamente donde había quedado, sin perder
    // nada; si en cambio se confirma la interrupción, `cancel` corta igual
    // que siempre, pausado o no.
    let mut spoken_phrases: Vec<String> = Vec::new();
    let mut interrupted = false;
    let mut first_audio_at: Option<Instant> = None;
    // Si el emisor de `pause_rx` se dropea (nunca pasa en el uso real: vive
    // todo el turno en `orchestrator.rs`), el canal queda "cerrado" y
    // `changed()` resuelve de inmediato para siempre — sin este guard, la
    // rama ganaría el `select!` en cada vuelta y el loop no volvería a
    // revisar `audio_rx` nunca más (busy-loop). Una vez cerrado, se deja de
    // escuchar esa rama: el valor de `paused` queda fijo en lo último visto.
    let mut pause_channel_closed = false;
    loop {
        let paused = *pause_rx.borrow();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                interrupted = true;
                break;
            }
            changed = pause_rx.changed(), if !pause_channel_closed => {
                if changed.is_err() {
                    pause_channel_closed = true;
                }
                // Solo hace falta despertar el loop para releer `paused`.
                continue;
            }
            maybe_chunk = audio_rx.recv(), if !paused => {
                match maybe_chunk {
                    Some((phrase, chunk)) => {
                        first_audio_at.get_or_insert_with(Instant::now);
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                interrupted = true;
                            }
                            result = player.play_chunk(&chunk) => {
                                result?;
                                if let Ok(mut eg) = echo_gate.lock() {
                                    eg.note_spoken(&phrase);
                                }
                                spoken_phrases.push(phrase);
                            }
                        }
                        if interrupted {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    if interrupted {
        player.stop();
        llm_task.abort();
        chunker_task.abort();
        synth_task.abort();
        return Ok(TurnOutput {
            spoken_text: spoken_phrases.join(" "),
            tool_calls: Vec::new(),
            interrupted: true,
        });
    }

    player.wait_until_drained().await?;

    if let Some(first_audio_at) = first_audio_at {
        tracing::info!(
            "Jarvis respondió en {:.2}s, audio reproducido en {:.2}s",
            (first_audio_at - turn_start).as_secs_f64(),
            first_audio_at.elapsed().as_secs_f64()
        );
    }

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
        interrupted: false,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use async_trait::async_trait;

    use super::*;
    use crate::audio::AudioPlayer;
    use crate::config::{EchoGuardConfig, PipelineConfig};
    use crate::errors::{LlmError, TtsError};
    use crate::llm::LlmEvent;

    /// Emite frases cada 100ms; sin cancelar tardaría 5s en terminar. Prueba
    /// que `run_speaking_turn` corta mucho antes que eso.
    struct SlowFakeLlm;

    #[async_trait]
    impl LlmProvider for SlowFakeLlm {
        async fn stream_chat(
            &self,
            _history: &[ChatMessage],
            _tools: &[ToolSpec],
            tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
        ) -> Result<(), LlmError> {
            for _ in 0..50 {
                if tx
                    .send(Ok(LlmEvent::TextDelta(
                        "hola caluroso saludo. ".to_string(),
                    )))
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let _ = tx.send(Ok(LlmEvent::Done)).await;
            Ok(())
        }
    }

    /// Devuelve 200ms de silencio s16le sin tocar ningún worker externo.
    struct FakeTts;

    #[async_trait]
    impl TtsProvider for FakeTts {
        async fn synthesize(&self, _text: &str) -> Result<AudioChunk, TtsError> {
            let sample_rate = 16_000u32;
            let pcm = vec![0u8; (sample_rate as usize / 5) * 2];
            Ok(AudioChunk {
                pcm,
                sample_rate,
                channels: 1,
                sample_width: 2,
            })
        }
    }

    #[tokio::test]
    async fn cancelling_mid_turn_returns_quickly_with_partial_text() {
        let mut player = AudioPlayer::new(None, 0.0, 5)
            .expect("esta prueba necesita un dispositivo de salida de audio real");

        let llm: Arc<dyn LlmProvider> = Arc::new(SlowFakeLlm);
        let tts: Arc<dyn TtsProvider> = Arc::new(FakeTts);
        let cancel = CancellationToken::new();
        let cfg = PipelineConfig::default();
        let history = vec![ChatMessage::user("hola")];

        let cancel_trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            cancel_trigger.cancel();
        });

        let echo_gate = Arc::new(Mutex::new(EchoGate::new(EchoGuardConfig::default())));
        let (_pause_tx, pause_rx) = watch::channel(false);

        let start = Instant::now();
        let out = run_speaking_turn(
            llm,
            tts,
            &mut player,
            &history,
            Arc::new(Vec::new()),
            &cfg,
            cancel,
            echo_gate,
            pause_rx,
        )
        .await
        .expect("no debería fallar, solo cancelarse");
        let elapsed = start.elapsed();

        assert!(out.interrupted, "el turno debería reportarse interrumpido");
        assert!(
            elapsed < Duration::from_millis(2000),
            "tardó {elapsed:?} en volver tras cancelar a los 400ms (sin cancelar tardaría 5s) — el corte no fue rápido"
        );
        assert!(
            !out.spoken_text.trim().is_empty(),
            "para esta prueba debería haber alcanzado a sonar al menos una frase antes de cancelar"
        );
    }

    /// Emite un número fijo de frases separadas por `delay`, para controlar
    /// con precisión cuándo pausar/reanudar en los tests de barge-in.
    struct CountedFakeLlm {
        phrases: usize,
        delay: Duration,
    }

    #[async_trait]
    impl LlmProvider for CountedFakeLlm {
        async fn stream_chat(
            &self,
            _history: &[ChatMessage],
            _tools: &[ToolSpec],
            tx: mpsc::Sender<Result<LlmEvent, LlmError>>,
        ) -> Result<(), LlmError> {
            for i in 0..self.phrases {
                if tx
                    .send(Ok(LlmEvent::TextDelta(format!("frase numero {i}. "))))
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                tokio::time::sleep(self.delay).await;
            }
            let _ = tx.send(Ok(LlmEvent::Done)).await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn pausar_y_reanudar_no_pierde_texto() {
        let mut player = AudioPlayer::new(None, 0.0, 5)
            .expect("esta prueba necesita un dispositivo de salida de audio real");

        let llm: Arc<dyn LlmProvider> = Arc::new(CountedFakeLlm {
            phrases: 5,
            delay: Duration::from_millis(60),
        });
        let tts: Arc<dyn TtsProvider> = Arc::new(FakeTts);
        let cancel = CancellationToken::new();
        let cfg = PipelineConfig::default();
        let history = vec![ChatMessage::user("hola")];
        let echo_gate = Arc::new(Mutex::new(EchoGate::new(EchoGuardConfig::default())));

        let (pause_tx, pause_rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            let _ = pause_tx.send(true);
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = pause_tx.send(false);
            // `pause_tx` sigue vivo hasta acá a propósito: en el uso real
            // (orchestrator.rs) vive todo el turno, nunca se dropea a mitad.
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let out = run_speaking_turn(
            llm,
            tts,
            &mut player,
            &history,
            Arc::new(Vec::new()),
            &cfg,
            cancel,
            echo_gate,
            pause_rx,
        )
        .await
        .expect("no debería fallar");

        assert!(
            !out.interrupted,
            "no debería reportarse interrumpido: solo se pausó y se reanudó"
        );
        for i in 0..5 {
            assert!(
                out.spoken_text.contains(&format!("frase numero {i}")),
                "faltó la frase {i} en la respuesta final: {}",
                out.spoken_text
            );
        }
    }

    #[tokio::test]
    async fn cancelar_mientras_esta_pausado_sigue_cortando_rapido() {
        let mut player = AudioPlayer::new(None, 0.0, 5)
            .expect("esta prueba necesita un dispositivo de salida de audio real");

        let llm: Arc<dyn LlmProvider> = Arc::new(SlowFakeLlm);
        let tts: Arc<dyn TtsProvider> = Arc::new(FakeTts);
        let cancel = CancellationToken::new();
        let cfg = PipelineConfig::default();
        let history = vec![ChatMessage::user("hola")];
        let echo_gate = Arc::new(Mutex::new(EchoGate::new(EchoGuardConfig::default())));

        let (pause_tx, pause_rx) = watch::channel(false);
        let cancel_trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = pause_tx.send(true);
            tokio::time::sleep(Duration::from_millis(150)).await;
            cancel_trigger.cancel();
        });

        let start = Instant::now();
        let out = run_speaking_turn(
            llm,
            tts,
            &mut player,
            &history,
            Arc::new(Vec::new()),
            &cfg,
            cancel,
            echo_gate,
            pause_rx,
        )
        .await
        .expect("no debería fallar, solo cancelarse");
        let elapsed = start.elapsed();

        assert!(
            out.interrupted,
            "debería reportarse interrumpido tras cancelar, aunque estuviera pausado"
        );
        assert!(
            elapsed < Duration::from_millis(1000),
            "tardó {elapsed:?} en cortar tras cancelar estando pausado — el corte no fue rápido"
        );
    }
}
