//! Captura de micrófono + resampling continuo a 16kHz mono.
//!
//! Mismo patrón que `src/audio/player.rs` (salida) pero en el sentido
//! contrario: el callback de tiempo real de `cpal` (vía el reexport
//! `rodio::cpal`, ver el comentario en `Cargo.toml` sobre por qué no se
//! declara `cpal` como dependencia directa) empuja muestras a un
//! `ringbuf::HeapRb`; el hilo de procesamiento de STT (`engine.rs`, no
//! async) las saca del otro lado con `read_frame`, bloqueante con reintentos
//! cortos.
//!
//! El resampling usa `sherpa_onnx::LinearResampler`, que mantiene estado
//! *entre* llamadas — a diferencia del bug original (`scipy.signal.resample`
//! por frame independiente en `workers/stt_engine.py::_read_frame`, ver el
//! plan), acá no hay discontinuidades en los bordes de cada frame.

use std::collections::VecDeque;
use std::time::Duration;

use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::cpal::SampleFormat;
use sherpa_onnx::LinearResampler;

use crate::errors::SttError;

pub const SAMPLE_RATE: u32 = 16000;
/// Tamaño de frame que exige Silero VAD a 16kHz (32ms) — igual que el motor
/// Python anterior.
pub const FRAME_SAMPLES: usize = 512;

const RING_BUFFER_SECONDS: usize = 2;

pub struct AudioCapture {
    consumer: HeapCons<f32>,
    native_scratch: Vec<f32>,
    resampler: Option<LinearResampler>,
    /// Muestras a 16kHz ya remuestreadas pero todavía no entregadas como
    /// frame completo.
    pending: VecDeque<f32>,
    device_name: String,
    native_sample_rate: u32,
    _stream: rodio::cpal::Stream,
}

impl AudioCapture {
    pub fn new(device_index: Option<u32>) -> Result<Self, SttError> {
        let host = rodio::cpal::default_host();
        let device = match device_index {
            Some(idx) => host
                .input_devices()
                .map_err(|e| SttError::Backend(e.to_string()))?
                .nth(idx as usize)
                .ok_or(SttError::NoInputDevice)?,
            None => host
                .default_input_device()
                .ok_or(SttError::NoInputDevice)?,
        };
        let device_name = device.name().unwrap_or_else(|_| "?".to_string());

        let supported = device
            .default_input_config()
            .map_err(|e| SttError::Backend(e.to_string()))?;
        let sample_format = supported.sample_format();
        let native_sample_rate = supported.sample_rate().0;
        let channels = supported.channels();
        let config = supported.config();

        let ring_capacity = native_sample_rate as usize * RING_BUFFER_SECONDS;
        let ring = HeapRb::<f32>::new(ring_capacity);
        let (producer, consumer) = ring.split();

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, channels, producer)?,
            SampleFormat::I16 => build_stream::<i16>(&device, &config, channels, producer)?,
            other => {
                return Err(SttError::Backend(format!(
                    "el micrófono '{device_name}' entrega muestras en formato {other:?}; \
                     Jarvis solo sabe capturar f32 o i16"
                )))
            }
        };
        stream.play().map_err(|e| SttError::Backend(e.to_string()))?;

        let resampler = if native_sample_rate != SAMPLE_RATE {
            Some(
                LinearResampler::create(native_sample_rate as i32, SAMPLE_RATE as i32).ok_or_else(
                    || {
                        SttError::Backend(format!(
                            "no se pudo crear el resampler {native_sample_rate}Hz -> {SAMPLE_RATE}Hz"
                        ))
                    },
                )?,
            )
        } else {
            None
        };

        tracing::info!(
            device = %device_name,
            native_sample_rate,
            channels,
            resampling = resampler.is_some(),
            "micrófono listo"
        );

        Ok(Self {
            consumer,
            native_scratch: vec![0.0; FRAME_SAMPLES.max(native_sample_rate as usize / 50)],
            resampler,
            pending: VecDeque::with_capacity(FRAME_SAMPLES * 2),
            device_name,
            native_sample_rate,
            _stream: stream,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn native_sample_rate(&self) -> u32 {
        self.native_sample_rate
    }

    /// Bloquea (con reintentos cortos) hasta juntar un frame completo de
    /// `FRAME_SAMPLES` muestras mono a 16kHz.
    pub fn read_frame(&mut self) -> [f32; FRAME_SAMPLES] {
        while self.pending.len() < FRAME_SAMPLES {
            let popped = self.consumer.pop_slice(&mut self.native_scratch);
            if popped == 0 {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            let native_chunk = &self.native_scratch[..popped];
            match &self.resampler {
                Some(r) => self.pending.extend(r.resample(native_chunk, false)),
                None => self.pending.extend(native_chunk.iter().copied()),
            }
        }
        let mut frame = [0.0f32; FRAME_SAMPLES];
        for sample in &mut frame {
            *sample = self.pending.pop_front().expect("pending >= FRAME_SAMPLES");
        }
        frame
    }
}

/// Construye el stream de entrada para un formato de muestra concreto,
/// bajando a mono (promedio de canales) antes de empujar al ring buffer —
/// el resto del pipeline (VAD, aplausos, ASR) siempre trabaja en mono.
fn build_stream<S>(
    device: &rodio::cpal::Device,
    config: &rodio::cpal::StreamConfig,
    channels: u16,
    mut producer: HeapProd<f32>,
) -> Result<rodio::cpal::Stream, SttError>
where
    S: rodio::cpal::SizedSample + ToMonoSample + Send + 'static,
{
    let channels = channels.max(1) as usize;
    device
        .build_input_stream(
            config,
            move |data: &[S], _: &rodio::cpal::InputCallbackInfo| {
                for frame in data.chunks_exact(channels) {
                    let sum: f32 = frame.iter().map(|s| s.to_mono_f32()).sum();
                    let _ = producer.try_push(sum / channels as f32);
                }
            },
            |error| tracing::error!(%error, "error del stream de micrófono"),
            None,
        )
        .map_err(|e| SttError::Backend(e.to_string()))
}

/// Conversión a f32 normalizado en [-1.0, 1.0] para los formatos de muestra
/// que `AudioCapture` soporta.
trait ToMonoSample {
    fn to_mono_f32(&self) -> f32;
}

impl ToMonoSample for f32 {
    fn to_mono_f32(&self) -> f32 {
        *self
    }
}

impl ToMonoSample for i16 {
    fn to_mono_f32(&self) -> f32 {
        *self as f32 / i16::MAX as f32
    }
}
