//! Mensajes del protocolo IPC con el worker de STT.

use serde::{Deserialize, Serialize};

/// Espeja `VadConfig` (src/config.rs) — parámetros del motor nativo.
#[derive(Debug, Serialize)]
pub struct VadInit {
    pub threshold: f32,
    pub neg_threshold: f32,
    pub pre_roll_ms: u32,
    pub min_speech_ms: u32,
    pub silence_long_ms: u32,
    pub silence_short_ms: u32,
    pub long_utterance_ms: u32,
    pub energy_floor_dbfs: Option<f32>,
    pub calibration_secs: f32,
}

/// Espeja `SttFiltersConfig` (src/config.rs) — filtros anti-alucinación.
#[derive(Debug, Serialize)]
pub struct FiltersInit {
    pub max_no_speech_prob: f32,
    pub min_avg_logprob: f32,
    pub max_compression_ratio: f32,
}

/// Espeja los campos de `BargeInConfig` que necesita el motor nativo para
/// decidir cuándo emitir `speech_confirmed` y con qué umbral de VAD entrar
/// en "recording" mientras Jarvis habla. La política de a qué modo (voz
/// cualquiera / wake word) responder vive en Rust, no acá.
#[derive(Debug, Serialize)]
pub struct BargeInInit {
    pub min_speech_ms: u32,
    pub vad_threshold_while_speaking: f32,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SttInMessage {
    Init {
        /// "native" | "realtimestt".
        engine: String,
        vad: VadInit,
        filters: FiltersInit,
        barge_in: BargeInInit,
        language: String,
        model: String,
        device: String,
        compute_type: String,
        input_device_index: Option<u32>,
        beam_size: Option<u8>,
        cpu_threads: Option<u8>,
        initial_prompt: String,
        recalibrate: bool,
        /// Los siguientes campos solo los usa el camino `realtimestt`.
        silero_sensitivity: f32,
        webrtc_sensitivity: u8,
        post_speech_silence_duration: f32,
        min_length_of_recording: f32,
        min_gap_between_recordings: f32,
        silero_deactivity_detection: bool,
        stuck_state_timeout_secs: u64,
    },
    Mute,
    Unmute,
    /// Solo lo entiende el motor nativo (ver `crate::stt::SttMode`); el
    /// camino `realtimestt` lo ignora silenciosamente si llegara a mandarse
    /// (Rust no lo hace: ver `Orchestrator::begin_speaking`/`end_speaking`).
    SetMode { mode: String },
    Shutdown,
}

/// Metadatos de telemetría de una transcripción o descarte (motor nativo).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TranscriptMeta {
    pub speech_ms: Option<u32>,
    pub transcribe_ms: Option<u32>,
    pub rms_dbfs: Option<f32>,
    pub no_speech_prob: Option<f32>,
    pub avg_logprob: Option<f32>,
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
        /// Piso de energía (dBFS) calibrado al arrancar (solo motor nativo).
        #[serde(default)]
        energy_floor_dbfs: Option<f32>,
        #[allow(dead_code)]
        sample_rate: u32,
    },
    /// Empezó a detectarse voz (solo motor nativo). En esta fase, Rust solo
    /// lo loguea — no dispara ninguna acción.
    VadStart {
        #[allow(dead_code)]
        #[serde(default)]
        while_tts: bool,
    },
    /// Terminó de detectarse voz (solo motor nativo). En esta fase, Rust solo
    /// lo loguea.
    VadEnd {
        #[serde(default)]
        speech_ms: Option<u32>,
        #[allow(dead_code)]
        #[serde(default)]
        while_tts: bool,
    },
    /// Voz sostenida durante `barge_in.min_speech_ms` mientras el motor
    /// nativo está en modo `speaking` (solo motor nativo, solo si
    /// `barge_in.enabled`). En modo `any_voice`, Rust cancela apenas llega
    /// esto, sin esperar la transcripción; en modo `wake_word` se ignora
    /// para la cancelación (se espera el `Transcript` con el nombre).
    SpeechConfirmed {
        #[allow(dead_code)]
        #[serde(default)]
        while_tts: bool,
    },
    Transcript {
        text: String,
        #[allow(dead_code)]
        timestamp: f64,
        #[serde(default)]
        while_tts: bool,
        #[serde(default)]
        meta: Option<TranscriptMeta>,
    },
    /// Audio descartado antes o después de transcribir (solo motor nativo):
    /// razones como "too_short", "below_energy_floor", "no_speech_prob",
    /// "avg_logprob", "compression_ratio".
    Discarded {
        reason: String,
        #[allow(dead_code)]
        #[serde(default)]
        meta: Option<TranscriptMeta>,
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
