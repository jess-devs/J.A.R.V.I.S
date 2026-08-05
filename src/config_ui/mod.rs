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

mod calibration;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower_http::services::ServeDir;

use crate::config::{
    AgentConfig, AudioConfig, BargeInConfig, Config, LlmConfig, LlmProviderKind, McpServerConfig,
    OnboardingConfig, PipelineConfig, SttConfig, TtsConfig, TtsProviderKind, WakeConfig,
    WelcomeConfig, WorkersConfig,
};
use crate::errors::ConfigError;

#[derive(Clone)]
struct AppState {
    config_path: PathBuf,
    /// Puerto en el que quedó bindeado el servidor, para saber qué `Origin`
    /// es el de la propia página (ver `origin_allowed`).
    port: u16,
}

/// Arranca el servidor y corre para siempre (o hasta que el proceso
/// termine). Pensado para spawnearse como una tarea de fondo desde `main`;
/// un error de bind se loguea y la tarea simplemente termina, sin tirar
/// abajo el resto de Jarvis.
///
/// `on_bound`, si se pasa, se llama una sola vez apenas el `TcpListener`
/// bindea con éxito (antes de empezar a servir requests) — hoy lo usa el
/// modo standalone `jarvis --config-ui` para abrir el navegador
/// automáticamente, algo que no tendría sentido hacer desde el modo
/// integrado (Jarvis corriendo en segundo plano no debería abrir ventanas
/// por su cuenta), que por eso siempre pasa `None`.
pub async fn serve(
    config_path: PathBuf,
    addr: SocketAddr,
    static_dir: PathBuf,
    on_bound: Option<Box<dyn FnOnce() + Send>>,
) {
    let state = AppState {
        config_path,
        port: addr.port(),
    };

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
        .route(
            "/api/status/speaker_verification",
            get(status_speaker_verification),
        )
        .route("/api/config/mcp", get(get_mcp).put(put_mcp))
        .route("/api/config/agent", get(get_agent).put(put_agent))
        .route(
            "/api/config/onboarding",
            get(get_onboarding).put(put_onboarding),
        )
        .route(
            "/api/onboarding/calibration/ws",
            get(calibration::calibration_ws),
        )
        .with_state(state.clone())
        // Solo sobre `api`: los estáticos del frontend no exponen nada y no
        // tiene sentido que un `Origin` raro impida cargar la propia página.
        .layer(axum::middleware::from_fn_with_state(state, guard_origin));

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

    if let Some(on_bound) = on_bound {
        on_bound();
    }

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "el servidor de la página de configuración terminó con error");
    }
}

// ---------------------------------------------------------------------
// Guard de Origin
// ---------------------------------------------------------------------

/// Decide si un request con este `Origin` puede tocar la API local.
///
/// Bindear a 127.0.0.1 evita que la API se vea desde la red, pero no evita
/// que una página web cualquiera que el usuario esté visitando le hable al
/// servidor desde su propio navegador. Para los `PUT` REST eso lo frena el
/// preflight de CORS (son `application/json`, así que el navegador pide
/// permiso primero y acá no se contesta ninguno), pero **un WebSocket no
/// tiene same-origin policy**: sin este chequeo, cualquier sitio podía abrir
/// `/api/onboarding/calibration/ws` y desde ahí enumerar micrófonos, abrir el
/// micrófono (`start_calibration`) o pisar el embedding de voz de referencia
/// con `start_enroll`, que es justamente lo que protege el gate de
/// verificación de hablante.
///
/// Un `Origin` ausente se acepta: el navegador SIEMPRE lo manda en el
/// handshake de un WebSocket y en cualquier request cross-origin, así que su
/// ausencia significa `curl`, un test o un cliente nativo — y un programa
/// nativo corriendo como el usuario ya puede hacer todo esto sin pasar por
/// acá. La vía que este guard cierra es la del navegador.
fn origin_allowed(origin: Option<&str>, port: u16) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    // Los únicos hosts que pueden llegar a un listener bindeado a 127.0.0.1,
    // y los mismos que el frontend produce al armar la URL desde
    // `location.host`. Comparación exacta: `http://127.0.0.1.evil.com:4756`
    // y el `Origin: null` de un iframe sandboxeado no matchean.
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|host| origin == format!("http://{host}:{port}"))
}

