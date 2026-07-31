"""Filtros anti-alucinación sobre la salida de Whisper: lógica pura, sin
dependencias de audio/hardware, separada de `stt_engine.py` para poder
testearla con métricas sintéticas (ver `tests/test_filters.py`).

Whisper a veces "transcribe" algo con silencio o ruido puro; cada segmento
trae sus propias métricas de confianza (`no_speech_prob`, `avg_logprob`,
`compression_ratio`) y si alguna se pasa del umbral configurado, la
transcripción se descarta antes de llegar a Jarvis.
"""

from __future__ import annotations


def classify_discard_reason(
    no_speech_prob: float,
    avg_logprob: float,
    compression_ratio: float,
    *,
    max_no_speech_prob: float,
    min_avg_logprob: float,
    max_compression_ratio: float,
) -> str | None:
    """Devuelve la razón de descarte (para el evento `discarded`), en el
    mismo orden de prioridad que revisa `stt_engine._Engine.transcribe_loop`,
    o `None` si el segmento pasa los tres filtros."""
    if no_speech_prob > max_no_speech_prob:
        return "no_speech_prob"
    if avg_logprob < min_avg_logprob:
        return "avg_logprob"
    if compression_ratio > max_compression_ratio:
        return "compression_ratio"
    return None
