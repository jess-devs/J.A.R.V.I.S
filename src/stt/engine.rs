//! Hilos del motor STT nativo. Dos hilos de trabajo (mismo split que el
//! motor Python anterior, ver `workers/stt_engine.py`):
//!
//!   - `stt-audio`: abre el micrófono, carga el VAD y hace captura + VAD +
//!     aplausos + segmentación. Nunca bloquea en el reconocimiento — así el
//!     VAD (barge-in incluido) sigue respondiendo en tiempo real mientras se
//!     transcribe la frase anterior.
//!   - `stt-transcribe`: carga Whisper y consume los segmentos que cierra
//!     el hilo de arriba para transcribirlos.
//!
//! Los dos hacen su propia carga de modelos/apertura de dispositivo *dentro*
//! del hilo (en vez de construirlos antes y moverlos adentro): `cpal::Stream`
//! (dentro de `AudioCapture`) no es `Send` en Windows a propósito (afinidad
//! de hilo de WASAPI/COM), así que el stream tiene que nacer y vivir en el
//! mismo hilo que lo usa. Cada hilo avisa por un `oneshot` si quedó listo o
//! si falló, y `SttWorker::spawn` espera a los dos con el timeout de
//! `stt_init_timeout_secs`.
//!
//! Un tercer hilo (`stt-watchdog`) vigila que ambos sigan dando señales de
//! vida; si alguno se cuelga, v1 solo lo loguea (no hay forma segura de
//! matar un hilo in-process, a diferencia del proceso Python que esto
//! reemplaza).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::config::{compute_thread_budget, BargeInConfig, SttConfig, SttFiltersConfig};
use crate::errors::SttError;
use crate::stt::asr::Asr;
use crate::stt::capture::AudioCapture;
use crate::stt::clap::ClapDetector;
use crate::stt::events::{SttEvent, SttMode, TranscriptMeta};
use crate::stt::vad::{rms_dbfs, DiscardReason, SegmentEvent, SpeechSegmenter};

pub struct ModeCell(AtomicU8);

impl ModeCell {
    fn new(mode: SttMode) -> Self {
        Self(AtomicU8::new(mode as u8))
    }

    pub fn set(&self, mode: SttMode) {
        self.0.store(mode as u8, Ordering::Release);
    }

    fn get(&self) -> SttMode {
        match self.0.load(Ordering::Acquire) {
            0 => SttMode::Listening,
            1 => SttMode::Speaking,
            _ => SttMode::Suppressed,
        }
    }
}

struct Heartbeats {
    audio: Mutex<Instant>,
    transcribe: Mutex<Instant>,
}

impl Heartbeats {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            audio: Mutex::new(now),
            transcribe: Mutex::new(now),
        }
    }

    fn beat_audio(&self) {
        *self.audio.lock().unwrap() = Instant::now();
    }

    fn beat_transcribe(&self) {
        *self.transcribe.lock().unwrap() = Instant::now();
    }

    fn stuck(&self, timeout: Duration) -> Vec<&'static str> {
        let now = Instant::now();
        let mut stuck = Vec::new();
        if now.duration_since(*self.audio.lock().unwrap()) > timeout {
            stuck.push("stt-audio");
        }
        if now.duration_since(*self.transcribe.lock().unwrap()) > timeout {
            stuck.push("stt-transcribe");
        }
        stuck
    }
}

struct ClosedSegment {
    audio: Vec<f32>,
    speech_ms: u32,
    rms_dbfs: f32,
    while_tts: bool,
}

/// Lo que informa `stt-audio` al quedar listo (o fallar) abriendo el
/// micrófono y cargando el VAD.
pub struct AudioReady {
    pub device_name: String,
    pub native_sample_rate: u32,
    pub energy_floor_dbfs: f32,
}

fn provider_for(device: &str) -> &'static str {
    match device {
        "cuda" => "cuda",
        _ => "cpu",
    }
}

