//! `TtsProvider` que habla con el worker Python de Piper. Las síntesis se
//! hacen de a una por vez (el worker es un único proceso secuencial), así
//! que un solo slot "pending" alcanza para correlacionar request/response —
//! el `request_id` en el wire protocol queda como aserción defensiva, no
//! como mecanismo real de concurrencia.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{oneshot, Mutex};

use crate::config::{TtsConfig, WorkersConfig};
use crate::errors::{TtsError, WorkerError};
use crate::ipc::{WorkerFrame, WorkerHandle};

use super::protocol::{TtsInMessage, TtsOutMessage};
use super::{AudioChunk, TtsProvider};

type PendingSlot = Arc<Mutex<Option<(String, oneshot::Sender<Result<AudioChunk, TtsError>>)>>>;

pub struct PiperWorkerProvider {
    handle: WorkerHandle,
    pending: PendingSlot,
    next_id: AtomicU64,
    synth_timeout: Duration,
    shutdown_timeout: Duration,
}

impl PiperWorkerProvider {
    pub async fn spawn(workers: &WorkersConfig, tts: &TtsConfig) -> Result<Self, TtsError> {
        let (handle, mut frames) =
            WorkerHandle::spawn("tts", &workers.python_executable, &workers.tts_script)
                .await
                .map_err(TtsError::Worker)?;

        handle
            .send(&TtsInMessage::Init {
                voice_path: tts.piper.voice_path.to_string_lossy().into_owned(),
                config_path: tts.piper.config_path.to_string_lossy().into_owned(),
                use_cuda: tts.piper.use_cuda,
                length_scale: tts.piper.length_scale,
                noise_w_scale: tts.piper.noise_w_scale,
            })
            .await
            .map_err(TtsError::Worker)?;

        let init_timeout = Duration::from_secs(workers.tts_init_timeout_secs);
        let wait_ready = async {
            loop {
                match frames.recv().await {
                    Some(WorkerFrame::Message(value)) => {
                        match serde_json::from_value::<TtsOutMessage>(value) {
                            Ok(TtsOutMessage::Ready {
                                sample_rate,
                                channels,
                                sample_width,
                            }) => return Ok((sample_rate, channels, sample_width)),
                            Ok(TtsOutMessage::FatalError { code, message }) => {
                                return Err(WorkerError::Fatal { code, message })
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

        let ready = match tokio::time::timeout(init_timeout, wait_ready).await {
            Ok(inner) => inner.map_err(TtsError::Worker)?,
            Err(_) => {
                return Err(TtsError::Worker(WorkerError::InitTimeout(
                    workers.tts_init_timeout_secs,
                )))
            }
        };

        tracing::info!(
            sample_rate = ready.0,
            channels = ready.1,
            sample_width = ready.2,
            "TTS worker listo"
        );

        let pending: PendingSlot = Arc::new(Mutex::new(None));

        tokio::spawn({
            let pending = pending.clone();
            async move {
                while let Some(frame) = frames.recv().await {
                    match frame {
                        WorkerFrame::MessageWithBytes(value, bytes) => {
                            if let Ok(TtsOutMessage::Audio {
                                request_id,
                                sample_rate,
                                channels,
                                sample_width,
                                ..
                            }) = serde_json::from_value::<TtsOutMessage>(value)
                            {
                                resolve_pending(
                                    &pending,
                                    &request_id,
                                    Ok(AudioChunk {
                                        pcm: bytes,
                                        sample_rate,
                                        channels,
                                        sample_width,
                                    }),
                                )
                                .await;
                            }
                        }
                        WorkerFrame::Message(value) => {
                            match serde_json::from_value::<TtsOutMessage>(value) {
                                Ok(TtsOutMessage::Error {
                                    request_id,
                                    code,
                                    message,
                                }) => {
                                    resolve_pending(
                                        &pending,
                                        &request_id,
                                        Err(TtsError::Worker(WorkerError::Fatal { code, message })),
                                    )
                                    .await;
                                }
                                Ok(TtsOutMessage::FatalError { code, message }) => {
                                    tracing::error!(code = %code, message = %message, "error fatal del worker TTS");
                                    let mut guard = pending.lock().await;
                                    if let Some((_, tx)) = guard.take() {
                                        let _ =
                                            tx.send(Err(TtsError::Worker(WorkerError::Fatal {
                                                code,
                                                message,
                                            })));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                let mut guard = pending.lock().await;
                if let Some((_, tx)) = guard.take() {
                    let _ = tx.send(Err(TtsError::Worker(WorkerError::Crashed(None))));
                }
            }
        });

        Ok(Self {
            handle,
            pending,
            next_id: AtomicU64::new(0),
            synth_timeout: Duration::from_secs(tts.synth_timeout_secs),
            shutdown_timeout: Duration::from_secs(workers.shutdown_timeout_secs),
        })
    }
}

async fn resolve_pending(
    pending: &PendingSlot,
    request_id: &str,
    result: Result<AudioChunk, TtsError>,
) {
    let mut guard = pending.lock().await;
    let matches = matches!(guard.as_ref(), Some((id, _)) if id == request_id);
    if matches {
        if let Some((_, tx)) = guard.take() {
            let _ = tx.send(result);
        }
    } else {
        // Mismatch esperado cuando se cancela un turno (Piper no tiene
        // mensaje de cancelación: sigue sintetizando la frase abortada
        // mientras el turno siguiente ya pidió la suya) — no es un bug.
        tracing::debug!(
            request_id,
            "respuesta TTS con request_id inesperado, se ignora"
        );
    }
}

#[async_trait]
impl TtsProvider for PiperWorkerProvider {
    async fn synthesize(&self, text: &str) -> Result<AudioChunk, TtsError> {
        let request_id = self.next_id.fetch_add(1, Ordering::SeqCst).to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut guard = self.pending.lock().await;
            *guard = Some((request_id.clone(), tx));
        }

        self.handle
            .send(&TtsInMessage::Synthesize {
                request_id: request_id.clone(),
                text: text.to_string(),
            })
            .await
            .map_err(TtsError::Worker)?;

        match tokio::time::timeout(self.synth_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(TtsError::Worker(WorkerError::Crashed(None))),
            Err(_) => Err(TtsError::Worker(WorkerError::Timeout(
                self.synth_timeout.as_secs(),
            ))),
        }
    }

    async fn shutdown(&self) {
        let _ = self.handle.send(&TtsInMessage::Shutdown).await;
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
