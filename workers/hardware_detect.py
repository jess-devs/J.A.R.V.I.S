"""Deteccion y calibracion de hardware para el worker de STT.

En vez de adivinar por specs, la primera vez que se arranca con todo en
"auto" se mide la velocidad real de transcripcion de ESTA maquina (RTF =
tiempo de transcripcion / duracion del audio) usando el warmup_audio.wav
que trae RealtimeSTT. El resultado se cachea en .cache/stt_profile.json
con un fingerprint del hardware: los arranques siguientes son instantaneos
y solo se recalibra si el hardware cambio (o si se fuerza por config).

Cualquier override manual (device/model distintos de "auto") salta la
calibracion por completo.

Se importa dentro de stt_worker.py despues del handshake inicial, para que
la carga de torch (lenta) no bloquee la respuesta al mensaje "init".
"""

import json
import os
import platform
import subprocess
import sys
import time
import wave

CACHE_PATH = os.path.join(os.path.dirname(__file__), ".cache", "stt_profile.json")

# Umbrales de la escalera de calibracion en CPU (RTF = tiempo/duracion).
# Con margen de sobra se habilita ademas la transcripcion temprana durante
# la ventana de confirmacion de silencio (~0.3s menos de latencia por turno).
RTF_COMFORTABLE = 0.5
RTF_ACCEPTABLE = 1.0
EARLY_TRANSCRIPTION_SECS = 0.3


def _log(message: str) -> None:
    print(f"[hardware_detect] {message}", file=sys.stderr, flush=True)


def _nvidia_smi_gpu() -> tuple[str | None, float]:
    """Nombre y VRAM (GB) de la GPU 0 vía nvidia-smi. No depende de que torch
    este compilado con soporte CUDA (el venv trae la build CPU-only, mas
    liviana; el motor real de inferencia es ctranslate2, no torch)."""
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=5, check=True,
        )
        name, mem_mib = (part.strip() for part in out.stdout.strip().splitlines()[0].split(","))
        return name, round(float(mem_mib) / 1024, 1)
    except Exception:
        return None, 0.0


def detect() -> dict:
    """Detecta GPU CUDA (con VRAM) y datos basicos de la CPU.

    La disponibilidad de CUDA se comprueba con ctranslate2 (el motor que
    ejecuta faster-whisper), no con torch: torch.cuda.is_available() daria
    falso con la build CPU-only del venv aunque la GPU sea perfectamente
    utilizable para la transcripcion.
    """
    info = {
        "cpu_name": platform.processor() or platform.machine(),
        "logical_cores": os.cpu_count() or 4,
        "device": "cpu",
        "vram_gb": 0.0,
        "gpu_name": None,
    }
    try:
        import ctranslate2

        cuda_usable = ctranslate2.get_cuda_device_count() > 0
    except Exception:
        cuda_usable = False
    if cuda_usable:
        name, vram_gb = _nvidia_smi_gpu()
        info["device"] = "cuda"
        info["gpu_name"] = name
        info["vram_gb"] = vram_gb
    return info


