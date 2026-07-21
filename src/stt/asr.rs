//! Wrapper sobre `sherpa_onnx::OfflineRecognizer` cargado con Whisper
//! (small, encoder fp32 + decoder int8) — reemplaza a faster-whisper.
//!
//! Se probó primero con NVIDIA Parakeet-TDT v3 (mejor WER en benchmarks
//! generales), pero ese modelo detecta el idioma automáticamente por audio
//! y el binding de sherpa-onnx no tiene forma de fijarlo: en frases muy
//! cortas sin contexto (el caso exacto de la wake word — "Jarvis" no es una
//! palabra real en ningún idioma) terminaba adivinando inglés en vez de
//! español, confirmado en pruebas reales. Whisper sí tiene un parámetro de
//! idioma explícito (`language: "es"`, fijo acá abajo), que es justamente el
//! mecanismo que usaba el motor Python anterior para lo mismo. La
//! alternativa de sesgar Parakeet hacia "jarvis" vía hotwords requiere
//! `modified_beam_search`, que está roto para este modelo en sherpa-onnx
//! 1.13.4 (~33% de alucinaciones/texto vacío incluso sin hotwords, ver
//! k2-fsa/sherpa-onnx#3267) — no es una alternativa viable hoy.

use std::path::Path;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineWhisperModelConfig};

use crate::errors::SttError;
use crate::stt::capture::SAMPLE_RATE;

pub struct Asr {
    recognizer: OfflineRecognizer,
}

impl Asr {
    /// `model_dir` debe contener `small-encoder.onnx`, `small-decoder.int8.onnx`
    /// y `small-tokens.txt` (el layout que publica k2-fsa para
    /// `sherpa-onnx-whisper-small`). `language` fuerza el idioma del decoder
    /// (`"es"` en producción, ver `SttConfig::language`) — sin esto, Whisper
    /// también intentaría detectar el idioma solo.
    pub fn new(
        model_dir: &Path,
        language: &str,
        provider: &str,
        num_threads: i32,
    ) -> Result<Self, SttError> {
        let encoder = model_dir.join("small-encoder.onnx");
        let decoder = model_dir.join("small-decoder.int8.onnx");
        let tokens = model_dir.join("small-tokens.txt");
        for path in [&encoder, &decoder, &tokens] {
            if !path.exists() {
                return Err(SttError::ModelNotFound(path.clone()));
            }
        }

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.whisper = OfflineWhisperModelConfig {
            encoder: Some(encoder.to_string_lossy().into_owned()),
            decoder: Some(decoder.to_string_lossy().into_owned()),
            language: Some(language.to_string()),
            task: Some("transcribe".to_string()),
            tail_paddings: 0,
            enable_token_timestamps: false,
            enable_segment_timestamps: false,
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.provider = Some(provider.to_string());
        config.model_config.num_threads = num_threads;

        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            SttError::ModelLoad(format!(
                "no se pudo crear OfflineRecognizer desde '{}'",
                model_dir.display()
            ))
        })?;

        Ok(Self { recognizer })
    }

    /// `audio` en f32 mono normalizado [-1.0, 1.0] a `SAMPLE_RATE` (16kHz).
    pub fn transcribe(&self, audio: &[f32]) -> String {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, audio);
        self.recognizer.decode(&stream);
        stream
            .get_result()
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default()
    }
}

