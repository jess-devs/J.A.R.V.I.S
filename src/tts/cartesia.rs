use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message;

use crate::config::{CartesiaConfig, CartesiaTransport};
use crate::errors::TtsError;

use super::{AudioChunk, TtsProvider};

const API_BASE: &str = "https://api.cartesia.ai";
const WS_BASE: &str = "wss://api.cartesia.ai";
const REQUEST_TIMEOUT_SECS: u64 = 30;

pub struct CartesiaProvider {
    client: reqwest::Client,
    model_id: String,
    voice_id: String,
    language: Option<String>,
    output_format: OutputFormat,
    api_key_env: String,
    cartesia_version: String,
    transport: CartesiaTransport,
}

impl CartesiaProvider {
    pub fn new(config: &CartesiaConfig) -> Result<Self, TtsError> {
        if std::env::var(&config.api_key_env).is_err() {
            return Err(TtsError::MissingApiKey(config.api_key_env.clone()));
        }
        let client = crate::http::client(Duration::from_secs(REQUEST_TIMEOUT_SECS));
        Ok(Self {
            client,
            model_id: config.model_id.clone(),
            voice_id: config.voice_id.clone(),
            language: config.language.clone(),
            output_format: OutputFormat {
                container: config.output_format.container.clone(),
                encoding: config.output_format.encoding.clone(),
                sample_rate: config.output_format.sample_rate,
            },
            api_key_env: config.api_key_env.clone(),
            cartesia_version: config.cartesia_version.clone(),
            transport: config.transport,
        })
    }

    fn sample_rate(&self) -> u32 {
        self.output_format.sample_rate
    }

    fn sample_width(&self) -> u16 {
        match self.output_format.encoding.as_str() {
            "pcm_s16le" => 2,
            "pcm_f32le" => 4,
            _ => 2,
        }
    }

    fn api_key(&self) -> Result<String, TtsError> {
        std::env::var(&self.api_key_env)
            .map_err(|_| TtsError::MissingApiKey(self.api_key_env.clone()))
    }
}

#[derive(Debug, Clone, Serialize)]
struct OutputFormat {
    container: String,
    encoding: String,
    sample_rate: u32,
}

#[derive(Serialize)]
struct VoiceSpecifier<'a> {
    mode: &'a str,
    id: &'a str,
}

#[derive(Serialize)]
struct TtsBytesRequest<'a> {
    model_id: &'a str,
    transcript: &'a str,
    voice: VoiceSpecifier<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    output_format: &'a OutputFormat,
}

#[derive(Serialize)]
struct WsGenerationRequest<'a> {
    model_id: &'a str,
    transcript: &'a str,
    voice: VoiceSpecifier<'a>,
    output_format: &'a OutputFormat,
    context_id: &'a str,
    #[serde(rename = "continue")]
    contin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct WsChunkMessage {
    data: Option<String>,
    #[allow(dead_code)]
    done: Option<bool>,
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

fn gen_context_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let b: [u8; 16] = rng.gen();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
    )
}

impl CartesiaProvider {
    async fn synthesize_rest(&self, text: &str) -> Result<AudioChunk, TtsError> {
        let api_key = self.api_key()?;

        let voice_spec = VoiceSpecifier {
            mode: "id",
            id: &self.voice_id,
        };

        let language = self.language.as_deref();

        let request_body = TtsBytesRequest {
            model_id: &self.model_id,
            transcript: text,
            voice: voice_spec,
            language,
            output_format: &self.output_format,
        };

        let url = format!("{API_BASE}/tts/bytes");
        let response = self
            .client
            .post(&url)
            .header("Cartesia-Version", &self.cartesia_version)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&request_body)
            .send()
            .await
            .map_err(TtsError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(TtsError::UnexpectedResponse(format!(
                "Cartesia respondió {status}: {body}"
            )));
        }