pub struct EngineHandle {
    mode: Arc<ModeCell>,
    shutdown: Arc<AtomicBool>,
    audio: Option<thread::JoinHandle<()>>,
    transcribe: Option<thread::JoinHandle<()>>,
    watchdog: Option<thread::JoinHandle<()>>,
}

impl EngineHandle {
    pub fn set_mode(&self, mode: SttMode) {
        self.mode.set(mode);
    }

    /// Señaliza a los tres hilos que corten y espera hasta `timeout` a que
    /// terminen. Si alguno no responde a tiempo, se loguea y se lo deja
    /// corriendo en segundo plano (muere solo cuando termina el proceso).
    pub async fn shutdown(&mut self, timeout: Duration) {
        self.shutdown.store(true, Ordering::Release);
        for handle in [self.audio.take(), self.transcribe.take(), self.watchdog.take()]
            .into_iter()
            .flatten()
        {
            let name = handle.thread().name().unwrap_or("stt-?").to_string();
            let joined = tokio::task::spawn_blocking(move || handle.join());
            match tokio::time::timeout(timeout, joined).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(_))) => tracing::warn!(hilo = %name, "el hilo de STT paniqueó al cerrar"),
                Ok(Err(_)) => {
                    tracing::warn!(hilo = %name, "no se pudo esperar el cierre del hilo de STT")
                }
                Err(_) => tracing::warn!(
                    hilo = %name,
                    "el hilo de STT no respondió a shutdown a tiempo, queda corriendo en segundo plano"
                ),
            }
        }
    }
}

pub struct SpawnResult {
    pub handle: EngineHandle,
    pub events: mpsc::Receiver<SttEvent>,
    pub audio_ready: oneshot::Receiver<Result<AudioReady, SttError>>,
    pub transcribe_ready: oneshot::Receiver<Result<(), SttError>>,
}

/// Lanza los tres hilos y vuelve de inmediato — no bloquea. La carga de
/// modelos/apertura de dispositivo ocurre *dentro* de cada hilo (ver el
/// comentario del módulo); `SttWorker::spawn` es quien espera los `oneshot`
/// de arriba con el timeout configurado.
pub fn spawn(stt: SttConfig, barge_in: BargeInConfig) -> SpawnResult {
    let (event_tx, event_rx) = mpsc::channel(64);
    let (seg_tx, seg_rx) = std_mpsc::channel();
    let (audio_ready_tx, audio_ready_rx) = oneshot::channel();
    let (transcribe_ready_tx, transcribe_ready_rx) = oneshot::channel();
    let mode = Arc::new(ModeCell::new(SttMode::Listening));
    let shutdown = Arc::new(AtomicBool::new(false));
    let heartbeats = Arc::new(Heartbeats::new());
    let stuck_timeout = Duration::from_secs(stt.stuck_state_timeout_secs);
    // Repartido una sola vez entre VAD, Whisper y Ollama (ver
    // `config::compute_thread_budget`) para que ninguno pida "todos los
    // núcleos" por separado.
    let thread_budget = compute_thread_budget(stt.cpu_threads);

    let audio = {
        let mode = mode.clone();
        let shutdown = shutdown.clone();
        let heartbeats = heartbeats.clone();
        let event_tx = event_tx.clone();
        let stt = stt.clone();
        let barge_in = barge_in.clone();
        let vad_threads = thread_budget.vad;
        thread::Builder::new()
            .name("stt-audio".to_string())
            .spawn(move || {
                let mut capture = match AudioCapture::new(stt.input_device_index) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = audio_ready_tx.send(Err(e));
                        return;
                    }
                };
                let segmenter = match SpeechSegmenter::new(
                    &stt.vad,
                    &barge_in,
                    &stt.vad_model_path,
                    vad_threads,
                    &mut capture,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = audio_ready_tx.send(Err(e));
                        return;
                    }
                };
                let clap = ClapDetector::new(stt.clap.clone());
                let ready = AudioReady {
                    device_name: capture.device_name().to_string(),
                    native_sample_rate: capture.native_sample_rate(),
                    energy_floor_dbfs: segmenter.energy_floor_dbfs(),
                };
                if audio_ready_tx.send(Ok(ready)).is_err() {
                    // SttWorker::spawn ya se rindió (timeout): no tiene
                    // sentido seguir capturando sin nadie escuchando.
                    return;
                }

                audio_loop(
                    capture, segmenter, clap, mode, shutdown, heartbeats, event_tx, seg_tx,
                )
            })
            .expect("no se pudo lanzar el hilo stt-audio")
    };

    let transcribe = {
        let shutdown = shutdown.clone();
        let heartbeats = heartbeats.clone();
        let event_tx = event_tx.clone();
        let stt = stt.clone();
        let num_threads = thread_budget.whisper;
        thread::Builder::new()
            .name("stt-transcribe".to_string())
            .spawn(move || {
                let provider = provider_for(&stt.device);

                let asr = match Asr::new(&stt.model_dir, &stt.language, provider, num_threads) {
                    Ok(a) => a,
                    Err(e) => {
                        let _ = transcribe_ready_tx.send(Err(e));
                        return;
                    }
                };
                if transcribe_ready_tx.send(Ok(())).is_err() {
                    return;
                }

                transcribe_loop(asr, seg_rx, shutdown, heartbeats, event_tx, stt.filters.clone())
            })
            .expect("no se pudo lanzar el hilo stt-transcribe")
    };

    let watchdog = {
        let shutdown = shutdown.clone();
        let heartbeats = heartbeats.clone();
        thread::Builder::new()
            .name("stt-watchdog".to_string())
            .spawn(move || watchdog_loop(shutdown, heartbeats, stuck_timeout))
            .expect("no se pudo lanzar el hilo stt-watchdog")
    };

    drop(event_tx);

    SpawnResult {
        handle: EngineHandle {
            mode,
            shutdown,
            audio: Some(audio),
            transcribe: Some(transcribe),
            watchdog: Some(watchdog),
        },
        events: event_rx,
        audio_ready: audio_ready_rx,
        transcribe_ready: transcribe_ready_rx,
    }
}