async fn guard_origin(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Un header presente pero no representable como texto no puede ser el
    // origin de la propia página, así que se rechaza en vez de confundirse
    // con el caso "no vino ningún Origin", que sí se acepta.
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .map(|value| value.to_str().unwrap_or("<no es texto válido>"));

    if !origin_allowed(origin, state.port) {
        tracing::warn!(
            origin = origin.unwrap_or_default(),
            path = %req.uri().path(),
            "request a la API de configuración rechazado: viene de otra página web, no de la configuración local"
        );
        return ApiError::forbidden(
            "origen no permitido: esta API solo responde a la página de configuración local",
        )
        .into_response();
    }

    next.run(req).await
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
section_crud!(get_onboarding, put_onboarding, onboarding, OnboardingConfig);

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
// Estado de la verificación de hablante: si ya hay una voz enrolada.
// Vive fuera de la sección `agent` (`AgentConfig::speaker_verification`,
// PUT/GET normales) porque no es config — es una lectura del filesystem,
// mismo espíritu que `status_welcome` (chequea si el mp3 existe).
// ---------------------------------------------------------------------

#[derive(Serialize)]
struct SpeakerVerificationStatus {
    enrolled: bool,
    /// Fecha de modificación del embedding guardado, si existe (formato
    /// RFC 3339) — para mostrar "enrolado el ..." en la web.
    enrolled_at: Option<String>,
}

async fn status_speaker_verification(
    State(state): State<AppState>,
) -> Result<Json<SpeakerVerificationStatus>, ApiError> {
    let config = load(&state.config_path)?;
    let embedding_path = config.runtime_dir().join("speaker_embedding.json");

    let enrolled = embedding_path.is_file();
    let enrolled_at = std::fs::metadata(&embedding_path)
        .and_then(|meta| meta.modified())
        .ok()
        .map(|modified| chrono::DateTime::<chrono::Local>::from(modified).to_rfc3339());

    Ok(Json(SpeakerVerificationStatus {
        enrolled,
        enrolled_at,
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

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: u16 = 4756;

    #[test]
    fn sin_origin_se_acepta() {
        // curl, un cliente nativo o un request same-origin sin CORS.
        assert!(origin_allowed(None, PORT));
    }

    #[test]
    fn la_propia_pagina_se_acepta_por_cualquiera_de_sus_hosts() {
        assert!(origin_allowed(Some("http://127.0.0.1:4756"), PORT));
        assert!(origin_allowed(Some("http://localhost:4756"), PORT));
        assert!(origin_allowed(Some("http://[::1]:4756"), PORT));
    }

    #[test]
    fn otra_pagina_web_se_rechaza() {
        assert!(!origin_allowed(Some("https://example.com"), PORT));
        assert!(!origin_allowed(Some("http://evil.com"), PORT));
    }

    #[test]
    fn origin_null_se_rechaza() {
        // Un iframe sandboxeado o una página abierta con file:// mandan
        // exactamente este valor.
        assert!(!origin_allowed(Some("null"), PORT));
    }

    #[test]
    fn el_host_no_se_matchea_por_prefijo() {
        assert!(!origin_allowed(
            Some("http://127.0.0.1.evil.com:4756"),
            PORT
        ));
        assert!(!origin_allowed(
            Some("http://localhost.evil.com:4756"),
            PORT
        ));
    }

    #[test]
    fn otro_puerto_del_mismo_host_se_rechaza() {
        // Otro servidor local (o el propio Jarvis en otra config) no es la
        // página de configuración de este proceso.
        assert!(!origin_allowed(Some("http://127.0.0.1:1234"), PORT));
    }

    #[test]
    fn https_se_rechaza() {
        // El servidor es HTTP plano: un origin https con este host/puerto no
        // puede ser la propia página.
        assert!(!origin_allowed(Some("https://127.0.0.1:4756"), PORT));
    }
}
