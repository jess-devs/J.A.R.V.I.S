//! Mensajes del protocolo IPC con el worker de TTS (Piper).

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TtsInMessage {
    Init {
        voice_path: String,
        config_path: String,
        use_cuda: bool,
    },
    Synthesize {
        request_id: String,
        text: String,
    },
    #[allow(dead_code)]
    Shutdown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TtsOutMessage {
    Ready {
        sample_rate: u32,
        channels: u16,
        sample_width: u16,
    },
    Audio {
        request_id: String,
        #[allow(dead_code)]
        bytes: usize,
        sample_rate: u32,
        channels: u16,
        sample_width: u16,
    },
    Error {
        request_id: String,
        code: String,
        message: String,
    },
    FatalError {
        code: String,
        message: String,
    },
}