/// Tests gateados por la presencia del modelo real (lo descarga
/// `scripts/setup.ps1`/`setup.sh`, ~640MB, no se versiona) — si no está, se
/// omiten con un aviso en vez de fallar, para no romper `cargo test` en una
/// máquina que todavía no corrió el setup. El audio de prueba en español sí
/// se versiona (`tests/fixtures/stt/es.wav`, 235KB — sale del tarball de
/// Parakeet-TDT, pero es un wav genérico, no depende de qué motor esté
/// activo).
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn default_model_dir() -> PathBuf {
        PathBuf::from("models/stt/sherpa-onnx-whisper-small")
    }

    fn fixture_wav() -> PathBuf {
        PathBuf::from("tests/fixtures/stt/es.wav")
    }

    fn resample_all_at_once(samples: &[f32], src_rate: i32, dst_rate: i32) -> Vec<f32> {
        if src_rate == dst_rate {
            return samples.to_vec();
        }
        let r = sherpa_onnx::LinearResampler::create(src_rate, dst_rate)
            .expect("crear resampler");
        r.resample(samples, true)
    }

    /// Remuestrea en chunks de `FRAME_SAMPLES` (512), igual que
    /// `AudioCapture::read_frame` hace en producción — usa la misma
    /// instancia de `LinearResampler` a lo largo de todos los chunks, así
    /// que mantiene estado continuo entre llamadas.
    fn resample_chunked(samples: &[f32], src_rate: i32, dst_rate: i32) -> Vec<f32> {
        if src_rate == dst_rate {
            return samples.to_vec();
        }
        let r = sherpa_onnx::LinearResampler::create(src_rate, dst_rate)
            .expect("crear resampler");
        let mut out = Vec::with_capacity(samples.len());
        let mut chunks = samples.chunks(512).peekable();
        while let Some(chunk) = chunks.next() {
            out.extend(r.resample(chunk, chunks.peek().is_none()));
        }
        out
    }

    /// `None` si el modelo no está descargado (test se omite). Devuelve el
    /// audio de prueba tal cual viene en el wav (a su sample rate nativo,
    /// no necesariamente 16kHz) — cada test decide cómo remuestrearlo.
    fn load_asr_and_raw_sample() -> Option<(Asr, i32, Vec<f32>)> {
        let model_dir = default_model_dir();
        if !model_dir.join("small-encoder.onnx").exists() {
            eprintln!(
                "modelo STT no encontrado en {} (corré scripts/setup.ps1 o setup.sh) — \
                 se omite este test",
                model_dir.display()
            );
            return None;
        }
        let asr = Asr::new(&model_dir, "es", "cpu", 2).expect("cargar Whisper");
        let wav_path = fixture_wav();
        let wave = sherpa_onnx::Wave::read(wav_path.to_str().expect("ruta utf-8"))
            .unwrap_or_else(|| panic!("leer {}", wav_path.display()));
        Some((asr, wave.sample_rate(), wave.samples().to_vec()))
    }

    #[test]
    fn transcribes_bundled_spanish_sample() {
        let Some((asr, wave_rate, raw)) = load_asr_and_raw_sample() else {
            return;
        };
        let samples = resample_all_at_once(&raw, wave_rate, SAMPLE_RATE as i32);
        let text = asr.transcribe(&samples);
        assert!(
            !text.trim().is_empty(),
            "la transcripción del wav de prueba no debería estar vacía"
        );
    }

    /// Regresión directa del bug raíz identificado en el motor anterior
    /// (`workers/stt_engine.py::_read_frame`, ya eliminado): re-muestreaba
    /// cada frame de forma independiente con `scipy.signal.resample`, sin
    /// continuidad entre frames, generando artefactos cuando el micrófono no
    /// era nativamente 16kHz. `tests/fixtures/stt/es.wav` viene a 22050Hz
    /// (no 16kHz), así que sirve tal cual para comparar remuestrear todo de
    /// una vez contra remuestrear en chunks de 512 muestras (como hace
    /// `AudioCapture::read_frame` en producción) — con un resampler con
    /// estado continuo como `LinearResampler`, ambos caminos deberían
    /// producir una transcripción equivalente; con el bug original no.
    #[test]
    fn continuous_resampling_preserves_intelligibility() {
        let Some((asr, wave_rate, raw)) = load_asr_and_raw_sample() else {
            return;
        };
        let direct = resample_all_at_once(&raw, wave_rate, SAMPLE_RATE as i32);
        let chunked = resample_chunked(&raw, wave_rate, SAMPLE_RATE as i32);

        let direct_text = asr.transcribe(&direct);
        assert!(!direct_text.trim().is_empty());
        let chunked_text = asr.transcribe(&chunked);
        assert!(
            !chunked_text.trim().is_empty(),
            "la transcripción remuestreada en chunks no debería estar vacía"
        );

        let direct_words: std::collections::HashSet<&str> =
            direct_text.split_whitespace().collect();
        let chunked_words: std::collections::HashSet<&str> =
            chunked_text.split_whitespace().collect();
        let overlap = direct_words.intersection(&chunked_words).count();
        let expected = ((direct_words.len() as f32) * 0.5).ceil() as usize;
        assert!(
            overlap >= expected,
            "se esperaban al menos {expected} palabras en común entre remuestrear todo junto \
             y en chunks, hubo {overlap} (todo junto: {direct_text:?}, en chunks: {chunked_text:?})"
        );
    }
}
