//! Servidor HTTP local (127.0.0.1 únicamente) de la página de configuración
//! local de Jarvis: sirve el frontend compilado (`web/config-ui/dist`) y una
//! API de lectura/escritura sobre el mismo `config.yaml` que usa el resto de
//! la aplicación.
//!
//! No comparte estado en memoria con el `Orchestrator` en ejecución: cada
//! request relee/reescribe el archivo directamente. El proceso de Jarvis que
//! ya está corriendo sigue usando la copia de `Config` que cargó al
//! arrancar — los cambios hechos acá piden reiniciar Jarvis para aplicarse
//! (ver brief de la página de configuración). Esto evita tener que enhebrar
//! un `Config` compartido y mutable a través del `Orchestrator`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower_http::services::ServeDir;

use crate::config::{
    AgentConfig, AudioConfig, BargeInConfig, Config, LlmConfig, LlmProviderKind,
    McpServerConfig, PipelineConfig, SttConfig, TtsConfig, TtsProviderKind, WakeConfig,
    WelcomeConfig, WorkersConfig,
};
use crate::errors::ConfigError;

#[derive(Clone)]
struct AppState {
    config_path: PathBuf,
}

/// Arranca el servidor y corre para siempre (o hasta que el proceso
/// termine). Pensado para spawnearse como una tarea de fondo desde `main`;
/// un error de bind se loguea y la tarea simplemente termina, sin tirar
/// abajo el resto de Jarvis.
pub async fn serve(config_path: PathBuf, addr: SocketAddr, static_dir: PathBuf) {
    let state = AppState { config_path };

    let api = Router::new()
        .route("/api/config/llm", get(get_llm).put(put_llm))
        .route("/api/config/tts", get(get_tts).put(put_tts))
        .route("/api/status/llm", get(status_llm))
        .route("/api/status/tts", get(status_tts))
        .route("/api/config/workers", get(get_workers).put(put_workers))
        .route("/api/config/stt", get(get_stt).put(put_stt))
        .route("/api/config/wake", get(get_wake).put(put_wake))
        .route("/api/config/barge_in", get(get_barge_in).put(put_barge_in))
        .route("/api/config/audio", get(get_audio).put(put_audio))
        .route("/api/config/pipeline", get(get_pipeline).put(put_pipeline))
        .route("/api/config/welcome", get(get_welcome).put(put_welcome))
        .route("/api/status/welcome", get(status_welcome))
        .route("/api/config/mcp", get(get_mcp).put(put_mcp))
        .route("/api/config/agent", get(get_agent).put(put_agent))
        .with_state(state);

    let app = if static_dir.is_dir() {
        api.fallback_service(ServeDir::new(&static_dir))
    } else {
        tracing::warn!(
            dir = %static_dir.display(),
            "el frontend de la página de configuración no está compilado (falta `npm run build` en web/config-ui/); solo la API va a responder"
        );
        api
    };

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                error = %e,
                %addr,
                "no se pudo levantar el servidor de la página de configuración"
            );
            return;
        }
    };

    tracing::info!(%addr, "página de configuración local disponible");

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "el servidor de la página de configuración terminó con error");
    }
}

// ---------------------------------------------------------------------
// Errores de la API: cualquier ConfigError se traduce a una respuesta JSON.
// ---------------------------------------------------------------------

struct ApiError(StatusCode, String);

