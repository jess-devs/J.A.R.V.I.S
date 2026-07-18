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

/// Tabla de tiers curada a mano: todos modelos de la familia qwen, ya
/// excluida de la lista negra de `warn_model_tool_support` en
/// `startup_checks.rs`, así que el modo agéntico queda garantizado.
/// ATENCIÓN: esta tabla vive por triplicado — acá, en `scripts/setup.ps1`
/// y en `scripts/setup.sh` — y las tres copias deben mantenerse
/// sincronizadas a mano, o el setup baja un modelo distinto del que Jarvis
/// exige al arrancar.
pub fn recommend_model(hw: &HardwareProfile) -> &'static str {
    if hw.has_gpu {
        if hw.vram_gb >= 24.0 {
            "qwen3:32b"
        } else if hw.vram_gb >= 16.0 {
            "qwen3:14b"
        } else if hw.vram_gb >= 8.0 {
            "qwen3:8b"
        } else if hw.vram_gb >= 4.0 {
            "qwen3.5:4b"
        } else {
            "qwen3.5:0.8b"
        }
    } else if hw.ram_gb >= 16.0 {
        "qwen3.5:4b"
    } else {
        "qwen3.5:0.8b"
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

/// No hace nada salvo que el proveedor activo sea Ollama y
/// `llm.ollama.model` esté en `"auto"`. En ese caso detecta el hardware,
/// elige el modelo y sobreescribe la config en memoria antes de que
/// `startup_checks`/`build_provider` la lean.
pub async fn resolve(config: &mut Config) {
    if config.llm.provider != LlmProviderKind::Ollama || config.llm.ollama.model != "auto" {
        return;
    }

    let hw = detect_hardware().await;
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
        assert_eq!(recommend_model(&hw(true, 4.0, 0.0)), "qwen3.5:4b");
        assert_eq!(recommend_model(&hw(true, 2.0, 0.0)), "qwen3.5:0.8b");
    }

    #[test]
    fn cpu_only_tiers_by_ram() {
        assert_eq!(recommend_model(&hw(false, 0.0, 16.0)), "qwen3.5:4b");
        assert_eq!(recommend_model(&hw(false, 0.0, 32.0)), "qwen3.5:4b");
        assert_eq!(recommend_model(&hw(false, 0.0, 8.0)), "qwen3.5:0.8b");
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
