//! Auto-selección del modelo de Ollama según el hardware de esta máquina.
//!
//! Espejo en Rust de la detección de `workers/hardware_detect.py` (que hace
//! lo mismo para elegir el modelo de Whisper): mide VRAM/RAM una sola vez al
//! arrancar y elige un tier de modelo, en vez de depender de un valor fijo en
//! config.yaml que queda obsoleto si cambia la máquina. Solo se activa con
//! `llm.ollama.model: "auto"` — un modelo fijo sigue funcionando igual que
//! siempre.

use std::time::Duration;

use crate::config::{Config, LlmProviderKind};

#[derive(Debug, Clone, Copy)]
pub struct HardwareProfile {
    pub has_gpu: bool,
    pub vram_gb: f64,
    pub ram_gb: f64,
}

/// RAM total vía `sysinfo` (ya dependencia del proyecto, mismo patrón que
/// `src/tools/system_info.rs`) y VRAM vía `nvidia-smi` (mismo comando que usa
/// `workers/hardware_detect.py::_nvidia_smi_gpu`). Sin GPU NVIDIA o sin el
/// binario `nvidia-smi` en PATH, se asume CPU-only: no es un error.
pub async fn detect_hardware() -> HardwareProfile {
    let ram_gb = {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_memory();
        sys.total_memory() as f64 / 1e9
    };

    let (has_gpu, vram_gb) = detect_nvidia_vram().await;

    HardwareProfile {
        has_gpu,
        vram_gb,
        ram_gb,
    }
}

async fn detect_nvidia_vram() -> (bool, f64) {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
            .output(),
    )
    .await;

    let Ok(Ok(out)) = output else {
        return (false, 0.0);
    };
    if !out.status.success() {
        return (false, 0.0);
    }

    let text = String::from_utf8_lossy(&out.stdout);
    match text.lines().next().unwrap_or("").trim().parse::<f64>() {
        Ok(mem_mib) => (true, (mem_mib / 1024.0 * 10.0).round() / 10.0),
        Err(_) => (false, 0.0),
    }
}

/// Tiers de GPU ordenados de mayor a menor VRAM requerida — misma tabla que
/// usa `recommend_model` abajo, pero como lista para poder "bajar un
/// escalón" en `verify_vram_fit` sin duplicar los umbrales.
/// ATENCIÓN: esta tabla vive por triplicado — acá, en `scripts/setup.ps1`
/// y en `scripts/setup.sh` — y las tres copias deben mantenerse
/// sincronizadas a mano, o el setup baja un modelo distinto del que Jarvis
/// exige al arrancar.
const GPU_MODEL_TIERS: &[&str] = &[
    "qwen3:32b",
    "qwen3:14b",
    "qwen3:8b",
    "qwen3.5:4b",
    "qwen3.5:0.8b",
];

pub fn recommend_model(hw: &HardwareProfile) -> &'static str {
    if hw.has_gpu {
        if hw.vram_gb >= 24.0 {
            GPU_MODEL_TIERS[0]
        } else if hw.vram_gb >= 16.0 {
            GPU_MODEL_TIERS[1]
        } else if hw.vram_gb >= 8.0 {
            GPU_MODEL_TIERS[2]
        } else if hw.vram_gb >= 5.0 {
            // Margen de seguridad sobre los 4GB "nominales" del modelo: el
            // compositor de Windows y otros procesos ya usan una porción de
            // la VRAM reportada por nvidia-smi antes de que Ollama cargue
            // nada, así que una GPU de *exactamente* 4.0GB casi nunca entra
            // entera — mejor no intentarlo y evitar el ciclo de "prueba,
            // falla, `verify_vram_fit` baja de escalón a mitad de sesión".
            GPU_MODEL_TIERS[3]
        } else {
            GPU_MODEL_TIERS[4]
        }
    } else if hw.ram_gb >= 16.0 {
        "qwen3.5:4b"
    } else {
        "qwen3.5:0.8b"
    }
}

