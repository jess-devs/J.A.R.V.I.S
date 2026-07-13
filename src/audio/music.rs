//! Música de fondo del modo bienvenida (rodio, stream de salida independiente
//! del `AudioPlayer` de la voz de Jarvis — ver el módulo hermano `player.rs`).
//! `rodio` trae su propio `cpal` interno; dos streams WASAPI compartidos
//! conviven sin problema (riesgo aceptado: dos copias de `cpal` en el árbol
//! de dependencias, sin conflicto real de tipos).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Mutex;

use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

use crate::config::WelcomeConfig;
use crate::errors::AudioError;

/// Parte compartible (`Send + Sync`, vive en `Arc`) entre el `Orchestrator`
/// y el tool `stop_music`: solo lo que hace falta para controlar la
/// reproducción en curso, sin el `OutputStream` (`!Send`).
pub struct MusicShared {
    sink: Mutex<Option<Sink>>,
    base_volume: f32,
    duck_volume: f32,
}

impl MusicShared {
    fn new(base_volume: f32, duck_volume: f32) -> Self {
        Self {
            sink: Mutex::new(None),
            base_volume,
            duck_volume,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.sink.lock().unwrap().is_some()
    }

    pub fn stop(&self) {
        if let Some(sink) = self.sink.lock().unwrap().take() {
            sink.stop();
        }
    }

    /// Baja el volumen mientras se habla. Asignación directa de volumen (sin
    /// contador de referencias): solapar con otro duck/unduck es inofensivo.
    pub fn duck(&self) {
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink.set_volume(self.duck_volume);
        }
    }

    pub fn unduck(&self) {
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink.set_volume(self.base_volume);
        }
    }
}

/// Dueño del `OutputStream` de rodio (`!Send`, como `cpal::Stream` en
/// `AudioPlayer`) — vive en el `Orchestrator`, nunca cruza un `.await` que
/// requiera `Send`. `MusicShared` es la parte que sí se comparte.
pub struct MusicPlayer {
    stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    shared: std::sync::Arc<MusicShared>,
}

impl MusicPlayer {
    /// Lazy: no abre ningún dispositivo hasta el primer `play_file`.
    pub fn new(cfg: &WelcomeConfig) -> Self {
        Self {
            stream: None,
            handle: None,
            shared: std::sync::Arc::new(MusicShared::new(cfg.music_volume, cfg.duck_volume)),
        }
    }

    pub fn shared(&self) -> std::sync::Arc<MusicShared> {
        self.shared.clone()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.is_playing()
    }

    pub fn stop(&self) {
        self.shared.stop();
    }

    pub fn duck(&self) {
        self.shared.duck();
    }

    pub fn unduck(&self) {
        self.shared.unduck();
    }

    /// Reproduce `path` en loop no aplica acá (una sola pasada, como la
    /// escena original) sobre el mismo dispositivo que `output_device`
    /// (nombre de `config.audio.output_device`) cuando se puede resolver,
    /// para no separar la voz de Jarvis y la música en hardware distinto.
    pub fn play_file(
        &mut self,
        path: &Path,
        output_device: Option<&str>,
    ) -> Result<(), AudioError> {
        if self.handle.is_none() {
            let (stream, handle) = open_output_stream(output_device)?;
            self.stream = Some(stream);
            self.handle = Some(handle);
        }
        let handle = self
            .handle
            .as_ref()
            .expect("acabamos de inicializar el stream de música");

        let file = File::open(path).map_err(|e| {
            AudioError::Backend(format!(
                "no se pudo abrir el mp3 de bienvenida '{}': {e}",
                path.display()
            ))
        })?;
        let source = Decoder::new(BufReader::new(file)).map_err(|e| {
            AudioError::Backend(format!(
                "no se pudo decodificar el mp3 de bienvenida '{}': {e}",
                path.display()
            ))
        })?;

        let sink = Sink::try_new(handle).map_err(|e| {
            AudioError::Backend(format!("no se pudo crear el sink de música: {e}"))
        })?;
        sink.set_volume(self.shared.base_volume);
        sink.append(source);

        *self.shared.sink.lock().unwrap() = Some(sink);
        Ok(())
    }
}

fn open_output_stream(
    output_device: Option<&str>,
) -> Result<(OutputStream, OutputStreamHandle), AudioError> {
    let host = rodio::cpal::default_host();
    let device = output_device.and_then(|name| {
        host.output_devices()
            .ok()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
    });

    let result = match device {
        Some(device) => OutputStream::try_from_device(&device),
        None => OutputStream::try_default(),
    };
    result.map_err(|e| AudioError::Backend(format!("no se pudo abrir el dispositivo de música: {e}")))
}
