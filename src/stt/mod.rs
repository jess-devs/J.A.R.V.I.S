//! Wrapper del worker de STT: spawnea el proceso Python, hace el handshake
//! de inicialización y expone un stream de transcripciones al orquestador.

mod protocol;

use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::{BargeInConfig, SttConfig, WorkersConfig};
use crate::errors::WorkerError;
use crate::ipc::{WorkerFrame, WorkerHandle};

pub use protocol::{
    BargeInInit, ClapInit, FiltersInit, SttInMessage, SttOutMessage, TranscriptMeta, VadInit,
};

pub enum SttEvent {
    Transcript {
        text: String,
        /// true si esta frase se capturó mientras el motor estaba en modo
        /// `speaking` (Jarvis hablando) — telemetría; la decisión real de
        /// barge-in en `Orchestrator` no depende de este flag, depende de
        /// en qué loop se recibió el evento.
        #[allow(dead_code)]
        while_tts: bool,
        meta: Option<TranscriptMeta>,
    },
    /// Solo con `engine: native`. En esta fase no dispara ninguna acción.
    VadStart,
    /// Solo con `engine: native`. En esta fase no dispara ninguna acción.
    VadEnd {
        speech_ms: Option<u32>,
    },
    /// Voz sostenida durante `barge_in.min_speech_ms` mientras Jarvis habla
    /// (solo `engine: native`, solo si `barge_in.enabled`).
    SpeechConfirmed,
    /// Solo con `engine: native`: audio descartado antes o después de transcribir.
    Discarded {
        reason: String,
    },
    /// Doble aplauso confirmado (solo `engine: native`). Ver `ClapInit`.
    ClapDetected,
    /// Energía instantánea del micrófono (dBFS), cada ~100ms mientras el
    /// motor no está suprimido — independiente del VAD, para animar el nivel
    /// real de voz del usuario en la TUI.
    Level {
        dbfs: f32,
    },
    WorkerDied,
}

/// Modo del motor STT nativo (ver `workers/stt_engine.py::ModeState`). El
/// camino `realtimestt` no lo entiende — Rust nunca se lo manda (usa
/// `mute()`/`unmute()` en su lugar, ver `Orchestrator::begin_speaking`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttMode {
    Listening,
    Speaking,
    /// Rust no lo manda hoy (usa el mensaje `Mute`, que el motor nativo
    /// mapea internamente a este modo — ver `stt_worker.py::_run_native`).
    /// Queda acá para que `SttMode` documente el espacio de estados completo.
    #[allow(dead_code)]
    Suppressed,
}

impl SttMode {
    fn as_str(self) -> &'static str {
        match self {
            SttMode::Listening => "listening",
            SttMode::Speaking => "speaking",
            SttMode::Suppressed => "suppressed",
        }
    }
}

pub struct SttWorker {
    handle: WorkerHandle,
    frames: mpsc::Receiver<WorkerFrame>,
    shutdown_timeout: Duration,
}

impl SttWorker {
    pub async fn spawn(
        workers: &WorkersConfig,
        stt: &SttConfig,
        barge_in: &BargeInConfig,
    ) -> Result<Self, WorkerError> {
        let (handle, mut frames) =
            WorkerHandle::spawn("stt", &workers.python_executable, &workers.stt_script).await?;

        handle
            .send(&SttInMessage::Init {
                engine: match stt.engine {
                    crate::config::SttEngineKind::Native => "native".to_string(),
                    crate::config::SttEngineKind::Realtimestt => "realtimestt".to_string(),
                },
                vad: VadInit {
                    threshold: stt.vad.threshold,
                    neg_threshold: stt.vad.neg_threshold,
                    pre_roll_ms: stt.vad.pre_roll_ms,
                    min_speech_ms: stt.vad.min_speech_ms,
                    silence_long_ms: stt.vad.silence_long_ms,
                    silence_short_ms: stt.vad.silence_short_ms,
                    long_utterance_ms: stt.vad.long_utterance_ms,
                    energy_floor_dbfs: stt.vad.energy_floor_dbfs,
                    calibration_secs: stt.vad.calibration_secs,
                },
                filters: FiltersInit {
                    max_no_speech_prob: stt.filters.max_no_speech_prob,
                    min_avg_logprob: stt.filters.min_avg_logprob,
                    max_compression_ratio: stt.filters.max_compression_ratio,
                },
                barge_in: BargeInInit {
                    min_speech_ms: barge_in.min_speech_ms,
                    vad_threshold_while_speaking: barge_in.echo_guard.vad_threshold_while_speaking,
                },
                clap: ClapInit {
                    min_peak_dbfs: stt.clap.min_peak_dbfs,
                    min_rise_db: stt.clap.min_rise_db,
                    decay_ms: stt.clap.decay_ms,
                    max_vad_prob: stt.clap.max_vad_prob,
                    min_zcr: stt.clap.min_zcr,
                    double_min_gap_ms: stt.clap.double_min_gap_ms,
                    double_max_gap_ms: stt.clap.double_max_gap_ms,
                    refractory_ms: stt.clap.refractory_ms,
                },
                language: stt.language.clone(),
                model: stt.whisper_model.clone(),
                device: stt.device.clone(),
                compute_type: stt.compute_type.clone(),
                input_device_index: stt.input_device_index,
                beam_size: stt.beam_size,
                cpu_threads: stt.cpu_threads,
                initial_prompt: stt.initial_prompt.clone(),
                recalibrate: stt.recalibrate,
                silero_sensitivity: stt.silero_sensitivity,
                webrtc_sensitivity: stt.webrtc_sensitivity,
                post_speech_silence_duration: stt.post_speech_silence_duration,
                min_length_of_recording: stt.min_length_of_recording,
                min_gap_between_recordings: stt.min_gap_between_recordings,
                silero_deactivity_detection: stt.silero_deactivity_detection,
                stuck_state_timeout_secs: stt.stuck_state_timeout_secs,
            })
            .await?;

        let init_timeout = Duration::from_secs(workers.stt_init_timeout_secs);
        let wait_ready = async {
            loop {
                match frames.recv().await {
                    Some(WorkerFrame::Message(value)) => {
                        match serde_json::from_value::<SttOutMessage>(value) {
                            Ok(SttOutMessage::Ready {
                                device,
                                compute_type,
                                whisper_model,
                                vram_gb,
                                beam_size,
                                cpu_threads,
                                rtf,
                                from_cache,
                                energy_floor_dbfs,
                                ..
                            }) => {
                                tracing::info!(
                                    device = %device,
                                    compute_type = %compute_type,
                                    whisper_model = %whisper_model,
                                    vram_gb = %vram_gb,
                                    beam_size = ?beam_size,
                                    cpu_threads = ?cpu_threads,
                                    rtf = ?rtf,
                                    perfil_cacheado = from_cache,
                                    energy_floor_dbfs = ?energy_floor_dbfs,
                                    "STT worker listo"
                                );
                                return Ok(());
                            }
                            Ok(SttOutMessage::FatalError { code, message }) => {
                                return Err(WorkerError::Fatal { code, message });
                            }
                            Ok(_) => continue,
                            Err(e) => return Err(WorkerError::Protocol(e.to_string())),
                        }
                    }
                    Some(WorkerFrame::MessageWithBytes(..)) => continue,
                    None => return Err(WorkerError::Crashed(None)),
                }
            }
        };

        match tokio::time::timeout(init_timeout, wait_ready).await {
            Ok(inner) => inner?,
            Err(_) => return Err(WorkerError::InitTimeout(workers.stt_init_timeout_secs)),
        }

        Ok(Self {
            handle,
            frames,
            shutdown_timeout: Duration::from_secs(workers.shutdown_timeout_secs),
        })
    }

