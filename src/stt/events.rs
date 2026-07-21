//! Eventos públicos del motor STT — mismo contrato que consumía
//! `src/orchestrator.rs` cuando esto era un worker Python por IPC (ver
//! `SttEvent` en el `mod.rs` viejo), para no tener que tocar el resto del
//! sistema (`orchestrator.rs`, `wake.rs`, `echo_gate.rs`) al migrar el motor.

pub enum SttEvent {
    Transcript {
        text: String,
        #[allow(dead_code)]
        while_tts: bool,
        meta: Option<TranscriptMeta>,
    },
    VadStart,
    VadEnd {
        speech_ms: Option<u32>,
    },
    /// Voz sostenida durante `barge_in.min_speech_ms` mientras Jarvis habla.
    SpeechConfirmed,
    /// Audio descartado antes o después de transcribir.
    Discarded {
        reason: String,
    },
    /// Doble aplauso confirmado.
    ClapDetected,
    /// Energía instantánea del micrófono (dBFS), cada ~100ms mientras el
    /// motor no está suprimido.
    Level {
        dbfs: f32,
    },
    WorkerDied,
}

/// Modo del motor STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttMode {
    Listening,
    Speaking,
    Suppressed,
}

/// Metadatos de telemetría de una transcripción o descarte. Ya no lleva
/// `no_speech_prob`/`avg_logprob`: eran métricas del decoder autoregresivo
/// de Whisper, sin equivalente en un transducer TDT (ver `SttFiltersConfig`).
#[derive(Debug, Default, Clone)]
pub struct TranscriptMeta {
    pub speech_ms: Option<u32>,
    pub transcribe_ms: Option<u32>,
    pub rms_dbfs: Option<f32>,
}
