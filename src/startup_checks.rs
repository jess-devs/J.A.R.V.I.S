//! Validaciones de arranque: todo lo que se puede comprobar antes de pagar
//! el costo de cargar Whisper/Piper. Se acumulan todos los problemas
//! encontrados y se reportan juntos en un solo error, para que el usuario
//! no tenga que arreglar y relanzar uno por uno.

use std::time::Duration;

use cpal::traits::HostTrait;
use serde::Deserialize;

use crate::config::{Config, LlmProviderKind, TtsProviderKind};
use crate::errors::{JarvisError, Result};

pub async fn run(config: &Config) -> Result<()> {
    let mut problems = Vec::new();

    if let Err(e) = check_python_executable(config) {
        problems.push(e);
    } else if let Err(e) = check_python_imports(config).await {
        problems.push(e);
    }

    if let Err(e) = check_input_device_present(config) {
        problems.push(e);
    }

    match config.tts.provider {
        TtsProviderKind::Piper => {
            if let Err(e) = check_piper_voice_files(config) {
                problems.push(e);
            }
        }
        TtsProviderKind::Elevenlabs => {
            if let Err(e) = check_cloud_api_key(&config.tts.elevenlabs.api_key_env) {
                problems.push(e);
            }
        }
        TtsProviderKind::Cartesia => {
            if let Err(e) = check_cloud_api_key(&config.tts.cartesia.api_key_env) {
                problems.push(e);
            }
        }
    }

    match config.llm.provider {
        LlmProviderKind::Ollama => {
            if let Err(e) = check_ollama(config).await {
                problems.push(e);
            } else if config.agent.enabled {
                warn_model_tool_support("Ollama", &config.llm.ollama.model);
            }
        }
        LlmProviderKind::Anthropic => {
            if let Err(e) = check_cloud_api_key(&config.llm.anthropic.api_key_env) {
                problems.push(e);
            }
        }
        LlmProviderKind::Openai => {
            if let Err(e) = check_cloud_api_key(&config.llm.openai.api_key_env) {
                problems.push(e);
            }
        }
        LlmProviderKind::Deepseek => {
            if let Err(e) = check_cloud_api_key(&config.llm.deepseek.api_key_env) {
                problems.push(e);
            }
        }
        LlmProviderKind::LmStudio => {
            if let Err(e) = check_lmstudio(config).await {
                problems.push(e);
            } else if config.agent.enabled {
                warn_model_tool_support("LM Studio", &config.llm.lmstudio.model);
            }
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        let joined = problems
            .iter()
            .map(|p| format!("  - {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        Err(JarvisError::Preflight(format!(
            "se encontraron {} problema(s) antes de arrancar:\n{joined}",
            problems.len()
        )))
    }
}

fn check_python_executable(config: &Config) -> std::result::Result<(), String> {
    let path = &config.workers.python_executable;
    if path.exists() {
        Ok(())
    } else {
        Err(format!(
            "no se encontró el ejecutable de Python en '{}'. Creá el venv siguiendo workers/README.md",
            path.display()
        ))
    }
}

async fn check_python_imports(config: &Config) -> std::result::Result<(), String> {
    let path = config.workers.python_executable.clone();
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(&path)
            .arg("-c")
            .arg("import RealtimeSTT, piper, torch, faster_whisper, silero_vad")
            .output(),
    )
    .await;

    match output {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => Err(format!(
            "el entorno Python en '{}' no tiene las dependencias instaladas. Ejecutá: {} -m pip install -r workers/requirements.txt (detalle: {})",
            path.display(),
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(Err(e)) => Err(format!("no se pudo ejecutar '{}': {e}", path.display())),
        Err(_) => Err(format!(
            "tiempo de espera agotado comprobando el entorno Python en '{}'",
            path.display()
        )),
    }
}

fn check_piper_voice_files(config: &Config) -> std::result::Result<(), String> {
    let piper = &config.tts.piper;
    let mut missing = Vec::new();
    if !piper.voice_path.exists() {
        missing.push(piper.voice_path.display().to_string());
    }
    if !piper.config_path.exists() {
        missing.push(piper.config_path.display().to_string());
    }

    if missing.is_empty() {
        return Ok(());
    }

    let voice_name = piper
        .voice_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Err(format!(
        "faltan archivos de voz Piper: {}. Descargala con: python -m piper.download_voices {voice_name} (destino: voices/)",
        missing.join(", ")
    ))
}

/// Comprobación genérica: hace falta que exista al menos un micrófono en el
/// sistema. No valida el `input_device_index` específico configurado — los
/// índices de cpal (usado acá) y de PyAudio (usado por el worker de STT) no
/// tienen por qué coincidir, así que una validación cruzada podría dar tanto
/// falsos positivos como falsos negativos. La validación real de ese índice
/// ocurre en el worker Python al abrir el stream (reporta `fatal_error` con
/// un mensaje accionable si el índice no existe para PyAudio); acá solo se
/// deja una pista hacia `--list-devices` cuando hay un índice configurado.
fn check_input_device_present(config: &Config) -> std::result::Result<(), String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(mut devices) => {
            if devices.next().is_some() {
                Ok(())
            } else {
                let hint = match config.stt.input_device_index {
                    Some(idx) => format!(
                        " (configuraste input_device_index: {idx} — corré \
                         `python workers/stt_worker.py --list-devices` para ver \
                         los índices reales de PyAudio)"
                    ),
                    None => String::new(),
                };
                Err(format!("no se detectó ningún micrófono en el sistema{hint}"))
            }
        }
        Err(e) => Err(format!("no se pudo enumerar dispositivos de audio: {e}")),
    }
}

