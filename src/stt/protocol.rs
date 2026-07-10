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
        beam_size: Option<u8>,
        cpu_threads: Option<u8>,
        initial_prompt: String,
        recalibrate: bool,
        silero_sensitivity: f32,
        webrtc_sensitivity: u8,
        post_speech_silence_duration: f32,
        min_length_of_recording: f32,
        min_gap_between_recordings: f32,
        silero_deactivity_detection: bool,
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
        #[serde(default)]
        beam_size: Option<u8>,
        #[serde(default)]
        cpu_threads: Option<u8>,
        /// Real-time factor medido en la calibración (solo camino CPU-auto).
        #[serde(default)]
        rtf: Option<f32>,
        /// true si el perfil salió del caché de calibración, sin re-medir.
        #[serde(default)]
        from_cache: bool,
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
