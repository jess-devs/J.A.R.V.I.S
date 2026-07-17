// El audio se reproduce sin bloquear la interfaz.
// Hay dos mundos separados: las tareas que generan el sonido (asíncronas)
// y el sistema de audio del dispositivo (que no puede esperar).
// Se comunican a través de un "tubo" (ringbuf) que permite pasar los datos sin detener a ninguno.

// Al iniciar el programa se abre el dispositivo de audio con su configuración original (la que el propio Windows da por defecto).
// No se usa la del audio de Piper, porque en Windows WASAPI compartido no convierte formatos por ti:
// si el dispositivo pide 96 kHz y 8 canales, tienes que darle justo eso.
// Por eso, antes de enviar el sonido por el tubo, se ajusta la frecuencia (remuestreo)
// y se duplican los canales (de mono a los que necesite el dispositivo).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapProd;
use ringbuf::HeapRb;
use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::errors::AudioError;
use crate::tts::AudioChunk;

const RING_BUFFER_SECONDS: usize = 2;

pub struct AudioPlayer {
    volume: f32,
    output_sample_rate: u32,
    output_channels: u16,
    producer: HeapProd<f32>,
    drain_timeout: Duration,
    stop_flag: Arc<AtomicBool>,
    _stream: rodio::cpal::Stream,
    /// `true` mientras el callback de audio (tiempo real) está sacando
    /// muestras reales del buffer, en vez de rellenar con silencio. Ver
    /// `PlaybackMeter`.
    speaking_flag: Arc<AtomicBool>,
    /// RMS (0.0-1.0 aprox.) de lo que el callback de audio mandó a la tarjeta
    /// de sonido en su última invocación, codificado con `f32::to_bits` (no
    /// existe `AtomicF32` en `std`). Ver `PlaybackMeter`.
    level_bits: Arc<AtomicU32>,
}

/// Handle liviano y clonable hacia el nivel de reproducción en tiempo real,
/// escrito directamente desde el callback de audio de `cpal` (no desde
/// `play_chunk`, que solo encola — ver el comentario en `AudioPlayer::new`).
/// Pensado para que la TUI anime `JarvisSpeaking` pegado a lo que realmente
/// suena, no a lo que ya se encoló para sonar.
#[derive(Clone)]
pub struct PlaybackMeter {
    speaking: Arc<AtomicBool>,
    level_bits: Arc<AtomicU32>,
}

impl PlaybackMeter {
    /// `true` si en el último callback de audio se mandaron muestras reales
    /// a la tarjeta de sonido (no silencio de relleno).
    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::Relaxed)
    }

    /// RMS (0.0-1.0 aprox.) de lo que sonó en el último callback de audio.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Relaxed))
    }
}

impl AudioPlayer {
    pub fn new(
        output_device: Option<&str>,
        volume: f32,
        drain_timeout_secs: u64,
    ) -> Result<Self, AudioError> {
        let host = rodio::cpal::default_host();
        let device = match output_device {
            Some(name) => host
                .output_devices()
                .map_err(|e| AudioError::Backend(e.to_string()))?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                .ok_or(AudioError::NoOutputDevice)?,
            None => host
                .default_output_device()
                .ok_or(AudioError::NoOutputDevice)?,
        };
        let device_name = device.name().unwrap_or_else(|_| "?".to_string());

        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Backend(e.to_string()))?;

        if supported.sample_format() != rodio::cpal::SampleFormat::F32 {
            return Err(AudioError::Backend(format!(
                "el dispositivo de salida '{device_name}' espera muestras en formato {:?}; \
                 por ahora Jarvis solo sabe reproducir en f32",
                supported.sample_format()
            )));
        }

        let output_sample_rate = supported.sample_rate().0;
        let output_channels = supported.channels();
        let config = supported.config();

