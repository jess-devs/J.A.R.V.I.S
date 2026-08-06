//! Wrapper chico para el worker de calibración de micrófono: un proceso
//! Python separado y liviano (no carga Whisper/Silero/torch) que enumera
//! dispositivos PyAudio y transmite el nivel dBFS en vivo de uno elegido.
//!
//! Corre completamente aparte del `SttWorker` de producción — ver
//! `docs/planning/onboarding-mic-calibracion.md`, sección "Decisión
//! arquitectónica central", sobre por qué no comparten proceso ni pausan
//! uno al otro.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::config::WorkersConfig;
use crate::errors::WorkerError;
use crate::ipc::{WorkerFrame, WorkerHandle};

use super::protocol::{AudioDeviceInfo, SttInMessage, SttOutMessage};

/// El worker de calibración no carga ningún modelo (solo PyAudio), así que
/// un handshake lento indica un problema real (proceso no arrancó,
/// antivirus/EDR lento resolviendo el intérprete) más que "todavía
/// cargando" — a diferencia de `stt_init_timeout_secs` (pensado para cargar
/// Whisper), esta constante no necesita ser configurable.
const CALIBRATION_INIT_TIMEOUT_SECS: u64 = 10;

/// Serializable con el mismo shape que se manda por el WebSocket de
/// calibración hacia el navegador (ver `crate::config_ui::calibration`) —
/// el frontend recibe literalmente `serde_json::to_string(&event)`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CalibrationEvent {
    Devices {
        devices: Vec<AudioDeviceInfo>,
    },
    Started {
        device_index: u32,
        device_name: String,
        sample_rate: u32,
    },
    Level {
        dbfs: f32,
    },
    /// La grabación de enrollment de voz arrancó.
    EnrollStarted {
        device_index: u32,
        device_name: String,
        total_ms: u32,
    },
    /// Progreso de la grabación en curso, cada ~200ms.
    EnrollProgress {
        elapsed_ms: u32,
        total_ms: u32,
    },
    /// Grabación terminada, calculando el embedding — puede tardar (carga
    /// perezosa del modelo de `speechbrain`).
    EnrollProcessing,
    /// Embedding calculado y guardado con éxito.
    EnrollComplete {
        embedding_path: String,
    },
    /// Error recuperable (dispositivo ocupado/exclusivo, enumeración
    /// fallida, enrollment fallido, etc.) — el worker sigue vivo, se puede
    /// reintentar.
    Error {
        code: String,
        message: String,
    },
    /// El proceso murió (fatal_error, o el stream de stdout se cerró sin
    /// aviso). Tratar igual que `next_event() == None` — ver
    /// `Orchestrator` para el mismo patrón con `SttEvent::WorkerDied`.
    Died,
}

pub struct CalibrationWorker {
    handle: WorkerHandle,
    frames: mpsc::Receiver<WorkerFrame>,
    shutdown_timeout: Duration,
}