/// Próximo tier más chico en `GPU_MODEL_TIERS`, si existe. `None` si el
/// modelo actual no está en la tabla o ya es el más chico.
fn step_down_gpu_tier(current: &str) -> Option<&'static str> {
    let idx = GPU_MODEL_TIERS.iter().position(|&m| m == current)?;
    GPU_MODEL_TIERS.get(idx + 1).copied()
}

/// Cuánto tiempo mantiene Ollama el modelo cargado en VRAM/RAM tras la
/// última respuesta, según el mismo `HardwareProfile` que ya usa
/// `recommend_model`. Solo se aplica cuando `llm.ollama.keep_alive` quedó
/// en null en config.yaml — un valor fijo por el usuario siempre gana.
/// Máquinas justas de VRAM/RAM liberan más agresivo; máquinas con margen se
/// quedan calentitas más tiempo por si sigue la conversación.
pub fn recommend_keep_alive(hw: &HardwareProfile) -> &'static str {
    if hw.has_gpu {
        if hw.vram_gb >= 16.0 {
            "5m"
        } else if hw.vram_gb >= 4.0 {
            "1m"
        } else {
            "30s"
        }
    } else if hw.ram_gb >= 16.0 {
        "1m"
    } else {
        "30s"
    }
}

/// `true` para los modelos con tokens de "pensamiento" (qwen3, deepseek-r1):
/// ver el comentario de `OllamaConfig::think` en `config.rs`. Ollama rechaza
/// el campo `think` en modelos que no lo soportan, así que solo se activa
/// para estas familias.
pub fn model_needs_think_false(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.starts_with("qwen3") || lower.contains("deepseek-r1")
}

/// No hace nada salvo que el proveedor activo sea Ollama. Detecta el
/// hardware una sola vez y, según qué haya quedado sin fijar en
/// config.yaml, auto-selecciona el modelo (`model: "auto"`) y/o el
/// `keep_alive` (`keep_alive: null`) antes de que `startup_checks`/
/// `build_provider` lean la config — cada uno se resuelve por separado, así
/// que un modelo fijado a mano igual recibe un `keep_alive` ajustado a la
/// máquina, y viceversa.
///
/// Devuelve `true` si el modelo fue auto-seleccionado (es decir, si tenía
/// sentido en `config.yaml` dejarlo en `"auto"`) — el llamador lo usa para
/// decidir si vale la pena correr `verify_vram_fit` más adelante; un modelo
/// fijado a mano por el usuario nunca se toca.
pub async fn resolve(config: &mut Config) -> bool {
    if config.llm.provider != LlmProviderKind::Ollama {
        return false;
    }

    let model_is_auto = config.llm.ollama.model == "auto";
    let keep_alive_is_auto = config.llm.ollama.keep_alive.is_none();
    if !model_is_auto && !keep_alive_is_auto {
        return false;
    }

    let hw = detect_hardware().await;

    if model_is_auto {
        let model = recommend_model(&hw);
        tracing::info!(
            has_gpu = hw.has_gpu,
            vram_gb = hw.vram_gb,
            ram_gb = hw.ram_gb,
            model,
            "modelo de Ollama auto-seleccionado según hardware"
        );

        config.llm.ollama.model = model.to_string();
        if config.llm.ollama.think.is_none() && model_needs_think_false(model) {
            config.llm.ollama.think = Some(false);
        }
    }

    if keep_alive_is_auto {
        let keep_alive = recommend_keep_alive(&hw);
        tracing::info!(
            has_gpu = hw.has_gpu,
            vram_gb = hw.vram_gb,
            ram_gb = hw.ram_gb,
            keep_alive,
            "keep_alive de Ollama auto-calculado según hardware"
        );
        config.llm.ollama.keep_alive = Some(keep_alive.to_string());
    }

    model_is_auto
}

#[derive(serde::Deserialize)]
struct OllamaPsResponse {
    #[serde(default)]
    models: Vec<OllamaPsModel>,
}

#[derive(serde::Deserialize)]
struct OllamaPsModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    size_vram: u64,
}

