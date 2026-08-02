"""Tests de las funciones puras de `calibration_engine.py` (RMS->dBFS y el
mapeo camelCase->snake_case de `AudioDeviceInfo`) — no abren ningún stream
de audio real ni instancian `pyaudio.PyAudio()`.
"""

import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from calibration_engine import (  # noqa: E402
    _fix_mojibake,
    _looks_like_real_microphone,
    device_info_to_dict,
    friendly_device_error,
    rms_dbfs,
)


def test_rms_dbfs_silencio_es_muy_negativo():
    silencio = np.zeros(512, dtype=np.float32)
    assert rms_dbfs(silencio) < -80.0


def test_rms_dbfs_tono_a_amplitud_completa_es_cercano_a_cero():
    # Onda cuadrada a amplitud +-1.0: RMS = 1.0 -> 0 dBFS exacto.
    onda_cuadrada = np.array([1.0, -1.0] * 256, dtype=np.float32)
    assert abs(rms_dbfs(onda_cuadrada)) < 0.01


def test_rms_dbfs_a_media_amplitud_es_unos_6db_menos():
    silencio_medio = np.array([0.5, -0.5] * 256, dtype=np.float32)
    # 20*log10(0.5) ~= -6.02 dB respecto de amplitud completa.
    assert -6.5 < rms_dbfs(silencio_medio) < -5.5


def test_device_info_to_dict_mapea_camel_case_a_snake_case():
    info = {
        "name": "Micrófono USB",
        "maxInputChannels": 2,
        "defaultSampleRate": 44100.0,
    }
    result = device_info_to_dict(3, info, default_index=1)
    assert result == {
        "index": 3,
        "name": "Micrófono USB",
        "max_input_channels": 2,
        "default_sample_rate": 44100,
        "is_default": False,
    }


def test_device_info_to_dict_marca_is_default_cuando_coincide_el_indice():
    info = {"name": "Mic default", "maxInputChannels": 1, "defaultSampleRate": 16000.0}
    result = device_info_to_dict(1, info, default_index=1)
    assert result["is_default"] is True


def test_device_info_to_dict_usa_nombre_generico_si_falta():
    info = {"maxInputChannels": 1, "defaultSampleRate": 16000.0}
    result = device_info_to_dict(5, info, default_index=None)
    assert result["name"] == "Dispositivo 5"
    assert result["is_default"] is False


def test_fix_mojibake_repara_utf8_mal_reinterpretado_como_latin1():
    # PortAudio en Windows a veces reporta "Mezcla estéreo" (nombres del
    # host API MME) con sus bytes UTF-8 de "é" (0xC3 0xA9) reinterpretados
    # como dos caracteres Latin-1 sueltos ("Ã" + "©") — visto en vivo contra
    # dispositivos reales de este repo, no es un caso hipotético.
    mojibake = "Mezcla estÃ©reo"
    assert _fix_mojibake(mojibake) == "Mezcla estéreo"


def test_fix_mojibake_deja_intacto_un_nombre_ya_correcto():
    # Un nombre que ya llegó bien decodificado no debe tocarse: reinterpretar
    # "é" (un solo carácter, U+00E9) como Latin-1 produce un byte suelto que
    # no es UTF-8 válido, así que debe fallar y devolver el original.
    correcto = "Mezcla estéreo (Realtek(R) Audio)"
    assert _fix_mojibake(correcto) == correcto


def test_fix_mojibake_deja_intacto_ascii_puro():
    assert _fix_mojibake("SteelSeries Sonar - Microphone") == "SteelSeries Sonar - Microphone"


def test_friendly_device_error_incluye_causas_tipicas_y_el_detalle_original():
    mensaje = friendly_device_error(OSError("[Errno -9996] Invalid input device"))
    assert "en uso por otra aplicación" in mensaje
    assert "[Errno -9996] Invalid input device" in mensaje


# Nombres reales observados en Windows (ver la investigación en
# docs/planning/onboarding-mic-calibracion.md) que _looks_like_real_microphone
# debe rechazar: monitores de lo que suena por los parlantes y mapeos
# genéricos al dispositivo default del sistema, ninguno de los dos es un
# micrófono real que tenga sentido ofrecer para calibrar.
def test_looks_like_real_microphone_acepta_microfonos_reales():
    assert _looks_like_real_microphone("Microphone Array (Realtek(R) Audio)")
    assert _looks_like_real_microphone("Headset Microphone (Realtek(R) Audio)")
    assert _looks_like_real_microphone("SteelSeries Sonar - Microphone (SteelSeries Sonar Virtual Audio Device)")
    assert _looks_like_real_microphone("Varios micrófonos (Realtek HD Audio Mic input)")


def test_looks_like_real_microphone_rechaza_monitores_de_salida():
    assert not _looks_like_real_microphone("Mezcla estéreo (Realtek(R) Audio)")
    assert not _looks_like_real_microphone("Stereo Mix (Realtek(R) Audio)")
    assert not _looks_like_real_microphone("What U Hear (Creative)")


def test_looks_like_real_microphone_rechaza_mapeos_genericos_del_sistema():
    assert not _looks_like_real_microphone("Asignador de sonido Microsoft - Input")
    assert not _looks_like_real_microphone("Controlador primario de captura de sonido")
    assert not _looks_like_real_microphone("Microsoft Sound Mapper - Input")


def test_looks_like_real_microphone_rechaza_bus_de_monitoreo_de_terceros():
    # El bus "Stream Wave" de SteelSeries Sonar es para que lo capture OBS,
    # no para hablarle a Jarvis -- distinto de su "Chat Capture Wave" (mismo
    # canal que expone como "Microphone" vía WASAPI), que sí es un mic real
    # y por eso no debe rechazarse por nombre.
    assert not _looks_like_real_microphone("SteelSeries Sonar - Stream (SteelSeries_Sonar_VAD Stream Wave)")
    assert _looks_like_real_microphone("SteelSeries Sonar - Microphone (SteelSeries_Sonar_VAD Chat Capture Wave)")


def test_looks_like_real_microphone_filtra_incluso_con_mojibake():
    # El filtro corre sobre el nombre YA corregido (ver
    # CalibrationEngine.list_devices) -- este test documenta que filtrar
    # sobre el nombre crudo mal decodificado ("estÃ©reo") no matchearía la
    # palabra clave "estéreo" y dejaría pasar un monitor de salida por error.
    crudo_mal_decodificado = "Mezcla estÃ©reo (Realtek(R) Audio)"
    assert _looks_like_real_microphone(crudo_mal_decodificado)  # no matchea "estéreo" sin arreglar
    assert not _looks_like_real_microphone(_fix_mojibake(crudo_mal_decodificado))  # sí, una vez arreglado
