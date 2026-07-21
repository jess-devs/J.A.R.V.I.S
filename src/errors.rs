//! Jerarquía de errores del proyecto. Cada variante lleva un mensaje accionable
//! en español, pensado para mostrarse directo al usuario sin traceback.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, JarvisError>;

#[derive(Debug, thiserror::Error)]
pub enum JarvisError {
    #[error("configuración: {0}")]
    Config(#[from] ConfigError),

    #[error("preflight: {0}")]
    Preflight(String),

    #[error("STT: {0}")]
    Stt(#[from] SttError),

    #[error("TTS: {0}")]
    Tts(#[from] TtsError),

    #[error("proveedor LLM: {0}")]
    Llm(#[from] LlmError),

    #[error("herramienta: {0}")]
    Tool(#[from] ToolError),

    #[error("audio: {0}")]
    Audio(#[from] AudioError),

    #[error("pipeline: {0}")]
    Pipeline(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no se encontró el archivo de configuración '{0}'. Copiá config.example.yaml a config.yaml si aún no existe")]
    NotFound(PathBuf),

    #[error("no se pudo leer '{path}': {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config.yaml inválido: {0}")]
    Parse(String),
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("no se pudo iniciar el proceso Python ({executable:?}): {source}")]
    Spawn {
        executable: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("el worker no respondió 'ready' dentro de {0} segundos")]
    InitTimeout(u64),

    #[error("error de protocolo hablando con el worker: {0}")]
    Protocol(String),

    #[error("el worker terminó inesperadamente (código de salida: {0:?})")]
    Crashed(Option<i32>),

    #[error("error fatal reportado por el worker [{code}]: {message}")]
    Fatal { code: String, message: String },

    #[error("tiempo de espera agotado ({0}s) esperando una respuesta del worker")]
    Timeout(u64),
}

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error(
        "no se encontró el modelo de reconocimiento en '{0}'. Corré scripts/setup.ps1 \
         (o scripts/setup.sh) para descargarlo"
    )]
    ModelNotFound(PathBuf),

    #[error("no se pudo cargar el modelo de reconocimiento de voz: {0}")]
    ModelLoad(String),

    #[error("no se encontró ningún micrófono disponible")]
    NoInputDevice,

    #[error("error del backend de audio: {0}")]
    Backend(String),

    #[error("el motor STT no quedó listo dentro de {0} segundos")]
    InitTimeout(u64),

    #[error("el motor STT terminó inesperadamente{}", .0.as_ref().map(|m| format!(": {m}")).unwrap_or_default())]
    Crashed(Option<String>),
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("no se pudo conectar a Ollama en {base_url}. ¿Está corriendo `ollama serve`?")]
    OllamaUnreachable { base_url: String },

    #[error("falta la variable de entorno {0}. Definila en tu .env")]
    MissingApiKey(String),

    #[error("error de red hablando con el proveedor LLM: {0}")]
    Network(#[from] reqwest::Error),

    #[error("respuesta inesperada del proveedor LLM: {0}")]
    UnexpectedResponse(String),
}

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("falta la variable de entorno {0}. Definila en tu .env")]
    MissingApiKey(String),

    #[error("worker de síntesis: {0}")]
    Worker(#[from] WorkerError),

    #[error("error de red hablando con el proveedor TTS: {0}")]
    Network(#[from] reqwest::Error),

    #[error("respuesta inesperada del proveedor TTS: {0}")]
    UnexpectedResponse(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("argumentos inválidos: {0}")]
    InvalidArgs(String),

    #[error("la herramienta tardó más de {0} segundos y se canceló")]
    Timeout(u64),

    #[error("{0}")]
    Execution(String),
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no se encontró ningún dispositivo de salida de audio")]
    NoOutputDevice,

    #[error("error del backend de audio: {0}")]
    Backend(String),

    #[error("la reproducción de audio no avanzó en {0} segundos (¿el dispositivo de salida dejó de responder?)")]
    PlaybackStalled(u64),
}