fn check_cloud_api_key(env_var: &str) -> std::result::Result<(), String> {
    if std::env::var(env_var).is_ok() {
        Ok(())
    } else {
        Err(format!(
            "falta la variable de entorno {env_var}. Definila en tu .env"
        ))
    }
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagsModel>,
}

#[derive(Deserialize)]
struct TagsModel {
    name: String,
}

/// El modo agéntico depende de tool calling. Casi todos los modelos
/// instruidos recientes lo soportan, pero algunos populares (las familias
/// `llama2`, `gemma`, `phi`, `mistral` clásico) no. No se puede saber con
/// certeza sin una petición de prueba, así que solo avisamos: si el modelo
/// pertenece a una familia conocida sin tools, sugerimos un cambio.
/// `contains` (no `starts_with`) porque catálogos como el de LM Studio
/// anteponen el namespace del autor, ej. "google/gemma-4-e4b".
fn warn_model_tool_support(provider: &str, model: &str) {
    const SIN_TOOLS: [&str; 4] = ["llama2", "gemma", "phi", "orca"];
    let lower = model.to_lowercase();
    if SIN_TOOLS.iter().any(|fam| lower.contains(fam)) {
        tracing::warn!(
            provider,
            model,
            "el modo agéntico está activo pero '{model}' ({provider}) podría no soportar \
             tool calling. Si Jarvis no usa las herramientas, probá con un modelo de la \
             familia qwen (ej. 'qwen3:8b' en Ollama).",
        );
    } else {
        tracing::info!(provider, model, "modo agéntico activo con {provider}");
    }
}

async fn check_ollama(config: &Config) -> std::result::Result<(), String> {
    let base_url = &config.llm.ollama.base_url;
    let model = &config.llm.ollama.model;
    let url = format!("{base_url}/api/tags");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(&url).send().await.map_err(|_| {
        format!("no se pudo conectar a Ollama en {base_url}. ¿Está corriendo `ollama serve`?")
    })?;

    let body: TagsResponse = response
        .json()
        .await
        .map_err(|e| format!("respuesta inesperada de Ollama: {e}"))?;

    let found = body.models.iter().any(|m| &m.name == model);
    let available: Vec<&str> = body.models.iter().map(|m| m.name.as_str()).collect();
    if found {
        Ok(())
    } else {
        let listado = if available.is_empty() {
            "ninguno".to_string()
        } else {
            available.join(", ")
        };
        Err(format!(
            "el modelo '{model}' no está descargado en Ollama. Ejecutá: ollama pull {model} (modelos disponibles: {listado})"
        ))
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelsEntry>,
}

#[derive(Deserialize)]
struct ModelsEntry {
    id: String,
}

async fn check_lmstudio(config: &Config) -> std::result::Result<(), String> {
    let lmstudio = &config.llm.lmstudio;
    if let Some(env_var) = &lmstudio.api_key_env {
        check_cloud_api_key(env_var)?;
    }

    let base_url = lmstudio.base_url.trim_end_matches('/');
    let model = &lmstudio.model;
    let url = format!("{base_url}/models");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(&url).send().await.map_err(|_| {
        format!(
            "no se pudo conectar a LM Studio en {base_url}. ¿Tenés el servidor local \
             activado? (pestaña Developer -> Start Server)"
        )
    })?;

    let body: ModelsResponse = response
        .json()
        .await
        .map_err(|e| format!("respuesta inesperada de LM Studio: {e}"))?;

    let found = body.data.iter().any(|m| &m.id == model);
    let available: Vec<&str> = body.data.iter().map(|m| m.id.as_str()).collect();
    if found {
        Ok(())
    } else {
        let listado = if available.is_empty() {
            "ninguno".to_string()
        } else {
            available.join(", ")
        };
        Err(format!(
            "el modelo '{model}' no está cargado en LM Studio (modelos disponibles: \
             {listado}). Cargalo desde la pestaña Developer o ajustá llm.lmstudio.model."
        ))
    }
}
