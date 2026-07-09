//! Wrapper del worker de STT: spawnea el proceso Python, hace el handshake
//! de inicialización y expone un stream de transcripciones al orquestador.

mod protocol;

use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::{SttConfig, WorkersConfig};
use crate::errors::WorkerError;
use crate::ipc::{WorkerFrame, WorkerHandle};

pub use protocol::{SttInMessage, SttOutMessage};

pub enum SttEvent {
    Transcript { text: String },
    WorkerDied,
}

pub struct SttWorker {
    handle: WorkerHandle,
    frames: mpsc::Receiver<WorkerFrame>,
    shutdown_timeout: Duration,
}

impl SttWorker {
    pub async fn spawn(workers: &WorkersConfig, stt: &SttConfig) -> Result<Self, WorkerError> {
        let (handle, mut frames) =
            WorkerHandle::spawn("stt", &workers.python_executable, &workers.stt_script).await?;

        handle
            .send(&SttInMessage::Init {
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

    /// Espera el próximo evento del worker: una frase transcrita, o la caída del proceso.
    pub async fn next_transcript(&mut self) -> Option<SttEvent> {
        loop {
            match self.frames.recv().await? {
                WorkerFrame::Message(value) => {
                    match serde_json::from_value::<SttOutMessage>(value) {
                        Ok(SttOutMessage::Transcript { text, .. }) => {
                            return Some(SttEvent::Transcript { text })
                        }
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
