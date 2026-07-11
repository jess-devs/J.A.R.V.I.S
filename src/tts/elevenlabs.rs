//! TTS en la nube vía ElevenLabs. HTTP directo (no pasa por ningún worker
//! Python): `POST /v1/text-to-speech/{voice_id}?output_format=pcm_*` con
//! `output_format=pcm_*` devuelve PCM crudo (s16le, mono) sin necesidad de
//! decodificar mp3/ffmpeg.

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use crate::config::ElevenLabsConfig;
use crate::errors::TtsError;

use super::{AudioChunk, TtsProvider};

const API_BASE: &str = "https://api.elevenlabs.io/v1/text-to-speech";
const REQUEST_TIMEOUT_SECS: u64 = 30;

pub struct ElevenLabsProvider {
    client: reqwest::Client,
    voice_id: String,
    model_id: String,
    output_format: String,
    api_key_env: String,
}

impl ElevenLabsProvider {
    pub fn new(config: &ElevenLabsConfig) -> Result<Self, TtsError> {
        if std::env::var(&config.api_key_env).is_err() {
            return Err(TtsError::MissingApiKey(config.api_key_env.clone()));
        }
        let client = crate::http::client(Duration::from_secs(REQUEST_TIMEOUT_SECS));
        Ok(Self {
            client,
            voice_id: config.voice_id.clone(),
            model_id: config.model_id.clone(),
            output_format: config.output_format.clone(),
            api_key_env: config.api_key_env.clone(),
        })
    }

    /// Deriva el sample rate del nombre del output_format (ej. "pcm_22050" -> 22050).
    fn sample_rate(&self) -> u32 {
        self.output_format
            .rsplit('_')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(22050)
    }
}

#[derive(Serialize)]
struct SynthesizeRequest<'a> {
    text: &'a str,
    model_id: &'a str,
}

#[async_trait]
impl TtsProvider for ElevenLabsProvider {
    async fn synthesize(&self, text: &str) -> Result<AudioChunk, TtsError> {
        if !self.output_format.starts_with("pcm_") {
            return Err(TtsError::UnexpectedResponse(format!(
                "elevenlabs.output_format='{}' no es un formato PCM crudo soportado (usá algo como 'pcm_22050')",
                self.output_format
            )));
        }

        let api_key = std::env::var(&self.api_key_env)
            .map_err(|_| TtsError::MissingApiKey(self.api_key_env.clone()))?;

        let url = format!(
            "{API_BASE}/{}?output_format={}",
            self.voice_id, self.output_format
        );

        let response = self
            .client
            .post(&url)
            .header("xi-api-key", api_key)
            .json(&SynthesizeRequest {
                text,
                model_id: &self.model_id,
            })
            .send()
            .await
            .map_err(TtsError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(TtsError::UnexpectedResponse(format!(
                "ElevenLabs respondió {status}: {body}"
            )));
        }

        let pcm = response.bytes().await.map_err(TtsError::Network)?;

        Ok(AudioChunk {
            pcm: pcm.to_vec(),
            sample_rate: self.sample_rate(),
            channels: 1,
            sample_width: 2,
        })
    }
}
