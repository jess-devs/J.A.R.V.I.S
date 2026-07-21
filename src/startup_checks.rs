//! Validaciones de arranque: todo lo que se puede comprobar antes de pagar
//! el costo de cargar Whisper/Piper. Se acumulan todos los problemas
//! encontrados y se reportan juntos en un solo error, para que el usuario
//! no tenga que arreglar y relanzar uno por uno.

use std::time::Duration;

use rodio::cpal::traits::HostTrait;
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

    if let Err(e) = check_stt_model_files(config) {
        problems.push(e);
    }

    if let Err(e) = check_input_device_present(config) {
        problems.push(e);
    }

    if let Err(e) = check_welcome_music(config) {
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
            ensure_ollama_serve(config).await;
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

/// El STT ya no depende de Python (motor nativo, ver `src/stt/`); lo único
/// que queda del venv es el worker de TTS (Piper).
async fn check_python_imports(config: &Config) -> std::result::Result<(), String> {
    let path = config.workers.python_executable.clone();
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(&path)
            .arg("-c")
            .arg("import piper")
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

/// Los archivos del modelo de reconocimiento (Whisper + Silero VAD) no se
/// versionan (~640MB) — los descarga `scripts/setup.ps1`/`setup.sh`.
fn check_stt_model_files(config: &Config) -> std::result::Result<(), String> {
    let stt = &config.stt;
    let mut missing = Vec::new();
    for name in ["small-encoder.onnx", "small-decoder.int8.onnx", "small-tokens.txt"] {
        let path = stt.model_dir.join(name);
        if !path.exists() {
            missing.push(path.display().to_string());
        }
    }
    if !stt.vad_model_path.exists() {
        missing.push(stt.vad_model_path.display().to_string());
    }

    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "faltan archivos del modelo de reconocimiento de voz: {}. Corré scripts/setup.ps1 \
         (o scripts/setup.sh) para descargarlos",
        missing.join(", ")
    ))
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
/// sistema. No valida el `input_device_index` específico configurado (índice
/// dentro de `host.input_devices()`, ver `src/stt/capture.rs`) — si el
/// índice no existe, el motor STT lo reporta con un mensaje accionable al
/// arrancar.
fn check_input_device_present(config: &Config) -> std::result::Result<(), String> {
    let host = rodio::cpal::default_host();
    match host.input_devices() {
        Ok(mut devices) => {
            if devices.next().is_some() {
                Ok(())
            } else {
                let hint = match config.stt.input_device_index {
                    Some(idx) => format!(" (configuraste input_device_index: {idx})"),
                    None => String::new(),
                };
                Err(format!(
                    "no se detectó ningún micrófono en el sistema{hint}"
                ))
            }
        }
        Err(e) => Err(format!("no se pudo enumerar dispositivos de audio: {e}")),
    }
}

/// El mp3 del modo bienvenida es del usuario y nunca se versiona (ver
/// `assets/music/.gitkeep` y `.gitignore`) — si `welcome.enabled` está
/// prendido, tiene que haberlo puesto ahí a mano.
fn check_welcome_music(config: &Config) -> std::result::Result<(), String> {
    let welcome = &config.welcome;
    if !welcome.enabled || welcome.music_path.exists() {
        return Ok(());
    }
    Err(format!(
        "welcome.enabled=true pero no se encontró '{}'. Colocá tu mp3 ahí (ver assets/music/.gitkeep) \
         o desactivá welcome.enabled en config.yaml",
        welcome.music_path.display()
    ))
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

/// Con `llm.ollama.auto_serve: true` y un `base_url` local, levanta
/// `ollama serve` si el servidor no responde, y espera a que esté listo.
/// Nunca falla: cualquier problema se loguea y se deja que `check_ollama`
/// produzca después su error accionable de siempre. El proceso lanzado nace
/// como hijo dentro del Job Object de Jarvis (ver `ipc::job_object`), así
/// que el kernel lo mata automáticamente cuando Jarvis muere — no hace
/// falta guardar el `Child` ni limpiarlo a mano.
async fn ensure_ollama_serve(config: &Config) {
    let ollama = &config.llm.ollama;
    if !ollama.auto_serve {
        return;
    }

    let is_local = url::Url::parse(&ollama.base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|h| matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]"));
    if !is_local {
        tracing::debug!(
            base_url = %ollama.base_url,
            "auto_serve activo pero base_url no es local; no se levanta ollama serve"
        );
        return;
    }

    let url = format!("{}/api/version", ollama.base_url);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "no se pudo crear el cliente HTTP para sondear Ollama");
            return;
        }
    };

    if client.get(&url).send().await.is_ok() {
        tracing::debug!("Ollama ya está corriendo; no hace falta levantarlo");
        return;
    }

    // `println!`/`eprintln!` además del log de tracing a propósito: este
    // chequeo corre antes de que la TUI (si está activa) tome la pantalla,
    // así que es seguro imprimir acá, y es el único punto en el que
    // `ui.enabled: true` desviaría el aviso a `logs/jarvis.log` en vez de
    // mostrarlo — el usuario debe enterarse en consola de que Jarvis está
    // lanzando un proceso externo.
    println!("Ollama no responde; iniciando `ollama serve` automáticamente...");
    tracing::info!("Ollama no responde; levantando `ollama serve`...");

    // Hilos de Ollama coordinados con los de STT (ver
    // `config::compute_thread_budget`) para que no se pisen pidiendo cada
    // uno "todos los núcleos" — solo posible acá porque somos nosotros
    // quienes lanzamos el proceso; si `num_thread` viene fijado a mano en
    // config.yaml, se respeta tal cual.
    let ollama_num_thread = ollama
        .num_thread
        .unwrap_or_else(|| crate::config::compute_thread_budget(config.stt.cpu_threads).ollama);

    let mut command = tokio::process::Command::new("ollama");
    command
        .arg("serve")
        .env("OLLAMA_NUM_THREAD", ollama_num_thread.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    // El Child se dropea a propósito (sin kill_on_drop): el proceso sigue
    // vivo como hijo y el Job Object garantiza su limpieza al salir Jarvis.
    if let Err(e) = command.spawn() {
        eprintln!("No se pudo iniciar `ollama serve` automáticamente: {e}");
        tracing::warn!(
            error = %e,
            "no se pudo lanzar `ollama serve` (¿está Ollama instalado y en el PATH?)"
        );
        return;
    }

    const READY_TIMEOUT: Duration = Duration::from_secs(15);
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if client.get(&url).send().await.is_ok() {
            println!("Ollama listo.");
            tracing::info!("`ollama serve` levantado y respondiendo");
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!(
        "`ollama serve` no respondió tras {} segundos; revisá si Ollama arrancó correctamente.",
        READY_TIMEOUT.as_secs()
    );
    tracing::warn!(
        "`ollama serve` no respondió tras {} segundos; el preflight reportará el detalle",
        READY_TIMEOUT.as_secs()
    );
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
