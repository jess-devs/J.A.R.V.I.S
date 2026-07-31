"""Tests de los filtros anti-alucinación (`filters.classify_discard_reason`)
con métricas sintéticas — no necesitan audio real ni cargar Whisper.

Los umbrales usados acá replican los defaults de `stt_engine._Engine`
(ver `config.yaml` / `CONFIGURACION.md`, sección `stt.filters`).
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from filters import classify_discard_reason  # noqa: E402

DEFAULT_THRESHOLDS = {
    "max_no_speech_prob": 0.6,
    "min_avg_logprob": -1.0,
    "max_compression_ratio": 2.4,
}


def classify(no_speech_prob: float, avg_logprob: float, compression_ratio: float):
    return classify_discard_reason(
        no_speech_prob,
        avg_logprob,
        compression_ratio,
        **DEFAULT_THRESHOLDS,
    )


def test_transcripcion_confiada_no_se_descarta():
    # Métricas típicas de una frase real transcripta con confianza.
    assert classify(no_speech_prob=0.05, avg_logprob=-0.3, compression_ratio=1.4) is None


def test_alta_probabilidad_de_no_habla_se_descarta():
    # Whisper "transcribe" algo sobre silencio/ruido puro.
    assert (
        classify(no_speech_prob=0.9, avg_logprob=-0.3, compression_ratio=1.4)
        == "no_speech_prob"
    )


def test_baja_confianza_del_decoder_se_descarta():
    assert (
        classify(no_speech_prob=0.05, avg_logprob=-1.5, compression_ratio=1.4)
        == "avg_logprob"
    )


def test_texto_repetitivo_se_descarta():
    # Síntoma clásico de alucinación en bucle ("gracias gracias gracias...").
    assert (
        classify(no_speech_prob=0.05, avg_logprob=-0.3, compression_ratio=3.0)
        == "compression_ratio"
    )


def test_justo_en_el_umbral_no_se_descarta():
    # Los umbrales son estrictos (>, <), así que el valor exacto pasa.
    assert classify(no_speech_prob=0.6, avg_logprob=-1.0, compression_ratio=2.4) is None


def test_prioridad_no_speech_prob_gana_sobre_los_demas():
    # Replica el orden de chequeo de stt_engine.transcribe_loop: si varias
    # métricas fallan a la vez, se reporta la primera en ese orden.
    assert (
        classify(no_speech_prob=0.95, avg_logprob=-2.0, compression_ratio=5.0)
        == "no_speech_prob"
    )


def test_prioridad_avg_logprob_antes_que_compression_ratio():
    assert (
        classify(no_speech_prob=0.05, avg_logprob=-2.0, compression_ratio=5.0)
        == "avg_logprob"
    )