async fn fetch_loaded_model_sizes(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
) -> Option<(u64, u64)> {
    let resp = client
        .get(format!("{base_url}/api/ps"))
        .send()
        .await
        .ok()?;
    let parsed: OllamaPsResponse = resp.json().await.ok()?;
    parsed
        .models
        .into_iter()
        .find(|m| m.name == model || m.name == format!("{model}:latest"))
        .map(|m| (m.size, m.size_vram))
}

/// Dispara una generación mínima (fuerza la carga a VRAM si el modelo no
/// estaba ya cargado) y consulta `/api/ps` para ver si entró entero.
/// `true` = offload parcial detectado (conviene bajar de escalón); `false`
/// = cupo entero, o no se pudo medir (se asume que está bien y no se toca
/// nada — mejor un falso negativo que downgradear sin evidencia).
async fn warm_up_and_check_partial_offload(
    client: &reqwest::Client,
    base_url: &str,
    config: &Config,
) -> bool {
    let warm_up = serde_json::json!({
        "model": config.llm.ollama.model,
        "messages": [{"role": "user", "content": "hola"}],
        "stream": false,
        "keep_alive": config.llm.ollama.keep_alive,
    });
    if client
        .post(format!("{base_url}/api/chat"))
        .json(&warm_up)
        .send()
        .await
        .is_err()
    {
        return false;
    }

    let Some((size, size_vram)) =
        fetch_loaded_model_sizes(client, base_url, &config.llm.ollama.model).await
    else {
        return false;
    };

    // 0.85 en vez de 0.95: solo baja de escalón ante offload sustancial, no
    // ante un desajuste chico que apenas roza el borde de la VRAM.
    size > 0 && (size_vram as f64) < (size as f64 * 0.85)
}

