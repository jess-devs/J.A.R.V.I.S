//! Reproducción de audio no bloqueante: un ring buffer SPSC (`ringbuf`) hace
//! de puente entre las tareas async que producen audio (el pipeline de
//! streaming) y el callback en tiempo real de `cpal`, que corre en su propio
//! hilo dedicado y nunca debe bloquearse esperando al runtime de tokio.
//!
//! El stream de salida se construye una sola vez, al arrancar, usando la
//! configuración *nativa* que reporta el dispositivo (`default_output_config`)
//! — no la del audio de Piper. En Windows, WASAPI en modo compartido no
//! resamplea ni remezcla canales por vos: dispositivos comunes (salidas
//! virtuales de auriculares gaming, HDMI, etc.) exigen un sample rate y una
//! cantidad de canales específicos (ej. 96 kHz / 8 canales), y pedir un
//! stream mono a 22050 Hz ahí falla con "Stream configuration is not
//! supported in shared mode". Por eso cada chunk se resamplea (interpolación
//! lineal) y se remezcla (mono -> N canales) al formato del dispositivo antes
//! de empujarlo al ring buffer.

use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapProd;
use ringbuf::HeapRb;

use crate::errors::AudioError;
use crate::tts::AudioChunk;

/// ~2s de margen al sample rate/canales de salida — de sobra para el patrón
/// de uso (frases cortas sintetizadas y reproducidas casi de inmediato).
const RING_BUFFER_SECONDS: usize = 2;

pub struct AudioPlayer {
    volume: f32,
    output_sample_rate: u32,
    output_channels: u16,
    producer: HeapProd<f32>,
    _stream: cpal::Stream,
}

impl AudioPlayer {
    pub fn new(output_device: Option<&str>, volume: f32) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = match output_device {
            Some(name) => host
                .output_devices()
                .map_err(|e| AudioError::Backend(e.to_string()))?
                .find(|d| d.to_string() == name)
                .ok_or(AudioError::NoOutputDevice)?,
            None => host
                .default_output_device()
                .ok_or(AudioError::NoOutputDevice)?,
        };

        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Backend(e.to_string()))?;

        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(AudioError::Backend(format!(
                "el dispositivo de salida '{device}' espera muestras en formato {:?}; \
                 por ahora Jarvis solo sabe reproducir en f32",
                supported.sample_format()
            )));
        }

        let output_sample_rate = supported.sample_rate();
        let output_channels = supported.channels();
        let config = supported.config();

        let ring_capacity =
            output_sample_rate as usize * output_channels as usize * RING_BUFFER_SECONDS;
        let ring = HeapRb::<f32>::new(ring_capacity);
        let (producer, mut consumer) = ring.split();

        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let filled = consumer.pop_slice(data);
                    for sample in &mut data[filled..] {
                        *sample = 0.0;
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
            device = %device,
            sample_rate = output_sample_rate,
            channels = output_channels,
            "dispositivo de audio de salida listo"
        );

        Ok(Self {
            volume,
            output_sample_rate,
            output_channels,
            producer,
            _stream: stream,
        })
    }

    /// Resamplea/remezcla el chunk al formato nativo del dispositivo y lo
    /// empuja al ring buffer. No bloquea esperando a que termine de sonar —
    /// solo espera si el buffer está lleno (backpressure).
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

        let mut remaining: &[f32] = &samples;
        while !remaining.is_empty() {
            let pushed = self.producer.push_slice(remaining);
            remaining = &remaining[pushed..];
            if !remaining.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        Ok(())
    }

    /// Espera hasta que el ring buffer se vacíe (toda la respuesta terminó
    /// de reproducirse, con un margen residual de la latencia física de la
    /// tarjeta de sonido).
    pub async fn wait_until_drained(&self) {
        while !self.producer.is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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

/// Resampling por interpolación lineal. Suficiente para voz a esta escala —
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