    /// Espera el próximo evento del worker: una frase transcrita, un evento de
    /// VAD, un descarte, o la caída del proceso.
    pub async fn next_event(&mut self) -> Option<SttEvent> {
        loop {
            match self.frames.recv().await? {
                WorkerFrame::Message(value) => {
                    match serde_json::from_value::<SttOutMessage>(value) {
                        Ok(SttOutMessage::Transcript {
                            text,
                            while_tts,
                            meta,
                            ..
                        }) => {
                            return Some(SttEvent::Transcript {
                                text,
                                while_tts,
                                meta,
                            })
                        }
                        Ok(SttOutMessage::VadStart { .. }) => return Some(SttEvent::VadStart),
                        Ok(SttOutMessage::VadEnd { speech_ms, .. }) => {
                            return Some(SttEvent::VadEnd { speech_ms })
                        }
                        Ok(SttOutMessage::SpeechConfirmed { .. }) => {
                            return Some(SttEvent::SpeechConfirmed)
                        }
                        Ok(SttOutMessage::Discarded { reason, .. }) => {
                            return Some(SttEvent::Discarded { reason })
                        }
                        Ok(SttOutMessage::ClapDetected) => return Some(SttEvent::ClapDetected),
                        Ok(SttOutMessage::Level { dbfs }) => return Some(SttEvent::Level { dbfs }),
                        Ok(SttOutMessage::Error { code, message, .. }) => {
                            tracing::warn!(code = %code, message = %message, "error de transcripción recuperable");
                            continue;
                        }
                        Ok(SttOutMessage::FatalError { code, message }) => {
                            tracing::error!(code = %code, message = %message, "error fatal del worker STT");
                            return Some(SttEvent::WorkerDied);
                        }
                        Ok(_) => continue,
                        Err(e) => {
                            tracing::error!(error = %e, "mensaje STT no reconocido");
                            continue;
                        }
                    }
                }
                WorkerFrame::MessageWithBytes(..) => continue,
            }
        }
    }

    pub async fn mute(&self) -> Result<(), WorkerError> {
        self.handle.send(&SttInMessage::Mute).await
    }

    pub async fn unmute(&self) -> Result<(), WorkerError> {
        self.handle.send(&SttInMessage::Unmute).await
    }

    /// Solo tiene efecto real con `engine: native` (ver `SttMode`). Llamarlo
    /// contra un worker `realtimestt` no rompe nada (el mensaje llega y se
    /// ignora en `stt_worker.py::control_loop`), pero Rust no debería
    /// depender de eso — ver `Orchestrator::begin_speaking`.
    pub async fn set_mode(&self, mode: SttMode) -> Result<(), WorkerError> {
        self.handle
            .send(&SttInMessage::SetMode {
                mode: mode.as_str().to_string(),
            })
            .await
    }

    pub async fn shutdown(&self) {
        let _ = self.handle.send(&SttInMessage::Shutdown).await;
        let deadline = tokio::time::Instant::now() + self.shutdown_timeout;
        while self.handle.is_alive() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if self.handle.is_alive() {
            tracing::warn!(
                worker = self.handle.name(),
                "no respondió a shutdown a tiempo, forzando cierre"
            );
            self.handle.kill().await;
        }
    }
}
