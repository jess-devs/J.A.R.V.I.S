//! Interfaz común de TTS: `PiperWorkerProvider` (local, único proveedor).

pub mod piper_worker;
mod protocol;

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::{Config, TtsProviderKind};
use crate::errors::TtsError;

#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub pcm: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_width: u16,
}

#[async_trait]
pub trait TtsProvider: Send + Sync {
    async fn synthesize(&self, text: &str) -> Result<AudioChunk, TtsError>;

    /// Libera recursos del proveedor (ej. cierra el worker Python). No-op
    /// por defecto para proveedores sin estado que cerrar (ej. HTTP puro).
    async fn shutdown(&self) {}
}

pub async fn build_provider(config: &Config) -> Result<Arc<dyn TtsProvider>, TtsError> {
    match config.tts.provider {
        TtsProviderKind::Piper => {
            let provider =
                piper_worker::PiperWorkerProvider::spawn(&config.workers, &config.tts).await?;
            Ok(Arc::new(provider))
        }
    }
}
