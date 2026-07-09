//! Mensajes del protocolo IPC con el worker de STT.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SttInMessage {
    Init {
        language: String,
        model: String,
        device: String,
        compute_type: String,
        input_device_index: Option<u32>,
        silero_sensitivity: f32,
        webrtc_sensitivity: u8,
        post_speech_silence_duration: f32,
    },
    Mute,
    Unmute,
    Shutdown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SttOutMessage {
    Ready {
        device: String,
        compute_type: String,
        whisper_model: String,
        vram_gb: f32,
        #[allow(dead_code)]
        sample_rate: u32,
    },
    Transcript {
        text: String,
        #[allow(dead_code)]
        timestamp: f64,
    },
    Error {
        code: String,
        message: String,
        #[allow(dead_code)]
        recoverable: bool,
    },
    FatalError {
        code: String,
        message: String,
    },
}