def resolve_cpu_threads(logical_cores: int, override: int | None) -> int:
    """Hilos para ctranslate2. Aproxima nucleos fisicos: los hyperthreads no
    ayudan a la inferencia y el default de la libreria (4 hilos) desperdicia
    la mitad del silicio en CPUs grandes."""
    if override:
        return override
    return max(2, logical_cores // 2)


def resolve_compute_type(device: str, override: str | None) -> str:
    if override and override != "auto":
        return override
    # "float16" puro falla en GPUs sin tensor cores (ej. GTX 16xx / Turing
    # TU11x: "Requested float16 compute type, but the target device or
    # backend do not support efficient float16 computation"). int8_float16
    # cuantiza los pesos a int8 y hace el computo en float16: funciona en
    # cualquier GPU CUDA y es mas preciso que int8 puro.
    return "int8_float16" if device == "cuda" else "int8"


def _warmup_wav_path() -> str | None:
    try:
        import RealtimeSTT

        path = os.path.join(
            os.path.dirname(RealtimeSTT.__file__), "assets", "warmup_audio.wav"
        )
        return path if os.path.exists(path) else None
    except Exception:
        return None


def measure_rtf(model_size: str, compute_type: str, cpu_threads: int) -> float | None:
    """Mide el RTF real de un modelo Whisper en esta CPU. Dos pasadas: la
    primera absorbe el warmup de kernels, se cronometra la segunda."""
    wav_path = _warmup_wav_path()
    if wav_path is None:
        _log("no se encontro warmup_audio.wav, se salta el benchmark")
        return None

    with wave.open(wav_path, "rb") as wav:
        duration = wav.getnframes() / wav.getframerate()
    if duration <= 0:
        return None

    from faster_whisper import WhisperModel

    _log(f"midiendo velocidad del modelo '{model_size}' (esto pasa una sola vez)...")
    model = WhisperModel(
        model_size, device="cpu", compute_type=compute_type, cpu_threads=cpu_threads
    )
    try:
        for attempt in range(2):
            start = time.perf_counter()
            segments, _ = model.transcribe(wav_path, language="es", beam_size=5)
            for _segment in segments:  # el generador es lazy: hay que agotarlo
                pass
            elapsed = time.perf_counter() - start
        rtf = elapsed / duration
        _log(f"modelo '{model_size}': {elapsed:.2f}s para {duration:.2f}s de audio (RTF {rtf:.2f})")
        return rtf
    finally:
        del model


def _calibrate_cpu(compute_type: str, cpu_threads: int) -> dict:
    """Escalera de decision: empieza en small y solo baja si la maquina no
    llega. Nunca sube a modelos gigantes sin pedirlo explicitamente."""
    rtf = measure_rtf("small", compute_type, cpu_threads)
    if rtf is None:
        # Sin benchmark posible: default conservador equivalente al anterior.
        return {"whisper_model": "small", "beam_size": 3, "early_transcription": 0.0, "rtf": None}
    if rtf <= RTF_COMFORTABLE:
        return {
            "whisper_model": "small",
            "beam_size": 5,
            "early_transcription": EARLY_TRANSCRIPTION_SECS,
            "rtf": round(rtf, 3),
        }
    if rtf <= RTF_ACCEPTABLE:
        return {"whisper_model": "small", "beam_size": 3, "early_transcription": 0.0, "rtf": round(rtf, 3)}

    rtf_base = measure_rtf("base", compute_type, cpu_threads)
    if rtf_base is not None and rtf_base <= RTF_ACCEPTABLE:
        return {"whisper_model": "base", "beam_size": 3, "early_transcription": 0.0, "rtf": round(rtf_base, 3)}
    return {
        "whisper_model": "tiny",
        "beam_size": 3,
        "early_transcription": 0.0,
        "rtf": round(rtf_base, 3) if rtf_base is not None else None,
    }


def _gpu_model_for_vram(vram_gb: float) -> str:
    if vram_gb >= 8:
        return "large-v3-turbo"  # precision cercana a large-v3, velocidad cercana a medium
    if vram_gb >= 6:
        return "medium"
    if vram_gb >= 4:
        return "small"
    return "base"


def _fingerprint(hw: dict) -> dict:
    try:
        from importlib.metadata import version

        stt_version = version("RealtimeSTT")
    except Exception:
        stt_version = "unknown"
    return {
        "cpu_name": hw["cpu_name"],
        "logical_cores": hw["logical_cores"],
        "gpu_name": hw["gpu_name"],
        "vram_gb": hw["vram_gb"],
        "realtimestt": stt_version,
        # Subir cuando cambie la logica de calibracion, para invalidar caches viejos.
        "calib_version": 2,
    }


def _load_cache(fingerprint: dict) -> dict | None:
    try:
        with open(CACHE_PATH, encoding="utf-8") as fh:
            cached = json.load(fh)
        if cached.get("fingerprint") == fingerprint:
            return cached["profile"]
    except (OSError, ValueError, KeyError):
        pass
    return None


def _save_cache(fingerprint: dict, profile: dict) -> None:
    try:
        os.makedirs(os.path.dirname(CACHE_PATH), exist_ok=True)
        with open(CACHE_PATH, "w", encoding="utf-8") as fh:
            json.dump({"fingerprint": fingerprint, "profile": profile}, fh, indent=2)
    except OSError as exc:
        _log(f"no se pudo guardar el cache de calibracion: {exc}")


def resolve_profile(init_msg: dict, cpu_threads: int) -> dict:
    """Punto unico de decision del perfil de STT. Devuelve un dict con:
    device, compute_type, whisper_model, beam_size, early_transcription,
    cpu_threads, vram_gb, rtf, from_cache."""
    hw = detect()

    device_override = init_msg.get("device", "auto")
    model_override = init_msg.get("model", "auto")
    beam_override = init_msg.get("beam_size")
    recalibrate = bool(init_msg.get("recalibrate", False))

    device = hw["device"] if device_override == "auto" else device_override
    compute_type = resolve_compute_type(device, init_msg.get("compute_type"))

    base = {
        "device": device,
        "compute_type": compute_type,
        "cpu_threads": cpu_threads,
        "vram_gb": hw["vram_gb"],
        "from_cache": False,
    }

    # Override manual del modelo: el usuario manda, sin benchmark.
    if model_override and model_override != "auto":
        return {
            **base,
            "whisper_model": model_override,
            "beam_size": beam_override or 5,
            "early_transcription": 0.0,
            "rtf": None,
        }

    # GPU: sobra velocidad, tiers por VRAM sin benchmark.
    if device == "cuda":
        return {
            **base,
            "whisper_model": _gpu_model_for_vram(hw["vram_gb"]),
            "beam_size": beam_override or 5,
            "early_transcription": EARLY_TRANSCRIPTION_SECS,
            "rtf": None,
        }

    # CPU: perfil calibrado, cacheado por fingerprint de hardware.
    fingerprint = _fingerprint(hw)
    if not recalibrate:
        cached = _load_cache(fingerprint)
        if cached is not None:
            _log("perfil de calibracion cargado desde cache")
            profile = {**base, **cached, "from_cache": True}
            if beam_override:
                profile["beam_size"] = beam_override
            return profile

    calibrated = _calibrate_cpu(compute_type, cpu_threads)
    _save_cache(fingerprint, calibrated)
    profile = {**base, **calibrated}
    if beam_override:
        profile["beam_size"] = beam_override
    return profile