        let ring_capacity =
            output_sample_rate as usize * output_channels as usize * RING_BUFFER_SECONDS;
        let ring = HeapRb::<f32>::new(ring_capacity);
        let (producer, mut consumer) = ring.split();

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_cb = stop_flag.clone();
        let speaking_flag = Arc::new(AtomicBool::new(false));
        let speaking_flag_cb = speaking_flag.clone();
        let level_bits = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let level_bits_cb = level_bits.clone();

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &rodio::cpal::OutputCallbackInfo| {
                    if stop_flag_cb.swap(false, Ordering::AcqRel) {
                        let mut scratch = [0.0f32; 1024];
                        while consumer.pop_slice(&mut scratch) > 0 {}
                    }
                    let filled = consumer.pop_slice(data);
                    for sample in &mut data[filled..] {
                        *sample = 0.0;
                    }
                    // Única fuente de verdad de "está sonando algo ahora
                    // mismo": esto es lo que de verdad se le mandó a la
                    // tarjeta de sonido en esta invocación, no lo que se
                    // encoló hace rato (`play_chunk` puede ir hasta
                    // `RING_BUFFER_SECONDS` adelantado). Operaciones
                    // atómicas puras, sin locks ni allocations — seguras acá.
                    speaking_flag_cb.store(filled > 0, Ordering::Relaxed);
                    if filled > 0 {
                        level_bits_cb.store(rms(&data[..filled]).to_bits(), Ordering::Relaxed);
                    }
                },
                |error| tracing::error!(%error, "error del stream de audio"),
                None,
            )
            .map_err(|e| AudioError::Backend(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::Backend(e.to_string()))?;

        tracing::info!(
            device = %device_name,
            sample_rate = output_sample_rate,
            channels = output_channels,
            "dispositivo de audio de salida listo"
        );

        Ok(Self {
            volume,
            output_sample_rate,
            output_channels,
            producer,
            drain_timeout: Duration::from_secs(drain_timeout_secs),
            stop_flag,
            _stream: stream,
            speaking_flag,
            level_bits,
        })
    }

    /// Handle hacia el nivel de reproducción en tiempo real (ver
    /// `PlaybackMeter`), para que la TUI anime `JarvisSpeaking` pegado a lo
    /// que realmente suena.
    pub fn playback_meter(&self) -> PlaybackMeter {
        PlaybackMeter {
            speaking: self.speaking_flag.clone(),
            level_bits: self.level_bits.clone(),
        }
    }

    /// Corta de inmediato lo que esté sonando: no espera a que termine de
    /// reproducirse, arma la bandera que el callback de audio consume en su
    /// próxima invocación (siguiente buffer, unos pocos milisegundos). No
    /// bloquea al hilo de tiempo real.
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
    }

    /// Resamplea/remezcla el chunk al formato nativo del dispositivo y lo
    /// empuja al ring buffer. No bloquea esperando a que termine de sonar
    /// solo espera si el buffer está lleno (backpressure).
    ///
    /// Lleva un `timeout`: sin él, un dispositivo de salida colgado (stream
    /// suspendido por el SO, driver caído) dejaría este loop esperando para
    /// siempre justo antes de reactivar el micrófono, el callback de error
    /// de `cpal` (línea de `build_output_stream` arriba) solo loguea, no
    /// desbloquea nada.
    pub async fn play_chunk(&mut self, chunk: &AudioChunk) -> Result<(), AudioError> {
        if chunk.sample_width != 2 {
            return Err(AudioError::Backend(format!(
                "formato de audio no soportado: sample_width={} (solo PCM de 16 bits)",
                chunk.sample_width
            )));
        }

        let mono = pcm_i16_to_mono_f32(&chunk.pcm, chunk.channels, self.volume);
        let resampled = resample_linear(&mono, chunk.sample_rate, self.output_sample_rate);
        let samples = upmix_mono_to_channels(&resampled, self.output_channels);

        let push = async {
            let mut remaining: &[f32] = &samples;
            while !remaining.is_empty() {
                let pushed = self.producer.push_slice(remaining);
                remaining = &remaining[pushed..];
                if !remaining.is_empty() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
        };
        tokio::time::timeout(self.drain_timeout, push)
            .await
            .map_err(|_| AudioError::PlaybackStalled(self.drain_timeout.as_secs()))
    }

    /// Espera hasta que el ring buffer se vacíe (toda la respuesta terminó
    /// de reproducirse, con un margen residual de la latencia física de la
    /// tarjeta de sonido). Ver nota de timeout en `play_chunk`.
    pub async fn wait_until_drained(&self) -> Result<(), AudioError> {
        let drain = async {
            while !self.producer.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(self.drain_timeout, drain)
            .await
            .map_err(|_| AudioError::PlaybackStalled(self.drain_timeout.as_secs()))
    }
}

/// Decodifica PCM s16le entrelazado a mono f32 en [-1,1], promediando
/// canales si la fuente no es mono (Piper siempre es mono; queda general por
/// si algún proveedor TTS futuro no lo es).
fn pcm_i16_to_mono_f32(pcm: &[u8], channels: u16, volume: f32) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    let frame_bytes = 2 * channels;
    pcm.chunks_exact(frame_bytes)
        .map(|frame| {
            let sum: i32 = frame
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as i32)
                .sum();
            let avg = sum as f32 / channels as f32;
            (avg / i16::MAX as f32) * volume
        })
        .collect()
}

/// Resampling por interpolación lineal. Suficiente para voz a esta escala,
/// un resampler con banda limitada (ej. crate `rubato`) sería mejor
/// fidelidad, pero es una dependencia extra que no hace falta para el MVP.
fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = dst_rate as f64 / src_rate as f64;
    let out_len = ((input.len() as f64) * ratio).round().max(1.0) as usize;
    let last = input.len() - 1;
    let mut output = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = (src_pos.floor() as usize).min(last);
        let frac = (src_pos - idx as f64) as f32;
        let a = input[idx];
        let b = input[(idx + 1).min(last)];
        output.push(a + (b - a) * frac);
    }
    output
}

/// RMS simple de un buffer en [-1,1], usado solo como nivel visual (ver
/// `PlaybackMeter`) — no afecta la reproducción. Se llama desde el callback
/// de audio sobre las muestras que de verdad se mandan a la tarjeta de
/// sonido (posiblemente multicanal entrelazado); es un proxy de energía, no
/// hace falta bajar a mono para eso.
fn rms(samples: &[f32]) -> f32 {
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Duplica cada muestra mono en las `channels` del dispositivo (entrelazado).
fn upmix_mono_to_channels(mono: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return mono.to_vec();
    }
    let mut out = Vec::with_capacity(mono.len() * channels);
    for &sample in mono {
        for _ in 0..channels {
            out.push(sample);
        }
    }
    out
}
