"""Detector de doble aplauso sobre el audio crudo del micrófono.

Corre frame a frame (float32, 512 samples @ 16kHz = 32ms) en el mismo hilo
que el VAD, así que tiene que ser aritmética barata: sin dependencias de
torch/pyaudio/ipc. Lo importan tanto stt_engine.py (integrado en el motor,
donde sí puede pesar el resto de esos imports) como stt_worker.py
--test-clap (standalone, corre antes de `import ipc` para poder imprimir en
vivo — ver el docstring de stt_worker.py).
"""

from __future__ import annotations

import time

import numpy as np


def _frame_metrics(frame: np.ndarray) -> tuple[float, float]:
    rms = float(np.sqrt(np.mean(np.square(frame))) + 1e-9)
    rms_db = 20.0 * np.log10(rms)
    signs = np.signbit(frame)
    zcr = float(np.mean(signs[1:] != signs[:-1])) if frame.size > 1 else 0.0
    return rms_db, zcr


class ClapDetector:
    """Detecta un doble aplauso sobre una secuencia de frames.

    Máquina de estados por frame:
      1. onset: la energía sube de golpe (`rms_db` muy por encima del fondo)
         con timbre de banda ancha (`zcr` alto) y, si hay VAD disponible, sin
         pinta de voz sonora (`prob` bajo).
      2. decaimiento: el onset solo se confirma como aplauso si la energía
         vuelve a caer por debajo del mismo umbral que lo disparó dentro de
         `decay_ms` (un aplauso real dura 1-2 frames); si se sostiene por
         encima (voz, música) se rechaza y entra en un lockout corto.
      3. doble: dos aplausos confirmados con un gap en
         [double_min_gap_ms, double_max_gap_ms] disparan `process() -> True`
         una sola vez, seguido de un refractario.
    """

    def __init__(self, cfg: dict) -> None:
        self.min_peak_dbfs = float(cfg.get("min_peak_dbfs", -30.0))
        self.min_rise_db = float(cfg.get("min_rise_db", 7.0))
        self.decay_ms = float(cfg.get("decay_ms", 220))
        self.max_vad_prob = float(cfg.get("max_vad_prob", 0.45))
        self.min_zcr = float(cfg.get("min_zcr", 0.14))
        self.double_min_gap_ms = float(cfg.get("double_min_gap_ms", 150))
        self.double_max_gap_ms = float(cfg.get("double_max_gap_ms", 900))
        self.refractory_ms = float(cfg.get("refractory_ms", 1500))

        self._bg_db = self.min_peak_dbfs - self.min_rise_db
        self._decaying_since: float | None = None
        self._decaying_threshold = -120.0
        self._lockout_until = 0.0
        self._refractory_until = 0.0
        self._first_clap_at: float | None = None

    def process(self, frame: np.ndarray, prob: float | None) -> bool:
        now = time.monotonic()
        rms_db, zcr = _frame_metrics(frame)

        if self._decaying_since is not None:
            return self._check_decay(now, rms_db)

        if now < self._refractory_until or now < self._lockout_until:
            return False

        threshold = max(self.min_peak_dbfs, self._bg_db + self.min_rise_db)
        onset = (
            rms_db >= threshold
            and zcr >= self.min_zcr
            and (prob is None or prob <= self.max_vad_prob)
        )
        if onset:
            self._decaying_since = now
            self._decaying_threshold = threshold
            return False

        # No es un onset: se actualiza el fondo con este frame, sin importar
        # si está por encima o por debajo del fondo actual. Antes esto solo
        # se hacía en frames "tranquilos" (más bajos que el fondo), lo que
        # dejaba a `_bg_db` sin forma de volver a subir una vez que caía por
        # una racha de silencio real: el ruido ambiente normal dejaba de
        # calificar como "tranquilo" respecto de ese fondo ya demasiado bajo,
        # y el umbral de decaimiento (`bg_db + 6dB`) se volvía inalcanzable
        # para cualquier aplauso real (rechazado siempre como "sostenido").
        self._bg_db += 0.05 * (rms_db - self._bg_db)
        return False

    def _check_decay(self, now: float, rms_db: float) -> bool:
        # Decaído = volvió a caer por debajo del umbral que disparó el onset.
        # No se compara contra el fondo absoluto (en la práctica el aplauso
        # no vuelve a acercarse al fondo medido en silencio real dentro de
        # decay_ms — se queda flotando más arriba por reverberación/ruido del
        # driver) ni contra un pico que se sigue actualizando con cada rebote
        # del eco (eso corre la meta cada vez que hay un rebote y nunca la
        # alcanza) — el umbral de disparo es fijo y ya filtró ruido de fondo.
        if rms_db < self._decaying_threshold:
            self._decaying_since = None
            # Ventana muerta corta tras confirmar: el eco/reverberación del
            # mismo golpe puede rebotar por encima del umbral otra vez a los
            # pocos ms, generando un "onset" espurio que no es el segundo
            # aplauso real (ese llega bastante más tarde). 80ms alcanza para
            # absorber el eco sin interferir con el gap real entre aplausos
            # (double_min_gap_ms por defecto es 150ms).
            self._lockout_until = now + 0.08
            return self._confirm_clap(now)

        elapsed_ms = (now - self._decaying_since) * 1000.0
        if elapsed_ms >= self.decay_ms:
            # La energía se sostuvo más de la cuenta: no es un aplauso (voz
            # gritada, música), se descarta con un lockout corto.
            self._decaying_since = None
            self._lockout_until = now + 0.3
        return False

    def _confirm_clap(self, now: float) -> bool:
        if self._first_clap_at is None:
            self._first_clap_at = now
            return False

        gap_ms = (now - self._first_clap_at) * 1000.0
        if gap_ms < self.double_min_gap_ms:
            # Demasiado cerca del primero (probable reverb del mismo golpe):
            # se ignora sin resetear la espera del segundo aplauso real.
            return False
        if gap_ms > self.double_max_gap_ms:
            # Se pasó la ventana: este aplauso pasa a ser el nuevo "primero".
            self._first_clap_at = now
            return False

        self._first_clap_at = None
        self._refractory_until = now + self.refractory_ms / 1000.0
        return True
