//! VAD + segmentación de frases. Puerto de
//! `_Engine.audio_loop`/`_finalize_utterance` (`workers/stt_engine.py`,
//! eliminado) sobre `sherpa_onnx::VoiceActivityDetector`.
//!
//! La API segura del crate solo expone un booleano (`detected()`), no la
//! probabilidad continua que usaba el motor Python, y no tiene pre-roll ni
//! histéresis de dos umbrales (`neg_threshold` desapareció de `VadConfig`,
//! ver el comentario ahí) — así que el pre-roll buffer y toda la máquina de
//! estados de segmentación (cuándo abrir/cerrar una frase, silencio
//! adaptativo, barge-in) se implementan acá en Rust puro, igual que antes.
//!
//! Se usan **dos** instancias de `VoiceActivityDetector` (creadas una sola
//! vez, cargar el modelo Silero ONNX es barato): `listening` con el umbral
//! normal y `speaking` con el umbral elevado de `barge_in.echo_guard`, para
//! que mientras Jarvis habla solo reaccione a voz sostenida y fuerte.

use std::collections::VecDeque;
use std::path::Path;
use std::time::Instant;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use crate::config::{BargeInConfig, VadConfig};
use crate::errors::SttError;
use crate::stt::capture::{AudioCapture, FRAME_SAMPLES, SAMPLE_RATE};
use crate::stt::SttMode;

/// Duración en ms de un frame — 512 muestras a 16kHz, exacto.
pub const FRAME_MS: u32 = (FRAME_SAMPLES as u32 * 1000) / SAMPLE_RATE;

pub fn rms_dbfs(samples: &[f32]) -> f32 {
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_sq / samples.len().max(1) as f32).sqrt() + 1e-9;
    20.0 * rms.log10()
}

/// Escucha `calibration_secs` de audio ambiente para fijar el piso de
/// energía por debajo del cual un segmento se descarta como ruido. Se llama
/// una sola vez al arrancar, antes del loop en tiempo real.
pub fn calibrate_energy_floor(capture: &mut AudioCapture, calibration_secs: f32) -> f32 {
    let n_frames = ((calibration_secs * SAMPLE_RATE as f32) / FRAME_SAMPLES as f32)
        .max(1.0) as usize;
    let sum: f32 = (0..n_frames)
        .map(|_| rms_dbfs(&capture.read_frame()))
        .sum();
    // Margen de 6dB sobre el ambiente medido: por debajo se considera ruido
    // de fondo, no habla.
    (sum / n_frames as f32) + 6.0
}

#[derive(Clone, Copy)]
pub enum DiscardReason {
    TooShort,
    BelowEnergyFloor,
}

pub enum SegmentEvent {
    VadStart {
        while_tts: bool,
    },
    /// Voz sostenida mientras Jarvis habla, ya confirmada como barge-in real
    /// (ver `BargeInConfig::min_speech_ms`) — se emite en cuanto se cumple,
    /// sin esperar a que la frase cierre.
    SpeechConfirmed,
    Closed {
        audio: Vec<f32>,
        speech_ms: u32,
        rms_dbfs: f32,
        while_tts: bool,
    },
    Discarded {
        reason: DiscardReason,
        speech_ms: u32,
        rms_dbfs: f32,
    },
}

enum RecordingState {
    Listening,
    Recording,
}

pub struct SpeechSegmenter {
    vad_listening: VoiceActivityDetector,
    vad_speaking: VoiceActivityDetector,
    min_speech_ms: u32,
    silence_long_ms: u32,
    silence_short_ms: u32,
    long_utterance_ms: u32,
    energy_floor_dbfs: f32,
    barge_in_min_speech_ms: u32,
    pre_roll_cap: usize,
    pre_roll: VecDeque<[f32; FRAME_SAMPLES]>,
    recording: Vec<[f32; FRAME_SAMPLES]>,
    speech_frames: u32,
    recording_state: RecordingState,
    recording_while_tts: bool,
    speech_confirmed_sent: bool,
    utterance_started_at: Instant,
    last_voiced_at: Instant,
}

impl SpeechSegmenter {
    pub fn new(
        vad_cfg: &VadConfig,
        barge_in: &BargeInConfig,
        vad_model_path: &Path,
        num_threads: i32,
        capture: &mut AudioCapture,
    ) -> Result<Self, SttError> {
        if !vad_model_path.exists() {
            return Err(SttError::ModelNotFound(vad_model_path.to_path_buf()));
        }
        let model_path = vad_model_path.to_string_lossy().into_owned();

        let make = |threshold: f32| -> Result<VoiceActivityDetector, SttError> {
            let config = VadModelConfig {
                silero_vad: SileroVadModelConfig {
                    model: Some(model_path.clone()),
                    threshold,
                    // Mínimos: el hangover/histéresis real lo maneja la
                    // máquina de estados de acá abajo (silence_long_ms/
                    // silence_short_ms), no el VAD.
                    min_silence_duration: 0.1,
                    min_speech_duration: 0.05,
                    window_size: FRAME_SAMPLES as i32,
                    max_speech_duration: 30.0,
                },
                ten_vad: Default::default(),
                sample_rate: SAMPLE_RATE as i32,
                num_threads,
                provider: Some("cpu".to_string()),
                debug: false,
            };
            VoiceActivityDetector::create(&config, 30.0)
                .ok_or_else(|| SttError::ModelLoad(format!("VAD Silero ({model_path})")))
        };

        let vad_listening = make(vad_cfg.threshold)?;
        let vad_speaking = make(barge_in.echo_guard.vad_threshold_while_speaking)?;

        let energy_floor_dbfs = match vad_cfg.energy_floor_dbfs {
            Some(v) => v,
            None => calibrate_energy_floor(capture, vad_cfg.calibration_secs),
        };

        let now = Instant::now();
        Ok(Self {
            vad_listening,
            vad_speaking,
            min_speech_ms: vad_cfg.min_speech_ms,
            silence_long_ms: vad_cfg.silence_long_ms,
            silence_short_ms: vad_cfg.silence_short_ms,
            long_utterance_ms: vad_cfg.long_utterance_ms,
            energy_floor_dbfs,
            barge_in_min_speech_ms: barge_in.min_speech_ms,
            pre_roll_cap: ((vad_cfg.pre_roll_ms / FRAME_MS) as usize).max(1),
            pre_roll: VecDeque::new(),
            recording: Vec::new(),
            speech_frames: 0,
            recording_state: RecordingState::Listening,
            recording_while_tts: false,
            speech_confirmed_sent: false,
            utterance_started_at: now,
            last_voiced_at: now,
        })
    }