fn discard_reason_str(reason: DiscardReason) -> &'static str {
    match reason {
        DiscardReason::TooShort => "too_short",
        DiscardReason::BelowEnergyFloor => "below_energy_floor",
    }
}

fn audio_loop(
    mut capture: AudioCapture,
    mut segmenter: SpeechSegmenter,
    mut clap: ClapDetector,
    mode: Arc<ModeCell>,
    shutdown: Arc<AtomicBool>,
    heartbeats: Arc<Heartbeats>,
    event_tx: mpsc::Sender<SttEvent>,
    seg_tx: std_mpsc::Sender<ClosedSegment>,
) {
    let mut last_level_sent_at = Instant::now() - Duration::from_secs(1);

    while !shutdown.load(Ordering::Acquire) {
        heartbeats.beat_audio();
        let frame = capture.read_frame();
        let current_mode = mode.get();
        let outcome = segmenter.process_frame(frame, current_mode);

        match outcome.speech_active {
            Some(active) => {
                if clap.process(&frame, Some(active)) {
                    let _ = event_tx.blocking_send(SttEvent::ClapDetected);
                }
                let now = Instant::now();
                if now.duration_since(last_level_sent_at) >= Duration::from_millis(100) {
                    let dbfs = (rms_dbfs(&frame) * 10.0).round() / 10.0;
                    let _ = event_tx.blocking_send(SttEvent::Level { dbfs });
                    last_level_sent_at = now;
                }
            }
            None => {
                clap.process(&frame, None);
            }
        }

        for event in outcome.events {
            match event {
                SegmentEvent::VadStart { while_tts } => {
                    tracing::debug!(while_tts, "VAD: inicio de voz");
                    let _ = event_tx.blocking_send(SttEvent::VadStart);
                }
                SegmentEvent::SpeechConfirmed => {
                    let _ = event_tx.blocking_send(SttEvent::SpeechConfirmed);
                }
                SegmentEvent::Discarded {
                    reason,
                    speech_ms,
                    rms_dbfs,
                } => {
                    tracing::debug!(
                        reason = discard_reason_str(reason),
                        speech_ms,
                        rms_dbfs,
                        "audio descartado"
                    );
                    let _ = event_tx.blocking_send(SttEvent::Discarded {
                        reason: discard_reason_str(reason).to_string(),
                    });
                }
                SegmentEvent::Closed {
                    audio,
                    speech_ms,
                    rms_dbfs,
                    while_tts,
                } => {
                    let _ = event_tx.blocking_send(SttEvent::VadEnd {
                        speech_ms: Some(speech_ms),
                    });
                    let _ = seg_tx.send(ClosedSegment {
                        audio,
                        speech_ms,
                        rms_dbfs,
                        while_tts,
                    });
                }
            }
        }
    }
}