/// Confirma que el modelo auto-seleccionado por `resolve` cargó entero en
/// VRAM. Si Ollama hizo offload parcial a CPU (el modelo elegido justo en
/// el límite de VRAM disponible), cada token generado usa GPU y CPU a la
/// vez — la causa más probable de que STT + Ollama saturen ambos al mismo
/// tiempo. Baja un escalón de la tabla y reintenta UNA sola vez; si el
/// escalón más chico tampoco entra, se deja así y solo se loguea.
///
/// Debe llamarse DESPUÉS de `startup_checks::run` (necesita `ollama serve`
/// arriba y el modelo ya confirmado/pulled por `check_ollama`), y solo si
/// `resolve` devolvió `true` (modelo auto-seleccionado — uno fijado a mano
/// por el usuario no se toca).
pub async fn verify_vram_fit(config: &mut Config) {
    if config.llm.provider != LlmProviderKind::Ollama {
        return;
    }

    let hw = detect_hardware().await;
    if !hw.has_gpu {
        // Sin GPU no hay VRAM que verificar; el tier CPU-only ya es el
        // único que existe para esta máquina.
        return;
    }

    let client = reqwest::Client::new();
    let base_url = config.llm.ollama.base_url.clone();

    if !warm_up_and_check_partial_offload(&client, &base_url, config).await {
        return;
    }

    let Some(next) = step_down_gpu_tier(&config.llm.ollama.model) else {
        tracing::warn!(
            model = %config.llm.ollama.model,
            "offload parcial a CPU detectado, pero ya es el modelo más chico de la tabla; se deja así"
        );
        return;
    };

    tracing::warn!(
        from = %config.llm.ollama.model,
        to = next,
        "offload parcial a CPU detectado (el modelo no entró entero en VRAM); bajando un escalón"
    );
    config.llm.ollama.model = next.to_string();
    if config.llm.ollama.think.is_none() && model_needs_think_false(next) {
        config.llm.ollama.think = Some(false);
    }

    if warm_up_and_check_partial_offload(&client, &base_url, config).await {
        tracing::warn!(
            model = %config.llm.ollama.model,
            "sigue habiendo offload parcial incluso tras bajar de escalón; se deja así"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(has_gpu: bool, vram_gb: f64, ram_gb: f64) -> HardwareProfile {
        HardwareProfile {
            has_gpu,
            vram_gb,
            ram_gb,
        }
    }

    #[test]
    fn gpu_tiers_by_vram() {
        assert_eq!(recommend_model(&hw(true, 24.0, 0.0)), "qwen3:32b");
        assert_eq!(recommend_model(&hw(true, 16.0, 0.0)), "qwen3:14b");
        assert_eq!(recommend_model(&hw(true, 8.0, 0.0)), "qwen3:8b");
        assert_eq!(recommend_model(&hw(true, 5.0, 0.0)), "qwen3.5:4b");
        assert_eq!(recommend_model(&hw(true, 2.0, 0.0)), "qwen3.5:0.8b");
    }

    #[test]
    fn gpu_4gb_boundary_falls_to_smallest_tier_with_safety_margin() {
        // Una GPU de 4.0GB "nominales" ya no recibe qwen3.5:4b: el margen de
        // seguridad asume que parte de esa VRAM no está realmente libre.
        assert_eq!(recommend_model(&hw(true, 4.9, 0.0)), "qwen3.5:0.8b");
        assert_eq!(recommend_model(&hw(true, 4.0, 0.0)), "qwen3.5:0.8b");
        assert_eq!(recommend_model(&hw(true, 5.0, 0.0)), "qwen3.5:4b");
    }

    #[test]
    fn cpu_only_tiers_by_ram() {
        assert_eq!(recommend_model(&hw(false, 0.0, 16.0)), "qwen3.5:4b");
        assert_eq!(recommend_model(&hw(false, 0.0, 32.0)), "qwen3.5:4b");
        assert_eq!(recommend_model(&hw(false, 0.0, 8.0)), "qwen3.5:0.8b");
    }

    #[test]
    fn keep_alive_tiers_by_vram() {
        assert_eq!(recommend_keep_alive(&hw(true, 24.0, 0.0)), "5m");
        assert_eq!(recommend_keep_alive(&hw(true, 16.0, 0.0)), "5m");
        assert_eq!(recommend_keep_alive(&hw(true, 8.0, 0.0)), "1m");
        assert_eq!(recommend_keep_alive(&hw(true, 4.0, 0.0)), "1m");
        assert_eq!(recommend_keep_alive(&hw(true, 2.0, 0.0)), "30s");
    }

    #[test]
    fn keep_alive_tiers_cpu_only_by_ram() {
        assert_eq!(recommend_keep_alive(&hw(false, 0.0, 32.0)), "1m");
        assert_eq!(recommend_keep_alive(&hw(false, 0.0, 16.0)), "1m");
        assert_eq!(recommend_keep_alive(&hw(false, 0.0, 8.0)), "30s");
    }

    #[test]
    fn steps_down_through_gpu_tiers() {
        assert_eq!(step_down_gpu_tier("qwen3:32b"), Some("qwen3:14b"));
        assert_eq!(step_down_gpu_tier("qwen3:14b"), Some("qwen3:8b"));
        assert_eq!(step_down_gpu_tier("qwen3:8b"), Some("qwen3.5:4b"));
        assert_eq!(step_down_gpu_tier("qwen3.5:4b"), Some("qwen3.5:0.8b"));
        assert_eq!(step_down_gpu_tier("qwen3.5:0.8b"), None);
        assert_eq!(step_down_gpu_tier("modelo-no-en-la-tabla"), None);
    }

    #[test]
    fn think_false_for_reasoning_models() {
        assert!(model_needs_think_false("qwen3:8b"));
        assert!(model_needs_think_false("qwen3:32b"));
        assert!(model_needs_think_false("qwen3.5:4b"));
        assert!(model_needs_think_false("qwen3.5:0.8b"));
        assert!(model_needs_think_false("deepseek-r1:14b"));
        assert!(!model_needs_think_false("qwen2.5:7b"));
        assert!(!model_needs_think_false("qwen2.5:3b-instruct"));
    }
}