    pub fn energy_floor_dbfs(&self) -> f32 {
        self.energy_floor_dbfs
    }

    /// `None` si el motor está `Suppressed` (no corre el VAD: sin costo de
    /// inferencia mientras el mic está silenciado). `Some(events)` con 0 o
    /// más eventos en cualquier otro modo.
    pub fn process_frame(&mut self, frame: [f32; FRAME_SAMPLES], mode: SttMode) -> FrameOutcome {
        if mode == SttMode::Suppressed {
            self.pre_roll.clear();
            self.recording_state = RecordingState::Listening;
            self.recording.clear();
            return FrameOutcome {
                speech_active: None,
                events: Vec::new(),
            };
        }

        let vad = if mode == SttMode::Speaking {
            &self.vad_speaking
        } else {
            &self.vad_listening
        };
        vad.accept_waveform(&frame);
        let speech_active = vad.detected();
        while !vad.is_empty() {
            vad.pop();
        }

        let now = Instant::now();
        let mut events = Vec::new();

        match self.recording_state {
            RecordingState::Listening => {
                self.pre_roll.push_back(frame);
                if self.pre_roll.len() > self.pre_roll_cap {
                    self.pre_roll.pop_front();
                }
                if speech_active {
                    self.recording_state = RecordingState::Recording;
                    self.recording = self.pre_roll.iter().copied().collect();
                    self.speech_frames = 1;
                    self.recording_while_tts = mode == SttMode::Speaking;
                    self.speech_confirmed_sent = false;
                    self.utterance_started_at = now;
                    self.last_voiced_at = now;
                    events.push(SegmentEvent::VadStart {
                        while_tts: self.recording_while_tts,
                    });
                }
            }
            RecordingState::Recording => {
                self.recording.push(frame);
                if speech_active {
                    self.speech_frames += 1;
                    self.last_voiced_at = now;
                }

                if self.recording_while_tts
                    && !self.speech_confirmed_sent
                    && self.speech_frames * FRAME_MS >= self.barge_in_min_speech_ms
                {
                    events.push(SegmentEvent::SpeechConfirmed);
                    self.speech_confirmed_sent = true;
                }

                let utterance_ms = now.duration_since(self.utterance_started_at).as_millis() as u32;
                let silence_needed_ms = if utterance_ms > self.long_utterance_ms {
                    self.silence_short_ms
                } else {
                    self.silence_long_ms
                };
                let silence_elapsed_ms = now.duration_since(self.last_voiced_at).as_millis() as u32;

                if silence_elapsed_ms >= silence_needed_ms {
                    events.push(self.finalize());
                    self.recording_state = RecordingState::Listening;
                    self.recording.clear();
                    self.pre_roll.clear();
                }
            }
        }

        FrameOutcome {
            speech_active: Some(speech_active),
            events,
        }
    }

    fn finalize(&mut self) -> SegmentEvent {
        let speech_ms = self.speech_frames * FRAME_MS;
        let audio: Vec<f32> = self.recording.iter().flatten().copied().collect();
        let rms = rms_dbfs(&audio);

        if speech_ms < self.min_speech_ms {
            return SegmentEvent::Discarded {
                reason: DiscardReason::TooShort,
                speech_ms,
                rms_dbfs: rms,
            };
        }
        if rms < self.energy_floor_dbfs {
            return SegmentEvent::Discarded {
                reason: DiscardReason::BelowEnergyFloor,
                speech_ms,
                rms_dbfs: rms,
            };
        }

        SegmentEvent::Closed {
            audio,
            speech_ms,
            rms_dbfs: rms,
            while_tts: self.recording_while_tts,
        }
    }
}

pub struct FrameOutcome {
    /// Señal de voz activa para este frame según el VAD del modo actual —
    /// `None` si el motor está suprimido (no se corrió el VAD). La consume
    /// `ClapDetector` (ver `src/stt/clap.rs`).
    pub speech_active: Option<bool>,
    pub events: Vec<SegmentEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_dbfs_of_silence_is_very_negative() {
        let silence = vec![0.0f32; FRAME_SAMPLES];
        assert!(rms_dbfs(&silence) < -100.0);
    }

    #[test]
    fn rms_dbfs_of_full_scale_square_wave_is_near_zero() {
        let full_scale: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let db = rms_dbfs(&full_scale);
        assert!((-0.5..0.5).contains(&db), "esperaba ~0dBFS, dio {db}");
    }

    #[test]
    fn frame_ms_matches_512_samples_at_16khz() {
        assert_eq!(FRAME_MS, 32);
    }
}
