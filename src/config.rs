//! Configuración de la aplicación, cargada desde `config.yaml`.
//!
//! Todas las claves son opcionales: lo que falte en el YAML se completa con
//! los valores por defecto de cada sección (vía `#[serde(default)]` a nivel
//! de contenedor + `impl Default`).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::errors::ConfigError;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub workers: WorkersConfig,
    pub stt: SttConfig,
    pub llm: LlmConfig,
    pub tts: TtsConfig,
    pub audio: AudioConfig,
    pub pipeline: PipelineConfig,
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workers: WorkersConfig::default(),
            stt: SttConfig::default(),
            llm: LlmConfig::default(),
            tts: TtsConfig::default(),
            audio: AudioConfig::default(),
            pipeline: PipelineConfig::default(),
            log_level: "info".to_string(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_path_buf()));
        }
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        serde_saphyr::from_str(&raw).map_err(|e| ConfigError::Parse(e.to_string()))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkersConfig {
    pub python_executable: PathBuf,
    pub stt_script: PathBuf,
    pub tts_script: PathBuf,
    pub stt_init_timeout_secs: u64,
    pub tts_init_timeout_secs: u64,
    pub shutdown_timeout_secs: u64,
    pub restart_on_crash: bool,
    pub max_restarts: u32,
}

impl Default for WorkersConfig {
    fn default() -> Self {
        let python_executable = if cfg!(windows) {
            PathBuf::from("workers/.venv/Scripts/python.exe")
        } else {
            PathBuf::from("workers/.venv/bin/python")
        };
        Self {
            python_executable,
            stt_script: PathBuf::from("workers/stt_worker.py"),
            tts_script: PathBuf::from("workers/tts_worker.py"),
            stt_init_timeout_secs: 60,
            tts_init_timeout_secs: 20,
            shutdown_timeout_secs: 3,
            restart_on_crash: true,
            max_restarts: 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    pub language: String,
    /// "auto" | cuda | cpu — override manual de la detección automática de hardware.
    pub device: String,
    /// "auto" | tiny | base | small | medium | large-v2
    pub whisper_model: String,
    /// "auto" | float16 | int8 | ...
    pub compute_type: String,
    pub input_device_index: Option<u32>,
    pub silero_sensitivity: f32,
    pub webrtc_sensitivity: u8,
    pub post_speech_silence_duration: f32,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            language: "es".to_string(),
            device: "auto".to_string(),
            whisper_model: "auto".to_string(),
            compute_type: "auto".to_string(),
            input_device_index: None,
            silero_sensitivity: 0.4,
            webrtc_sensitivity: 3,
            post_speech_silence_duration: 0.6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProviderKind {
    #[default]
    Ollama,
    Anthropic,
    Openai,
    Deepseek,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_string(),
            model: "qwen2.5:7b".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AnthropicConfig {
    pub model: String,
    pub api_key_env: String,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-5".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenAiConfig {
    pub model: String,
    pub api_key_env: String,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4o-mini".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
        }
    }
}

/// La API de DeepSeek es compatible con el formato de OpenAI (ver `llm::deepseek`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DeepSeekConfig {
    pub model: String,
    pub api_key_env: String,
}

impl Default for DeepSeekConfig {
    fn default() -> Self {
        Self {
            model: "deepseek-v4-flash".to_string(),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub provider: LlmProviderKind,
    pub ollama: OllamaConfig,
    pub anthropic: AnthropicConfig,
    pub openai: OpenAiConfig,
    pub deepseek: DeepSeekConfig,
    pub system_prompt: String,
    pub max_history_messages: usize,
    pub request_timeout_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: LlmProviderKind::default(),
            ollama: OllamaConfig::default(),
            anthropic: AnthropicConfig::default(),
            openai: OpenAiConfig::default(),
            deepseek: DeepSeekConfig::default(),
            system_prompt: "Eres Jarvis, un asistente de voz conversacional en español. \
                Estás hablando en voz alta, no escribiendo texto: nunca uses markdown \
                (nada de **, #, guiones de lista, bloques de código ni links). Respondé \
                de forma breve y natural, como en una charla — una o dos oraciones como \
                máximo, salvo que te pidan explícitamente más detalle o una explicación larga."
                .to_string(),
            max_history_messages: 20,
            request_timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsProviderKind {
    #[default]
    Piper,
    Elevenlabs,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PiperConfig {
    pub voice_path: PathBuf,
    pub config_path: PathBuf,
    pub use_cuda: bool,
}

impl Default for PiperConfig {
    fn default() -> Self {
        Self {
            voice_path: PathBuf::from("voices/es_MX-claude-high.onnx"),
            config_path: PathBuf::from("voices/es_MX-claude-high.onnx.json"),
            use_cuda: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ElevenLabsConfig {
    pub voice_id: String,
    pub model_id: String,
    /// Formato pedido a la API, ej. "pcm_22050"/"pcm_44100" -- debe ser un
    /// formato "pcm_*" (PCM crudo), no mp3/opus, para evitar depender de
    /// ffmpeg para decodificar la respuesta.
    pub output_format: String,
    pub api_key_env: String,
}

impl Default for ElevenLabsConfig {
    fn default() -> Self {
        Self {
            voice_id: String::new(),
            model_id: "eleven_multilingual_v2".to_string(),
            output_format: "pcm_22050".to_string(),
            api_key_env: "ELEVENLABS_API_KEY".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub provider: TtsProviderKind,
    pub piper: PiperConfig,
    pub elevenlabs: ElevenLabsConfig,
    pub synth_timeout_secs: u64,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            provider: TtsProviderKind::default(),
            piper: PiperConfig::default(),
            elevenlabs: ElevenLabsConfig::default(),
            synth_timeout_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub output_device: Option<String>,
    pub volume: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            output_device: None,
            volume: 1.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PipelineConfig {
    pub max_phrase_chars: usize,
    pub min_phrase_chars: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_phrase_chars: 220,
            min_phrase_chars: 15,
        }
    }
}