/// Descarta alucinaciones degeneradas: la misma palabra repetida
/// `max_repeat` veces seguidas o más. `max_repeat == 0` desactiva el filtro.
fn degenerate_repeat(text: &str, max_repeat: u32) -> bool {
    if max_repeat == 0 {
        return false;
    }
    let mut prev: Option<&str> = None;
    let mut run = 0u32;
    for word in text.split_whitespace() {
        if Some(word) == prev {
            run += 1;
            if run >= max_repeat {
                return true;
            }
        } else {
            prev = Some(word);
            run = 1;
        }
    }
    false
}

fn transcribe_loop(
    asr: Asr,
    seg_rx: std_mpsc::Receiver<ClosedSegment>,
    shutdown: Arc<AtomicBool>,
    heartbeats: Arc<Heartbeats>,
    event_tx: mpsc::Sender<SttEvent>,
    filters: SttFiltersConfig,
) {
    while !shutdown.load(Ordering::Acquire) {
        heartbeats.beat_transcribe();
        let segment = match seg_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(segment) => segment,
            Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let start = Instant::now();
        let text = asr.transcribe(&segment.audio);
        let transcribe_ms = start.elapsed().as_millis() as u32;

        let meta = TranscriptMeta {
            speech_ms: Some(segment.speech_ms),
            transcribe_ms: Some(transcribe_ms),
            rms_dbfs: Some(segment.rms_dbfs),
        };

        if text.is_empty() {
            let _ = event_tx.blocking_send(SttEvent::Discarded {
                reason: "empty".to_string(),
            });
            continue;
        }
        if degenerate_repeat(&text, filters.max_word_repeat) {
            let _ = event_tx.blocking_send(SttEvent::Discarded {
                reason: "word_repeat".to_string(),
            });
            continue;
        }

        let _ = event_tx.blocking_send(SttEvent::Transcript {
            text,
            while_tts: segment.while_tts,
            meta: Some(meta),
        });
    }
}

fn watchdog_loop(shutdown: Arc<AtomicBool>, heartbeats: Arc<Heartbeats>, timeout: Duration) {
    let mut already_warned: HashSet<&'static str> = HashSet::new();
    while !shutdown.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(250));
        let stuck = heartbeats.stuck(timeout);
        for name in &stuck {
            if already_warned.insert(name) {
                tracing::error!(
                    hilo = name,
                    timeout_secs = timeout.as_secs(),
                    "el motor STT parece colgado (sin señales de vida); v1 solo detecta y \
                     loguea, no hay reinicio automático de este hilo"
                );
            }
        }
        already_warned.retain(|name| stuck.contains(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_word_repeated_at_or_above_threshold() {
        assert!(degenerate_repeat("gracias gracias gracias gracias", 4));
        assert!(degenerate_repeat("no no no no no no", 4));
    }

    #[test]
    fn does_not_flag_below_threshold() {
        assert!(!degenerate_repeat("gracias gracias gracias", 4));
        assert!(!degenerate_repeat("jarvis abrí el navegador por favor", 4));
    }

    #[test]
    fn zero_disables_the_filter() {
        assert!(!degenerate_repeat("a a a a a a a a a a", 0));
    }

    #[test]
    fn resets_the_run_on_a_different_word() {
        assert!(!degenerate_repeat("sí sí sí no sí sí sí", 4));
    }
}
