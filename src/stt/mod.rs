//! Motor de reconocimiento de voz nativo: captura de micrófono, VAD,
//! detección de aplausos y transcripción (Whisper vía sherpa-onnx)
//! corriendo en hilos dedicados dentro del propio proceso de Jarvis — sin
//! subproceso Python ni IPC (a diferencia del motor anterior).
//!
//! `SttWorker` es la única superficie pública: mismo contrato
//! (`spawn`/`next_event`/`mute`/`unmute`/`set_mode`/`shutdown`) que tenía el
//! worker por IPC, para que `orchestrator.rs`, `wake.rs` y `echo_gate.rs` no
//! necesiten cambios más allá de lo que ya usan.

mod asr;
mod capture;
mod clap;
mod engine;
mod events;
mod vad;

pub use events::{SttEvent, SttMode};

use std::time::Duration;

use crate::config::{BargeInConfig, SttConfig, WorkersConfig};
use crate::errors::SttError;

pub struct SttWorker {
    handle: engine::EngineHandle,
    events: tokio::sync::mpsc::Receiver<SttEvent>,
    shutdown_timeout: Duration,
}

impl SttWorker {
    pub async fn spawn(
        workers: &WorkersConfig,
        stt: &SttConfig,
        barge_in: &BargeInConfig,
    ) -> Result<Self, SttError> {
        let init_timeout = Duration::from_secs(workers.stt_init_timeout_secs);
        // No bloquea: lanza los hilos y vuelve al toque. Cada hilo abre su
        // propio dispositivo/modelo y avisa por su `oneshot` — ver el
        // comentario de `engine::spawn` sobre por qué no se arma todo acá
        // antes de lanzarlos (afinidad de hilo de `cpal::Stream`).
        let mut spawn_result = engine::spawn(stt.clone(), barge_in.clone());

        let wait_ready = async {
            let audio = (&mut spawn_result.audio_ready).await;
            let transcribe = (&mut spawn_result.transcribe_ready).await;
            (audio, transcribe)
        };

        let ready_info = match tokio::time::timeout(init_timeout, wait_ready).await {
            Ok((Ok(Ok(audio)), Ok(Ok(())))) => audio,
            Ok((Ok(Err(e)), _)) | Ok((_, Ok(Err(e)))) => return Err(e),
            Ok((Err(_), _)) | Ok((_, Err(_))) => {
                return Err(SttError::ModelLoad(
                    "un hilo del STT se cerró antes de quedar listo".to_string(),
                ))
            }
            Err(_elapsed) => return Err(SttError::InitTimeout(workers.stt_init_timeout_secs)),
        };

        tracing::info!(
            device = %ready_info.device_name,
            native_sample_rate = ready_info.native_sample_rate,
            energy_floor_dbfs = ready_info.energy_floor_dbfs,
            "STT listo"
        );

        Ok(Self {
            handle: spawn_result.handle,
            events: spawn_result.events,
            shutdown_timeout: Duration::from_secs(workers.shutdown_timeout_secs),
        })
    }

    /// Espera el próximo evento del motor: una frase transcrita, un evento
    /// de VAD, un descarte, o la caída de alguno de sus hilos.
    pub async fn next_event(&mut self) -> Option<SttEvent> {
        // El canal solo se cierra cuando ambos hilos de trabajo terminaron
        // (panic o, en teoría, salida normal) — no hay ruta de "shutdown
        // silencioso" que un llamador de `next_event` pueda observar (ver
        // `Orchestrator::shutdown`, que deja de leer eventos antes de pedir
        // el cierre).
        Some(self.events.recv().await.unwrap_or(SttEvent::WorkerDied))
    }

    pub async fn mute(&self) -> Result<(), SttError> {
        self.handle.set_mode(SttMode::Suppressed);
        Ok(())
    }

    pub async fn unmute(&self) -> Result<(), SttError> {
        self.handle.set_mode(SttMode::Listening);
        Ok(())
    }

    pub async fn set_mode(&self, mode: SttMode) -> Result<(), SttError> {
        self.handle.set_mode(mode);
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        self.handle.shutdown(self.shutdown_timeout).await;
    }
}