        let pcm = response.bytes().await.map_err(TtsError::Network)?;
        Ok(AudioChunk {
            pcm: pcm.to_vec(),
            sample_rate: self.sample_rate(),
            channels: 1,
            sample_width: self.sample_width(),
        })
    }

    async fn synthesize_ws(&self, text: &str) -> Result<AudioChunk, TtsError> {
        let api_key = self.api_key()?;

        let ws_url = format!(
            "{WS_BASE}/tts/websocket?cartesia_version={}&access_token={}",
            self.cartesia_version, api_key
        );

        let (ws_stream, _response) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| TtsError::UnexpectedResponse(format!("error conectando WS: {e}")))?;

        let (mut write, mut read) = ws_stream.split();
        let context_id = gen_context_id();
        let voice_spec = VoiceSpecifier {
            mode: "id",
            id: &self.voice_id,
        };
        let language = self.language.as_deref();

        let req = WsGenerationRequest {
            model_id: &self.model_id,
            transcript: text,
            voice: voice_spec,
            output_format: &self.output_format,
            context_id: &context_id,
            contin: false,
            language,
        };

        let req_json = serde_json::to_string(&req).map_err(|e| {
            TtsError::UnexpectedResponse(format!("error serializando request: {e}"))
        })?;

        write
            .send(Message::Text(req_json.into()))
            .await
            .map_err(|e| TtsError::UnexpectedResponse(format!("error enviando WS: {e}")))?;

        let mut audio_bytes = Vec::new();

        while let Some(msg) = read.next().await {
            let msg =
                msg.map_err(|e| TtsError::UnexpectedResponse(format!("error recibiendo WS: {e}")))?;
            match msg {
                Message::Text(text) => {
                    let parsed: WsChunkMessage = serde_json::from_str(&text).map_err(|e| {
                        TtsError::UnexpectedResponse(format!("mensaje WS inesperado: {e}: {text}"))
                    })?;

                    match parsed.msg_type.as_str() {
                        "chunk" => {
                            if let Some(b64) = parsed.data {
                                let decoded = base64::engine::general_purpose::STANDARD
                                    .decode(&b64)
                                    .map_err(|e| {
                                        TtsError::UnexpectedResponse(format!(
                                            "error decodificando base64: {e}"
                                        ))
                                    })?;
                                audio_bytes.extend(decoded);
                            }
                        }
                        "done" => {
                            break;
                        }
                        "error" => {
                            return Err(TtsError::UnexpectedResponse(format!(
                                "Cartesia WS error [{}]: {} — {}",
                                parsed.error_code.as_deref().unwrap_or("unknown"),
                                parsed.title.as_deref().unwrap_or(""),
                                parsed.message.as_deref().unwrap_or(""),
                            )));
                        }
                        _ => {}
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        let _ = write.close().await;

        if audio_bytes.is_empty() {
            return Err(TtsError::UnexpectedResponse(
                "WebSocket cerró sin devolver audio".to_string(),
            ));
        }

        Ok(AudioChunk {
            pcm: audio_bytes,
            sample_rate: self.sample_rate(),
            channels: 1,
            sample_width: self.sample_width(),
        })
    }
}

#[async_trait]
impl TtsProvider for CartesiaProvider {
    async fn synthesize(&self, text: &str) -> Result<AudioChunk, TtsError> {
        match self.transport {
            CartesiaTransport::Rest => self.synthesize_rest(text).await,
            CartesiaTransport::WebSocket => self.synthesize_ws(text).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synthesize_rest_spanish() {
        let _ = dotenvy::dotenv();

        if std::env::var("CARTESIA_API_KEY").is_err() {
            eprintln!("SALTANDO test: CARTESIA_API_KEY no definida en .env");
            return;
        }

        let mut config = CartesiaConfig::default();
        config.voice_id = "db6b0ed5-d5d3-463d-ae85-518a07d3c2b4".to_string();

        let provider = CartesiaProvider::new(&config).expect("crear CartesiaProvider");
        let chunk = provider
            .synthesize("Hola, soy Jarvis. Es un placer estar aquí.")
            .await
            .expect("síntesis REST");

        assert_eq!(chunk.channels, 1);
        assert_eq!(chunk.sample_rate, config.output_format.sample_rate);
        assert_eq!(chunk.sample_width, 2);
        assert!(
            chunk.pcm.len() > 1000,
            "audio muy corto: {} bytes",
            chunk.pcm.len()
        );
    }

    #[tokio::test]
    async fn synthesize_ws_spanish() {
        let _ = dotenvy::dotenv();

        if std::env::var("CARTESIA_API_KEY").is_err() {
            eprintln!("SALTANDO test: CARTESIA_API_KEY no definida en .env");
            return;
        }

        let mut config = CartesiaConfig::default();
        config.voice_id = "db6b0ed5-d5d3-463d-ae85-518a07d3c2b4".to_string();
        config.transport = CartesiaTransport::WebSocket;

        let provider = CartesiaProvider::new(&config).expect("crear CartesiaProvider");
        let chunk = provider
            .synthesize("Hola, soy Jarvis. Es un placer estar aquí.")
            .await
            .expect("síntesis WS");

        assert_eq!(chunk.channels, 1);
        assert_eq!(chunk.sample_rate, config.output_format.sample_rate);
        assert_eq!(chunk.sample_width, 2);
        assert!(
            chunk.pcm.len() > 1000,
            "audio muy corto: {} bytes",
            chunk.pcm.len()
        );
    }
}
