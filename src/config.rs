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
    pub wake: WakeConfig,
    pub barge_in: BargeInConfig,
    pub llm: LlmConfig,
    pub tts: TtsConfig,
    pub audio: AudioConfig,
    pub pipeline: PipelineConfig,
    pub agent: AgentConfig,
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workers: WorkersConfig::default(),
            stt: SttConfig::default(),
            wake: WakeConfig::default(),
            barge_in: BargeInConfig::default(),
            llm: LlmConfig::default(),
            tts: TtsConfig::default(),
            audio: AudioConfig::default(),
            pipeline: PipelineConfig::default(),
            agent: AgentConfig::default(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttEngineKind {
    /// Motor propio: PyAudio + Silero VAD + faster-whisper directo.
    #[default]
    Native,
    /// RealtimeSTT — camino de respaldo si el motor nativo falla.
    Realtimestt,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    /// Probabilidad de Silero a partir de la cual se considera que empezó a hablar.
    pub threshold: f32,
    /// Probabilidad de Silero por debajo de la cual se considera que dejó de hablar
    /// (histéresis: menor que `threshold` para que las micro-pausas no corten).
    pub neg_threshold: f32,
    /// Audio previo a la detección de voz que se antepone al buffer, para no
    /// perder el inicio de la frase.
    pub pre_roll_ms: u32,
    /// Duración mínima de voz detectada para considerarla habla real (filtra blips).
    pub min_speech_ms: u32,
    /// Silencio requerido para cerrar la frase mientras dura menos de `long_utterance_ms`.
    pub silence_long_ms: u32,
    /// Silencio requerido para cerrar la frase una vez superado `long_utterance_ms`.
    pub silence_short_ms: u32,
    /// A partir de esta duración de locución, se exige `silence_short_ms` en vez
    /// de `silence_long_ms` para cerrar la frase.
    pub long_utterance_ms: u32,
    /// Piso de energía (dBFS) por debajo del cual se descarta como ruido.
    /// null = se calibra al arrancar midiendo el ambiente.
    pub energy_floor_dbfs: Option<f32>,
    /// Segundos de calibración del piso de energía al arrancar.
    pub calibration_secs: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            neg_threshold: 0.35,
            pre_roll_ms: 400,
            min_speech_ms: 250,
            silence_long_ms: 800,
            silence_short_ms: 450,
            long_utterance_ms: 2500,
            energy_floor_dbfs: None,
            calibration_secs: 1.5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SttFiltersConfig {
    /// Se descarta la transcripción si `no_speech_prob` de Whisper la supera.
    pub max_no_speech_prob: f32,
    /// Se descarta la transcripción si `avg_logprob` de Whisper cae por debajo.
    pub min_avg_logprob: f32,
    /// Se descarta la transcripción si `compression_ratio` de Whisper la supera
    /// (indicio de texto repetitivo/alucinado).
    pub max_compression_ratio: f32,
}

impl Default for SttFiltersConfig {
    fn default() -> Self {
        Self {
            max_no_speech_prob: 0.6,
            min_avg_logprob: -1.0,
            max_compression_ratio: 2.4,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// native (motor propio) | realtimestt (respaldo).
    pub engine: SttEngineKind,
    pub vad: VadConfig,
    pub filters: SttFiltersConfig,
    pub language: String,
    /// "auto" | cuda | cpu — override manual de la detección automática de hardware.
    pub device: String,
    /// "auto" | tiny | base | small | medium | large-v2 | large-v3-turbo.
    /// Con "auto", el worker calibra midiendo la velocidad real de la máquina
    /// (una sola vez, se cachea en workers/.cache/stt_profile.json).
    pub whisper_model: String,
    /// "auto" | float16 | int8 | ...
    pub compute_type: String,
    pub input_device_index: Option<u32>,
    /// null = elegido por la calibración (5 con máquina holgada, 3 si va justa).
    pub beam_size: Option<u8>,
    /// null = auto (~núcleos físicos). Fija OMP_NUM_THREADS para ctranslate2,
    /// que por defecto usa solo 4 hilos.
    pub cpu_threads: Option<u8>,
    /// Contexto en español para el decoder de Whisper — mejora la precisión.
    pub initial_prompt: String,
    /// true = ignora el caché de calibración y vuelve a medir en este arranque.
    pub recalibrate: bool,
    /// Las siguientes claves solo se usan con `engine: realtimestt` (camino de
    /// respaldo) — el motor nativo tiene sus propios parámetros bajo `vad`.
    pub silero_sensitivity: f32,
    pub webrtc_sensitivity: u8,
    pub post_speech_silence_duration: f32,
    /// Segundos mínimos de grabación para considerarla habla válida: filtra
    /// blips muy cortos que Whisper convertiría en alucinaciones.
    pub min_length_of_recording: f32,
    /// Segundos mínimos entre grabaciones: evita grabaciones fantasma
    /// consecutivas.
    pub min_gap_between_recordings: f32,
    /// true = usa Silero también para detectar el fin del habla (más robusto
    /// que solo el silencio; reduce cortes espurios que Whisper rellenaría con
    /// alucinaciones).
    pub silero_deactivity_detection: bool,
    /// Segundos que el worker de STT tolera trabado en un estado ocupado
    /// ("recording"/"transcribing", nunca deberían tardar más de unos pocos
    /// segundos) antes de asumir que quedó irrecuperablemente colgado y
    /// forzar su propia salida (para que Rust lo reinicie). Aplica a ambos
    /// motores.
    pub stuck_state_timeout_secs: u64,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: SttEngineKind::default(),
            vad: VadConfig::default(),
            filters: SttFiltersConfig::default(),
            language: "es".to_string(),
            device: "auto".to_string(),
            whisper_model: "auto".to_string(),
            compute_type: "auto".to_string(),
            input_device_index: None,
            beam_size: None,
            cpu_threads: None,
            initial_prompt: "Conversación en español con un asistente de voz llamado \
                Jarvis. El usuario a veces lo llama por su nombre: Jarvis."
                .to_string(),
            recalibrate: false,
            silero_sensitivity: 0.4,
            webrtc_sensitivity: 3,
            post_speech_silence_duration: 0.6,
            min_length_of_recording: 1.0,
            min_gap_between_recordings: 1.0,
            silero_deactivity_detection: true,
            stuck_state_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WakeConfig {
    /// false = sin gate: Jarvis responde a todo lo que transcribe.
    pub enabled: bool,
    /// Palabras que activan una respuesta (se matchean normalizadas y con
    /// tolerancia a 1 letra de error de transcripción).
    pub words: Vec<String>,
    /// Segundos tras la última respuesta durante los que Jarvis sigue atento
    /// y responde sin necesidad de repetir el nombre.
    pub attention_window_secs: u64,
    /// Dentro de la ventana de atención, ignora frases con menos de esta
    /// cantidad de palabras y sin el nombre: las alucinaciones de Whisper en
    /// silencio son casi siempre de una sola palabra ("bip", "bien"), los
    /// comandos reales son multi-palabra. 1 = sin filtro.
    pub window_min_words: usize,
    /// Frases-basura típicas de Whisper en silencio/ruido: si la transcripción
    /// normalizada coincide con alguna, se descarta por completo (ni siquiera
    /// se guarda como contexto ambiental).
    pub ignore_phrases: Vec<String>,
    /// true = las frases ignoradas se anteponen como contexto a la siguiente
    /// consulta real del usuario.
    pub ambient_context: bool,
    pub ambient_context_max: usize,
    pub ambient_context_ttl_secs: u64,
}

impl Default for WakeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            words: vec!["jarvis".to_string()],
            attention_window_secs: 25,
            window_min_words: 2,
            ignore_phrases: [
                "gracias",
                "muchas gracias",
                "gracias por ver el video",
                "subtitulos realizados por la comunidad de amara org",
                "suscribete",
                "hasta la proxima",
            ]
            .map(String::from)
            .to_vec(),
            ambient_context: true,
            ambient_context_max: 5,
            ambient_context_ttl_secs: 120,
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
    LmStudio,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
    /// Solo para modelos con razonamiento (qwen3, deepseek-r1): `false`
    /// desactiva los tokens de "pensamiento", que de otro modo el TTS
    /// hablaría en voz alta. null = no enviar el campo (obligatorio para
    /// modelos que no lo soportan, como qwen2.5 — Ollama rechaza la request).
    pub think: Option<bool>,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_string(),
            model: "qwen2.5:7b".to_string(),
            think: None,
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

/// LM Studio expone un servidor local compatible con la API de OpenAI (ver
/// `llm::lmstudio`). A diferencia de los proveedores de nube normalmente no
/// requiere API key, por eso `api_key_env` es opcional.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LmStudioConfig {
    pub base_url: String,
    /// Placeholder deliberado: si no calza con lo cargado, el preflight
    /// (`check_lmstudio`) lista los modelos que LM Studio sí tiene.
    pub model: String,
    /// null = sin autenticación (caso normal). Some = exige esa variable de
    /// entorno, igual que los proveedores de nube.
    pub api_key_env: Option<String>,
}

impl Default for LmStudioConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:1234/v1".to_string(),
            model: "local-model".to_string(),
            api_key_env: None,
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
    pub lmstudio: LmStudioConfig,
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
            lmstudio: LmStudioConfig::default(),
            system_prompt: "Eres Jarvis, un asistente de voz conversacional en español. \
                Estás hablando en voz alta, no escribiendo texto: nunca uses markdown \
                (nada de **, #, guiones de lista, bloques de código ni links). Respondé \
                de forma breve y natural, como en una charla — una o dos oraciones como \
                máximo, salvo que te pidan explícitamente más detalle o una explicación larga. \
                Dispones de herramientas para consultar el sistema y controlar la computadora: \
                úsalas cuando la petición lo requiera y nunca inventes datos del sistema ni \
                resultados que no obtuviste. Para mostrar un sitio web usa open_url, que abre el \
                navegador por defecto; nunca uses run_powershell ni Start-Process para abrir URLs, \
                ni abras el navegador como app para luego navegar. Para abrir programas usa \
                open_app; si una app no abre, díselo al usuario en vez de reintentar con \
                run_powershell. Usa run_powershell solo para tareas sin herramienta dedicada, y \
                siempre incluye el campo summary con una descripción breve y natural de lo que \
                hace. Antes de usar herramientas puedes decir UNA frase muy corta tipo 'Déjame \
                comprobarlo, señor', pero jamás describas la herramienta, sus parámetros, URLs, \
                rutas ni comandos técnicos en voz alta. Tras recibir resultados, responde con lo \
                esencial en una o dos frases; nunca leas listas largas, datos crudos ni JSON. Si \
                una acción es riesgosa, el sistema pedirá la confirmación por su cuenta: no la \
                pidas tú ni la menciones."
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
    Cartesia,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PiperConfig {
    pub voice_path: PathBuf,
    pub config_path: PathBuf,
    pub use_cuda: bool,
    /// null = usa el length_scale propio de la voz (1.0). <1 = más rápido,
    /// >1 = más lento.
    pub length_scale: Option<f32>,
    /// null = usa el noise_w_scale propio de la voz. Controla cuánto varía
    /// la duración entre fonemas: un poco más que el default de la voz suena
    /// menos monótono/robótico.
    pub noise_w_scale: Option<f32>,
}

impl Default for PiperConfig {
    fn default() -> Self {
        Self {
            voice_path: PathBuf::from("voices/es_ES-davefx-medium.onnx"),
            config_path: PathBuf::from("voices/es_ES-davefx-medium.onnx.json"),
            use_cuda: false,
            length_scale: None,
            noise_w_scale: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CartesiaTransport {
    #[default]
    Rest,
    WebSocket,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CartesiaOutputFormat {
    pub container: String,
    pub encoding: String,
    pub sample_rate: u32,
}

impl Default for CartesiaOutputFormat {
    fn default() -> Self {
        Self {
            container: "raw".to_string(),
            encoding: "pcm_s16le".to_string(),
            sample_rate: 22050,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CartesiaConfig {
    pub model_id: String,
    pub voice_id: String,
    pub language: Option<String>,
    pub output_format: CartesiaOutputFormat,
    pub api_key_env: String,
    pub cartesia_version: String,
    pub transport: CartesiaTransport,
}

impl Default for CartesiaConfig {
    fn default() -> Self {
        Self {
            model_id: "sonic-3.5".to_string(),
            voice_id: String::new(),
            language: Some("es".to_string()),
            output_format: CartesiaOutputFormat::default(),
            api_key_env: "CARTESIA_API_KEY".to_string(),
            cartesia_version: "2026-03-01".to_string(),
            transport: CartesiaTransport::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub provider: TtsProviderKind,
    pub piper: PiperConfig,
    pub elevenlabs: ElevenLabsConfig,
    pub cartesia: CartesiaConfig,
    pub synth_timeout_secs: u64,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            provider: TtsProviderKind::default(),
            piper: PiperConfig::default(),
            elevenlabs: ElevenLabsConfig::default(),
            cartesia: CartesiaConfig::default(),
            synth_timeout_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub output_device: Option<String>,
    pub volume: f32,
    /// Límite de seguridad para esperar a que el buffer de reproducción se
    /// vacíe. Solo actúa si el dispositivo de salida se cuelga o el sistema
    /// suspende el stream (ej. PC inactiva); no debería afectar respuestas
    /// normales.
    pub drain_timeout_secs: u64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            output_device: None,
            volume: 1.0,
            drain_timeout_secs: 60,
        }
    }
}

/// Capa agéntica: herramientas que Jarvis puede ejecutar (consultar el
/// sistema, controlar la PC, etc.) con confirmación por voz para acciones
/// riesgosas.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// false = comportamiento clásico: chat puro sin herramientas.
    pub enabled: bool,
    /// Máximo de pasadas LLM→tools por turno antes de forzar una respuesta.
    pub max_iterations: usize,
    pub tool_timeout_secs: u64,
    /// Segundos que Jarvis espera un "sí"/"no" (o el código) tras pedir
    /// confirmación antes de cancelar la acción.
    pub confirm_timeout_secs: u64,
    /// Truncado de cada resultado de herramienta antes de dárselo al LLM.
    pub max_tool_result_chars: usize,
    /// Frases enlatadas que Jarvis dice mientras ejecuta herramientas si el
    /// modelo no emitió su propio preámbulo.
    pub filler_phrases: Vec<String>,
    /// Nombres de herramientas a excluir del registro.
    pub disabled_tools: Vec<String>,
    /// Palabras/frases cortas que cuentan como confirmación afirmativa.
    pub confirm_yes: Vec<String>,
    pub confirm_no: Vec<String>,
    /// Código de aceptación de riesgos para acciones de nivel extremo. Se
    /// verifica en Rust contra la transcripción; NUNCA se pasa al LLM.
    pub risk_code: String,
    /// Regex adicionales (se suman a los defaults) que elevan un comando de
    /// PowerShell a nivel de riesgo extremo (requiere el código).
    pub high_risk_patterns: Vec<String>,
    pub files: FilesToolConfig,
    pub apps: AppsConfig,
    pub web: WebToolConfig,
    pub memory: MemoryConfig,
    pub translate: TranslateConfig,
    pub reminders: RemindersConfig,
    pub scripted_tools: ScriptedToolsConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_iterations: 6,
            tool_timeout_secs: 20,
            confirm_timeout_secs: 30,
            max_tool_result_chars: 3000,
            filler_phrases: vec![
                "Déjame revisar, señor.".to_string(),
                "Un momento, señor.".to_string(),
                "Enseguida lo compruebo, señor.".to_string(),
            ],
            disabled_tools: Vec::new(),
            confirm_yes: [
                "sí",
                "si",
                "claro",
                "adelante",
                "hazlo",
                "confirmo",
                "dale",
                "por supuesto",
                "sí señor",
                "procede",
                "afirmativo",
                "correcto",
            ]
            .map(String::from)
            .to_vec(),
            confirm_no: [
                "no", "cancela", "cancelar", "espera", "mejor no", "detente", "para", "negativo",
            ]
            .map(String::from)
            .to_vec(),
            risk_code: "0201".to_string(),
            high_risk_patterns: Vec::new(),
            files: FilesToolConfig::default(),
            apps: AppsConfig::default(),
            web: WebToolConfig::default(),
            memory: MemoryConfig::default(),
            translate: TranslateConfig::default(),
            reminders: RemindersConfig::default(),
            scripted_tools: ScriptedToolsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TranslateConfig {
    /// Idioma destino usado si el LLM no especifica `target_lang`.
    pub default_target_lang: String,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self {
            default_target_lang: "es".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RemindersConfig {
    /// Ruta del archivo SQLite de recordatorios.
    pub db_path: PathBuf,
    /// Cada cuántos segundos el poller revisa recordatorios vencidos.
    pub poll_interval_secs: u64,
    /// Tope de recordatorios activos simultáneos.
    pub max_active: usize,
}

impl Default for RemindersConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("data/reminders.db"),
            poll_interval_secs: 20,
            max_active: 50,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ScriptedToolsConfig {
    /// Ruta del archivo SQLite de tools personalizadas (separado de
    /// memory.db: datos de mayor riesgo, conviene poder borrarlos aparte).
    pub db_path: PathBuf,
    /// Tope de tools personalizadas simultáneas.
    pub max_tools: usize,
    /// Timeout de las recetas HTTP.
    pub http_timeout_secs: u64,
    /// Hosts permitidos para recetas HTTP; vacío = sin restricción.
    pub allowed_hosts: Vec<String>,
}

impl Default for ScriptedToolsConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("data/scripted_tools.db"),
            max_tools: 20,
            http_timeout_secs: 15,
            allowed_hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Ruta del archivo SQLite de memoria persistente.
    pub db_path: PathBuf,
    /// Cuántas memorias recientes inyectar en el system prompt de cada turno.
    pub max_injected: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("data/memory.db"),
            max_injected: 12,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WebToolConfig {
    /// Truncado del texto extraído de una página antes de dárselo al LLM
    /// (un 7B con historial digiere bien ~4000 chars).
    pub max_page_chars: usize,
    pub max_results: usize,
    pub user_agent: String,
}

impl Default for WebToolConfig {
    fn default() -> Self {
        Self {
            max_page_chars: 4000,
            max_results: 5,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                (KHTML, like Gecko) Chrome/126.0 Safari/537.36"
                .to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FilesToolConfig {
    /// Carpetas donde busca `find_files` cuando no hay Everything CLI.
    pub search_roots: Vec<PathBuf>,
    pub max_results: usize,
    /// Ruta a es.exe (Everything CLI) para búsqueda instantánea; null = walkdir.
    pub everything_cli: Option<PathBuf>,
}

impl Default for FilesToolConfig {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        Self {
            search_roots: vec![home],
            max_results: 20,
            everything_cli: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct AppsConfig {
    /// Alias hablado → comando/ejecutable real ("navegador" → "chrome").
    pub aliases: std::collections::HashMap<String, String>,
    /// Carpetas extra donde `open_app` busca accesos directos/ejecutables,
    /// además del Menú Inicio y el Escritorio (p.ej. apps portables).
    pub extra_search_roots: Vec<PathBuf>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BargeInMode {
    /// Solo interrumpe si la transcripción capturada mientras Jarvis habla
    /// contiene el wake word — fiable con altavoces (sin AEC, el eco no
    /// suele incluir el nombre a menos que Jarvis lo diga).
    #[default]
    WakeWord,
    /// Interrumpe con cualquier voz sostenida, sin exigir el nombre —
    /// recomendado solo con auriculares (con altavoces, el eco puede
    /// disparar falsos positivos incluso con el echo guard).
    AnyVoice,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EchoGuardConfig {
    pub enabled: bool,
    /// Fracción de tokens de la transcripción que deben solapar con frases
    /// TTS recientes para descartarla como eco propio.
    pub similarity_threshold: f32,
    /// Umbral de Silero para entrar en "recording" mientras Jarvis habla
    /// (más alto que `stt.vad.threshold`: filtra ruido/eco de fondo, solo
    /// reacciona a voz sostenida y relativamente fuerte).
    pub vad_threshold_while_speaking: f32,
    /// Cuánto se conservan las frases dichas por Jarvis para comparar contra
    /// transcripciones que llegan poco después de terminar de hablar.
    pub recent_tts_window_secs: u64,
}

impl Default for EchoGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            similarity_threshold: 0.72,
            vad_threshold_while_speaking: 0.75,
            recent_tts_window_secs: 12,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BargeInConfig {
    pub enabled: bool,
    pub mode: BargeInMode,
    /// Milisegundos de voz sostenida (modo `speaking` del motor STT nativo)
    /// para confirmar una interrupción real y no un ruido puntual.
    pub min_speech_ms: u32,
    pub echo_guard: EchoGuardConfig,
}

impl Default for BargeInConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: BargeInMode::WakeWord,
            min_speech_ms: 400,
            echo_guard: EchoGuardConfig::default(),
        }
    }
}
