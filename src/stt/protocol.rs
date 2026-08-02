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

/// Espeja `SpeakerVerificationConfig` (src/config.rs) — modo sombra del
/// ítem 4 de MEJORAS.md: si `enabled`, el motor nativo calcula (en un hilo
/// aparte, sin sumar latencia al turno) la similitud coseno de cada frase
/// contra la voz enrolada con `--enroll-voice`, y la manda como
/// `speaker_similarity` para quedar logueada — todavía no gatea ninguna
/// confirmación.
#[derive(Debug, Serialize)]
pub struct SpeakerVerificationInit {
    pub enabled: bool,
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

/// Espeja `ClapConfig` (src/config.rs, dentro de `SttConfig`) — parámetros
/// del detector de doble aplauso del motor nativo (modo bienvenida).
#[derive(Debug, Serialize)]
pub struct ClapInit {
    pub min_peak_dbfs: f32,
    pub min_rise_db: f32,
    pub decay_ms: u32,
    pub max_vad_prob: f32,
    pub min_zcr: f32,
    pub double_min_gap_ms: u32,
    pub double_max_gap_ms: u32,
    pub refractory_ms: u32,
}

/// Un dispositivo de entrada de audio tal como lo ve PyAudio (`index` es el
/// índice de PyAudio, NO el de `cpal` — ver `startup_checks.rs` sobre por
/// qué esos dos espacios de índices no coinciden).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioDeviceInfo {
    pub index: u32,
    pub name: String,
    pub max_input_channels: u32,
    pub default_sample_rate: u32,
    pub is_default: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SttInMessage {
    /// Alternativa a `Init` como primer mensaje: arranca el worker en modo
    /// calibración (solo PyAudio, sin cargar Whisper/Silero). Ver
    /// `crate::stt::calibration::CalibrationWorker`.
    Calibrate,
    /// Solo válido después de `Calibrate`/`CalibrationReady`.
    ListDevices,
    /// Solo válido después de `Calibrate`/`CalibrationReady`. Reabre el
    /// stream si ya había uno abierto contra otro dispositivo.
    StartCalibration {
        device_index: Option<u32>,
    },
    /// Solo válido después de `Calibrate`/`CalibrationReady`.
    StopCalibration,
    Init {
        /// "native" | "realtimestt".
        engine: String,
        vad: VadInit,
        filters: FiltersInit,
        barge_in: BargeInInit,
        clap: ClapInit,
        speaker_verification: SpeakerVerificationInit,
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
    SetMode {
        mode: String,
    },
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
    /// Respuesta a `SttInMessage::Calibrate`: el worker de calibración está
    /// listo para recibir `ListDevices`/`StartCalibration`.
    CalibrationReady,
    /// Respuesta a `SttInMessage::ListDevices`.
    Devices {
        devices: Vec<AudioDeviceInfo>,
    },
    /// Respuesta a `SttInMessage::StartCalibration`: el stream quedó
    /// abierto contra `device_index`. A partir de acá empiezan a llegar
    /// `Level{dbfs}` cada ~100ms, igual que en el motor nativo.
    CalibrationStarted {
        device_index: u32,
        device_name: String,
        sample_rate: u32,
    },
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
    /// Doble aplauso confirmado (solo motor nativo). Ver `ClapInit`.
    ClapDetected,
    /// Energía instantánea del micrófono (dBFS), emitida cada ~100ms
    /// mientras el motor nativo no está suprimido — independiente del VAD,
    /// para animar el nivel real de voz del usuario en la TUI.
    Level {
        #[serde(default)]
        dbfs: f32,
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
    /// Modo sombra del ítem 4 de MEJORAS.md (ver `SpeakerVerificationInit`):
    /// similitud coseno de la última frase contra la voz enrolada. Llega
    /// asíncrono, después del `Transcript` correspondiente (se calcula en
    /// un hilo aparte para no sumarle latencia al turno) — hoy solo se
    /// loguea, no gatea ninguna confirmación.
    SpeakerSimilarity {
        similarity: f32,
        #[allow(dead_code)]
        text_preview: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrate_serializa_como_type_calibrate() {
        let json = serde_json::to_value(&SttInMessage::Calibrate).unwrap();
        assert_eq!(json, serde_json::json!({"type": "calibrate"}));
    }

    #[test]
    fn list_devices_serializa_como_type_list_devices() {
        let json = serde_json::to_value(&SttInMessage::ListDevices).unwrap();
        assert_eq!(json, serde_json::json!({"type": "list_devices"}));
    }

    #[test]
    fn start_calibration_serializa_con_device_index() {
        let json = serde_json::to_value(&SttInMessage::StartCalibration {
            device_index: Some(2),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "start_calibration", "device_index": 2})
        );

        let json_none = serde_json::to_value(&SttInMessage::StartCalibration {
            device_index: None,
        })
        .unwrap();
        assert_eq!(
            json_none,
            serde_json::json!({"type": "start_calibration", "device_index": null})
        );
    }

    #[test]
    fn stop_calibration_serializa_como_type_stop_calibration() {
        let json = serde_json::to_value(&SttInMessage::StopCalibration).unwrap();
        assert_eq!(json, serde_json::json!({"type": "stop_calibration"}));
    }

    #[test]
    fn calibration_ready_deserializa() {
        let msg: SttOutMessage =
            serde_json::from_value(serde_json::json!({"type": "calibration_ready"})).unwrap();
        assert!(matches!(msg, SttOutMessage::CalibrationReady));
    }

    #[test]
    fn devices_deserializa_con_lista_de_dispositivos() {
        let msg: SttOutMessage = serde_json::from_value(serde_json::json!({
            "type": "devices",
            "devices": [
                {
                    "index": 1,
                    "name": "Micrófono USB",
                    "max_input_channels": 2,
                    "default_sample_rate": 44100,
                    "is_default": true
                }
            ]
        }))
        .unwrap();
        match msg {
            SttOutMessage::Devices { devices } => {
                assert_eq!(devices.len(), 1);
                assert_eq!(devices[0].index, 1);
                assert_eq!(devices[0].name, "Micrófono USB");
                assert_eq!(devices[0].max_input_channels, 2);
                assert_eq!(devices[0].default_sample_rate, 44100);
                assert!(devices[0].is_default);
            }
            other => panic!("esperaba Devices, llegó {other:?}"),
        }
    }

    #[test]
    fn calibration_started_deserializa() {
        let msg: SttOutMessage = serde_json::from_value(serde_json::json!({
            "type": "calibration_started",
            "device_index": 1,
            "device_name": "Micrófono USB",
            "sample_rate": 16000
        }))
        .unwrap();
        match msg {
            SttOutMessage::CalibrationStarted {
                device_index,
                device_name,
                sample_rate,
            } => {
                assert_eq!(device_index, 1);
                assert_eq!(device_name, "Micrófono USB");
                assert_eq!(sample_rate, 16000);
            }
            other => panic!("esperaba CalibrationStarted, llegó {other:?}"),
        }
    }
}
