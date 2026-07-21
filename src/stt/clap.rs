//! Detector de doble aplauso sobre el audio crudo del micrófono. Puerto 1:1
//! de `workers/clap_detector.py` (ver ese archivo, eliminado, para el
//! razonamiento original) — corre frame a frame (512 samples @16kHz = 32ms)
//! en el mismo hilo que el VAD, aritmética barata sin asignaciones.
//!
//! Único cambio respecto del original: `prob: Option<f32>` (probabilidad
//! continua de Silero) pasa a ser `speech_active: Option<bool>`, porque la
//! API segura de `sherpa_onnx::VoiceActivityDetector` solo expone un
//! booleano (ver `src/stt/vad.rs`). Un aplauso dura 1-2 frames (32-64ms), muy
//! por debajo de `VadConfig::min_speech_ms`, así que el VAD casi nunca lo
//! marca como voz sostenida — la semántica práctica no cambia.

use std::time::Instant;

use crate::config::ClapConfig;

fn frame_metrics(frame: &[f32]) -> (f32, f32) {
    let sum_sq: f32 = frame.iter().map(|s| s * s).sum();
    let rms = (sum_sq / frame.len() as f32).sqrt() + 1e-9;
    let rms_db = 20.0 * rms.log10();

    let zcr = if frame.len() > 1 {
        let crossings = frame
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f32 / (frame.len() - 1) as f32
    } else {
        0.0
    };

    (rms_db, zcr)
}

pub struct ClapDetector {
    config: ClapConfig,
    bg_db: f32,
    decaying_since: Option<Instant>,
    decaying_threshold: f32,
    lockout_until: Option<Instant>,
    refractory_until: Option<Instant>,
    first_clap_at: Option<Instant>,
}

impl ClapDetector {
    pub fn new(config: ClapConfig) -> Self {
        let bg_db = config.min_peak_dbfs - config.min_rise_db;
        Self {
            config,
            bg_db,
            decaying_since: None,
            decaying_threshold: -120.0,
            lockout_until: None,
            refractory_until: None,
            first_clap_at: None,
        }
    }

    /// `speech_active`: señal del VAD para este mismo frame (ver módulo).
    /// `true` si este frame confirma un doble aplauso.
    pub fn process(&mut self, frame: &[f32], speech_active: Option<bool>) -> bool {
        let now = Instant::now();
        let (rms_db, zcr) = frame_metrics(frame);

        if self.decaying_since.is_some() {
            return self.check_decay(now, rms_db);
        }

        if self.lockout_until.is_some_and(|t| now < t)
            || self.refractory_until.is_some_and(|t| now < t)
        {
            return false;
        }

        let threshold = self.config.min_peak_dbfs.max(self.bg_db + self.config.min_rise_db);
        let onset = rms_db >= threshold
            && zcr >= self.config.min_zcr
            && !(self.config.reject_if_speech_active && speech_active.unwrap_or(false));

        if onset {
            tracing::debug!(rms_db, zcr, threshold, "aplauso: posible onset");
            self.decaying_since = Some(now);
            self.decaying_threshold = threshold;
            return false;
        }

        // Frame tranquilo (o sostenido sin llegar a onset): actualiza el
        // fondo siempre, sin importar si está arriba o abajo del actual — ver
        // el comentario original en clap_detector.py sobre por qué solo
        // actualizar en frames "bajos" dejaba el umbral de decaimiento
        // inalcanzable tras una racha de silencio real.
        self.bg_db += 0.05 * (rms_db - self.bg_db);
        false
    }

    fn check_decay(&mut self, now: Instant, rms_db: f32) -> bool {
        if rms_db < self.decaying_threshold {
            self.decaying_since = None;
            // Ventana muerta corta: absorbe el eco/reverberación del mismo
            // golpe sin interferir con el gap real entre aplausos.
            self.lockout_until = Some(now + std::time::Duration::from_millis(80));
            return self.confirm_clap(now);
        }

        let elapsed_ms = now
            .duration_since(self.decaying_since.expect("decaying_since set"))
            .as_secs_f32()
            * 1000.0;
        if elapsed_ms >= self.config.decay_ms as f32 {
            // La energía se sostuvo más de la cuenta (voz, música): no es un
            // aplauso, se descarta con un lockout corto.
            self.decaying_since = None;
            self.lockout_until = Some(now + std::time::Duration::from_millis(300));
        }
        false
    }

    fn confirm_clap(&mut self, now: Instant) -> bool {
        tracing::debug!("aplauso: confirmado (1 golpe)");
        let Some(first) = self.first_clap_at else {
            self.first_clap_at = Some(now);
            return false;
        };

        let gap_ms = now.duration_since(first).as_secs_f32() * 1000.0;
        if gap_ms < self.config.double_min_gap_ms as f32 {
            // Probable reverb del mismo golpe: se ignora sin resetear la
            // espera del segundo aplauso real.
            return false;
        }
        if gap_ms > self.config.double_max_gap_ms as f32 {
            // Se pasó la ventana: este aplauso pasa a ser el nuevo "primero".
            self.first_clap_at = Some(now);
            return false;
        }

        tracing::debug!("aplauso: doble confirmado");
        self.first_clap_at = None;
        self.refractory_until =
            Some(now + std::time::Duration::from_millis(self.config.refractory_ms as u64));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    const FRAME_LEN: usize = 512;

    /// Onda cuadrada: ZCR≈1.0 (timbre de banda ancha) y RMS = amplitud,
    /// deterministico y sin depender de una fuente de ruido real.
    fn loud_frame() -> Vec<f32> {
        (0..FRAME_LEN)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect()
    }

    fn quiet_frame() -> Vec<f32> {
        vec![0.0; FRAME_LEN]
    }

    #[test]
    fn silence_never_confirms() {
        let mut det = ClapDetector::new(ClapConfig::default());
        for _ in 0..20 {
            assert!(!det.process(&quiet_frame(), Some(false)));
        }
    }

    #[test]
    fn single_clap_does_not_confirm_double_alone() {
        let mut det = ClapDetector::new(ClapConfig::default());
        assert!(!det.process(&loud_frame(), Some(false)));
        // El frame de silencio siguiente hace decaer el onset: confirma un
        // único golpe (log interno), pero no el evento de doble aplauso.
        assert!(!det.process(&quiet_frame(), Some(false)));
        for _ in 0..10 {
            assert!(!det.process(&quiet_frame(), Some(false)));
        }
    }

    #[test]
    fn double_clap_confirms_within_gap_window() {
        let mut det = ClapDetector::new(ClapConfig::default());
        assert!(!det.process(&loud_frame(), Some(false)));
        assert!(!det.process(&quiet_frame(), Some(false)));

        // Dentro de [double_min_gap_ms, double_max_gap_ms] (150-900ms default).
        sleep(Duration::from_millis(300));

        assert!(!det.process(&loud_frame(), Some(false)));
        assert!(det.process(&quiet_frame(), Some(false)));
    }

    #[test]
    fn rejects_onset_while_speech_active() {
        let mut det = ClapDetector::new(ClapConfig::default());
        assert!(!det.process(&loud_frame(), Some(true)));
        // Sin onset registrado, el frame de silencio tampoco debe confirmar nada.
        assert!(!det.process(&quiet_frame(), Some(true)));
    }
}
