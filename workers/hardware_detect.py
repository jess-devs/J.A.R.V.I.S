"""Deteccion de hardware disponible para el worker de STT.

Se importa dentro de stt_worker.py, despues del handshake inicial, para que
la carga de torch (lenta) no bloquee la respuesta al mensaje "init".
"""

import torch


def detect() -> dict:
    """Detecta si hay GPU CUDA disponible y cuanta VRAM tiene."""
    if torch.cuda.is_available():
        props = torch.cuda.get_device_properties(0)
        vram_gb = round(props.total_memory / (1024**3), 1)
        return {"device": "cuda", "vram_gb": vram_gb, "gpu_name": props.name}
    return {"device": "cpu", "vram_gb": 0.0, "gpu_name": None}


def resolve_whisper_model(vram_gb: float, override: str | None) -> str:
    """Elige el tamano de modelo Whisper segun VRAM, salvo que haya override explicito.

    "base" transcribe rapido pero se equivoca bastante seguido en espanol;
    "small" mejora la precision de forma notable a un costo de latencia
    manejable, incluso en CPU (el cuello de botella real de la conversacion
    suele ser el LLM, no el STT). Por eso es el piso, incluso sin GPU.
    """
    if override and override != "auto":
        return override
    if vram_gb >= 8:
        return "medium"
    return "small"


def resolve_compute_type(device: str, override: str | None) -> str:
    """Elige el tipo de computo (precision) segun el device, salvo override explicito."""
    if override and override != "auto":
        return override
    return "float16" if device == "cuda" else "int8"