impl From<ConfigError> for ApiError {
    fn from(e: ConfigError) -> Self {
        let status = match &e {
            ConfigError::NotFound(_) => StatusCode::NOT_FOUND,
            ConfigError::Parse(_) => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError(status, e.to_string())
    }
}

impl ApiError {
    /// Guarda rechazada por falta/error del código de aceptación actual
    /// (ver `put_agent`) — mismo espíritu que la confirmación hablada de
    /// las acciones de riesgo `Code`.
    fn forbidden(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::FORBIDDEN, msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn load(path: &Path) -> Result<Config, ApiError> {
    Config::load(path).map_err(ApiError::from)
}

fn save(config: &Config, path: &Path) -> Result<(), ApiError> {
    config.save(path).map_err(ApiError::from)
}

// ---------------------------------------------------------------------
// Sección llm
// ---------------------------------------------------------------------

async fn get_llm(State(state): State<AppState>) -> Result<Json<LlmConfig>, ApiError> {
    Ok(Json(load(&state.config_path)?.llm))
}

async fn put_llm(
    State(state): State<AppState>,
    Json(llm): Json<LlmConfig>,
) -> Result<StatusCode, ApiError> {
    let mut config = load(&state.config_path)?;
    config.llm = llm;
    save(&config, &state.config_path)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct LlmStatus {
    provider: LlmProviderKind,
    /// Solo para ollama/lmstudio: si el servidor local respondió.
    reachable: Option<bool>,
    /// Solo para proveedores de nube (o lmstudio con api_key_env seteado):
    /// si la variable de entorno de la key está definida (nunca su valor).
    api_key_present: Option<bool>,
    detail: String,
}

async fn status_llm(State(state): State<AppState>) -> Result<Json<LlmStatus>, ApiError> {
    let config = load(&state.config_path)?;
    let llm = &config.llm;
    let client = crate::http::client(Duration::from_secs(2));

    let status = match llm.provider {
        LlmProviderKind::Ollama => {
            let url = format!("{}/api/tags", llm.ollama.base_url.trim_end_matches('/'));
            let reachable = client.get(&url).send().await.is_ok();
            LlmStatus {
                provider: llm.provider,
                reachable: Some(reachable),
                api_key_present: None,
                detail: if reachable {
                    format!("Ollama responde en {}", llm.ollama.base_url)
                } else {
                    format!("Ollama no responde en {}", llm.ollama.base_url)
                },
            }
        }
        LlmProviderKind::LmStudio => {
            let base = llm.lmstudio.base_url.trim_end_matches('/');
            let reachable = client.get(format!("{base}/models")).send().await.is_ok();
            let api_key_present = llm
                .lmstudio
                .api_key_env
                .as_deref()
                .map(|var| std::env::var(var).is_ok());
            LlmStatus {
                provider: llm.provider,
                reachable: Some(reachable),
                api_key_present,
                detail: if reachable {
                    format!("LM Studio responde en {base}")
                } else {
                    format!("LM Studio no responde en {base}")
                },
            }
        }
        LlmProviderKind::Anthropic => {
            let present = std::env::var(&llm.anthropic.api_key_env).is_ok();
            LlmStatus {
                provider: llm.provider,
                reachable: None,
                api_key_present: Some(present),
                detail: env_detail(present, &llm.anthropic.api_key_env),
            }
        }
        LlmProviderKind::Openai => {
            let present = std::env::var(&llm.openai.api_key_env).is_ok();
            LlmStatus {
                provider: llm.provider,
                reachable: None,
                api_key_present: Some(present),
                detail: env_detail(present, &llm.openai.api_key_env),
            }
        }
        LlmProviderKind::Deepseek => {
            let present = std::env::var(&llm.deepseek.api_key_env).is_ok();
            LlmStatus {
                provider: llm.provider,
                reachable: None,
                api_key_present: Some(present),
                detail: env_detail(present, &llm.deepseek.api_key_env),
            }
        }
    };

    Ok(Json(status))
}

fn env_detail(present: bool, var: &str) -> String {
    if present {
        format!("{var} está definida")
    } else {
        format!("falta la variable de entorno {var} en tu .env")
    }
}

// ---------------------------------------------------------------------
// Sección tts
// ---------------------------------------------------------------------

async fn get_tts(State(state): State<AppState>) -> Result<Json<TtsConfig>, ApiError> {
    Ok(Json(load(&state.config_path)?.tts))
}

async fn put_tts(
    State(state): State<AppState>,
    Json(tts): Json<TtsConfig>,
) -> Result<StatusCode, ApiError> {
    let mut config = load(&state.config_path)?;
    config.tts = tts;
    save(&config, &state.config_path)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct TtsStatus {
    provider: TtsProviderKind,
    /// Solo para piper: si voice_path y config_path existen en disco.
    voice_files_present: Option<bool>,
    /// Solo para elevenlabs/cartesia: si la variable de entorno de la key
    /// está definida (nunca su valor).
    api_key_present: Option<bool>,
    detail: String,
}

async fn status_tts(State(state): State<AppState>) -> Result<Json<TtsStatus>, ApiError> {
    let config = load(&state.config_path)?;
    let tts = &config.tts;

    let status = match tts.provider {
        TtsProviderKind::Piper => {
            let present = tts.piper.voice_path.is_file() && tts.piper.config_path.is_file();
            TtsStatus {
                provider: tts.provider,
                voice_files_present: Some(present),
                api_key_present: None,
                detail: if present {
                    format!("voz encontrada en {}", tts.piper.voice_path.display())
                } else {
                    format!(
                        "faltan los archivos de voz ({} / {})",
                        tts.piper.voice_path.display(),
                        tts.piper.config_path.display()
                    )
                },
            }
        }
        TtsProviderKind::Elevenlabs => {
            let present = std::env::var(&tts.elevenlabs.api_key_env).is_ok();
            TtsStatus {
                provider: tts.provider,
                voice_files_present: None,
                api_key_present: Some(present),
                detail: env_detail(present, &tts.elevenlabs.api_key_env),
            }
        }
        TtsProviderKind::Cartesia => {
            let present = std::env::var(&tts.cartesia.api_key_env).is_ok();
            TtsStatus {
                provider: tts.provider,
                voice_files_present: None,
                api_key_present: Some(present),
                detail: env_detail(present, &tts.cartesia.api_key_env),
            }
        }
    };

    Ok(Json(status))
}

// ---------------------------------------------------------------------
// Secciones sin campos sensibles ni estado externo que consultar: mismo
// get/set mecánico que llm/tts pero sin el endpoint de status.
// ---------------------------------------------------------------------

macro_rules! section_crud {
    ($get_fn:ident, $put_fn:ident, $field:ident, $ty:ty) => {
        async fn $get_fn(State(state): State<AppState>) -> Result<Json<$ty>, ApiError> {
            Ok(Json(load(&state.config_path)?.$field))
        }

        async fn $put_fn(
            State(state): State<AppState>,
            Json(value): Json<$ty>,
        ) -> Result<StatusCode, ApiError> {
            let mut config = load(&state.config_path)?;
            config.$field = value;
            save(&config, &state.config_path)?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

section_crud!(get_workers, put_workers, workers, WorkersConfig);
section_crud!(get_stt, put_stt, stt, SttConfig);
section_crud!(get_wake, put_wake, wake, WakeConfig);
section_crud!(get_barge_in, put_barge_in, barge_in, BargeInConfig);
section_crud!(get_audio, put_audio, audio, AudioConfig);
section_crud!(get_pipeline, put_pipeline, pipeline, PipelineConfig);
section_crud!(get_welcome, put_welcome, welcome, WelcomeConfig);
section_crud!(get_mcp, put_mcp, mcp, Vec<McpServerConfig>);

#[derive(Serialize)]
struct WelcomeStatus {
    music_file_present: bool,
    detail: String,
}

async fn status_welcome(State(state): State<AppState>) -> Result<Json<WelcomeStatus>, ApiError> {
    let welcome = load(&state.config_path)?.welcome;
    let present = welcome.music_path.is_file();
    Ok(Json(WelcomeStatus {
        music_file_present: present,
        detail: if present {
            format!("encontrado en {}", welcome.music_path.display())
        } else {
            format!("no se encontró {}", welcome.music_path.display())
        },
    }))
}

// ---------------------------------------------------------------------
// Sección agent: lleva el `risk_code`, la red de seguridad final contra
// acciones peligrosas — cambiarlo (o cualquier otro campo, en el mismo
// request) exige reenviar el código ACTUAL, igual que la confirmación
// hablada de una acción de riesgo `Code`. El resto de los campos de agent
// no tienen esta restricción.
// ---------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct AgentUpdate {
    agent: AgentConfig,
    /// Requerido únicamente cuando `agent.risk_code` cambia respecto al
    /// valor guardado.
    current_risk_code: Option<String>,
}

async fn get_agent(State(state): State<AppState>) -> Result<Json<AgentConfig>, ApiError> {
    Ok(Json(load(&state.config_path)?.agent))
}

async fn put_agent(
    State(state): State<AppState>,
    Json(update): Json<AgentUpdate>,
) -> Result<StatusCode, ApiError> {
    let mut config = load(&state.config_path)?;

    if update.agent.risk_code != config.agent.risk_code {
        let provided = update.current_risk_code.as_deref().unwrap_or_default();
        if provided != config.agent.risk_code {
            return Err(ApiError::forbidden(
                "código de aceptación actual incorrecto o faltante; no se guardó ningún cambio",
            ));
        }
    }

    config.agent = update.agent;
    save(&config, &state.config_path)?;
    Ok(StatusCode::NO_CONTENT)
}