impl CalibrationWorker {
    pub async fn spawn(workers: &WorkersConfig, runtime_dir: &Path) -> Result<Self, WorkerError> {
        let (handle, mut frames) = WorkerHandle::spawn(
            "stt-calib",
            &workers.python_executable,
            &workers.stt_script,
            runtime_dir,
            &[],
        )
        .await?;

        handle.send(&SttInMessage::Calibrate).await?;

        let init_timeout = Duration::from_secs(CALIBRATION_INIT_TIMEOUT_SECS);
        let wait_ready = async {
            loop {
                match frames.recv().await {
                    Some(WorkerFrame::Message(value)) => {
                        match serde_json::from_value::<SttOutMessage>(value) {
                            Ok(SttOutMessage::CalibrationReady) => return Ok(()),
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
            Err(_) => return Err(WorkerError::InitTimeout(CALIBRATION_INIT_TIMEOUT_SECS)),
        }

        Ok(Self {
            handle,
            frames,
            shutdown_timeout: Duration::from_secs(workers.shutdown_timeout_secs),
        })
    }

    pub async fn list_devices(&self) -> Result<(), WorkerError> {
        self.handle.send(&SttInMessage::ListDevices).await
    }

    pub async fn start(&self, device_index: Option<u32>) -> Result<(), WorkerError> {
        self.handle
            .send(&SttInMessage::StartCalibration { device_index })
            .await
    }

    pub async fn stop(&self) -> Result<(), WorkerError> {
        self.handle.send(&SttInMessage::StopCalibration).await
    }

    pub async fn start_enroll(
        &self,
        device_index: Option<u32>,
        seconds: f32,
    ) -> Result<(), WorkerError> {
        self.handle
            .send(&SttInMessage::StartEnroll {
                device_index,
                seconds,
            })
            .await
    }

    pub async fn cancel_enroll(&self) -> Result<(), WorkerError> {
        self.handle.send(&SttInMessage::CancelEnroll).await
    }

    /// Espera el próximo evento del worker. `None` indica que el stream de
    /// frames se cerró (proceso terminado) sin que llegara a mandarse un
    /// `fatal_error` explícito — el llamador debe tratarlo igual que
    /// `Some(CalibrationEvent::Died)`.
    pub async fn next_event(&mut self) -> Option<CalibrationEvent> {
        loop {
            match self.frames.recv().await? {
                WorkerFrame::Message(value) => {
                    match serde_json::from_value::<SttOutMessage>(value) {
                        Ok(SttOutMessage::Devices { devices }) => {
                            return Some(CalibrationEvent::Devices { devices })
                        }
                        Ok(SttOutMessage::CalibrationStarted {
                            device_index,
                            device_name,
                            sample_rate,
                        }) => {
                            return Some(CalibrationEvent::Started {
                                device_index,
                                device_name,
                                sample_rate,
                            })
                        }
                        Ok(SttOutMessage::Level { dbfs }) => {
                            return Some(CalibrationEvent::Level { dbfs })
                        }
                        Ok(SttOutMessage::EnrollStarted {
                            device_index,
                            device_name,
                            total_ms,
                        }) => {
                            return Some(CalibrationEvent::EnrollStarted {
                                device_index,
                                device_name,
                                total_ms,
                            })
                        }
                        Ok(SttOutMessage::EnrollProgress {
                            elapsed_ms,
                            total_ms,
                        }) => {
                            return Some(CalibrationEvent::EnrollProgress {
                                elapsed_ms,
                                total_ms,
                            })
                        }
                        Ok(SttOutMessage::EnrollProcessing) => {
                            return Some(CalibrationEvent::EnrollProcessing)
                        }
                        Ok(SttOutMessage::EnrollComplete { embedding_path }) => {
                            return Some(CalibrationEvent::EnrollComplete { embedding_path })
                        }
                        Ok(SttOutMessage::Error { code, message, .. }) => {
                            return Some(CalibrationEvent::Error { code, message })
                        }
                        Ok(SttOutMessage::FatalError { code, message }) => {
                            tracing::error!(
                                code = %code,
                                message = %message,
                                "error fatal del worker de calibración"
                            );
                            return Some(CalibrationEvent::Died);
                        }
                        Ok(_) => continue,
                        Err(e) => {
                            tracing::error!(error = %e, "mensaje de calibración no reconocido");
                            continue;
                        }
                    }
                }
                WorkerFrame::MessageWithBytes(..) => continue,
            }
        }
    }

    /// Mismo patrón que `SttWorker::shutdown`: pide un cierre ordenado y,
    /// si no responde a tiempo, fuerza la terminación. Siempre debe
    /// llamarse al terminar de usar el worker (ver el handler WS en
    /// `crate::config_ui::calibration`) para no dejar procesos Python
    /// huérfanos con el micrófono abierto.
    pub async fn shutdown(&self) {
        let _ = self.handle.send(&SttInMessage::StopCalibration).await;
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
