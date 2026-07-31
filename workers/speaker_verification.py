"""Verificación de hablante — modo sombra (ítem 4 v1 de MEJORAS.md).

Calcula la similitud coseno entre el embedding de voz de cada frase y un
embedding de referencia enrolado una vez (`stt_worker.py --enroll-voice`).
Todavía NO se usa para gatear ninguna confirmación de seguridad — el valor
solo viaja hacia Rust (mensaje `speaker_similarity`) para quedar logueado,
así se junta evidencia real de qué umbral tendría sentido antes de aplicar
esto a algo relevante a seguridad (ver `agent.speaker_verification` en
CONFIGURACION.md).

Usa ECAPA-TDNN (`speechbrain/spkrec-ecapa-voxceleb`) vía el extra opcional
`speechbrain` — pesado (~80MB de pesos + torch, que el venv ya trae para
faster-whisper) pero puro Python+torch, sin extensión nativa que compilar
(a diferencia de alternativas como `resemblyzer`, que en Windows necesita
Visual Studio Build Tools para su dependencia `webrtcvad`). Instalación:
`pip install -r workers/requirements-speaker.txt`. Si no está instalado,
esta clase se desactiva sola (loguea un aviso una vez) en vez de romper el
resto del worker de STT.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

DEFAULT_EMBEDDING_PATH = Path("data/speaker_embedding.json")
_MODEL_SOURCE = "speechbrain/spkrec-ecapa-voxceleb"
_MODEL_CACHE_DIR = "workers/.cache/spkrec-ecapa-voxceleb"


class SpeakerVerifier:
    """Encapsula el modelo de embeddings + la voz de referencia enrolada.

    El modelo (~14s la primera vez que se usa, según medición local) se
    carga perezoso en el primer `embed()`, no en `__init__`, para no sumar
    latencia al arranque del worker cuando `speaker_verification.enabled`
    está activo pero el usuario todavía no dijo nada.
    """

    def __init__(self, embedding_path: Path | None = None) -> None:
        self.embedding_path = embedding_path or DEFAULT_EMBEDDING_PATH
        self._classifier = None
        self._unavailable = False
        self._reference: np.ndarray | None = None
        self._load_reference()

    @property
    def enrolled(self) -> bool:
        return self._reference is not None

    def _load_reference(self) -> None:
        if not self.embedding_path.exists():
            return
        try:
            data = json.loads(self.embedding_path.read_text())
            self._reference = np.array(data["embedding"], dtype=np.float32)
        except Exception:  # noqa: BLE001 - un embedding corrupto no debe tumbar el worker
            self._reference = None

    def _ensure_classifier(self):
        if self._classifier is not None or self._unavailable:
            return self._classifier
        try:
            from speechbrain.inference.speaker import EncoderClassifier
            from speechbrain.utils.fetching import LocalStrategy

            # LocalStrategy.COPY: el default (SYMLINK) pide privilegios de
            # administrador o "modo desarrollador" en Windows para crear
            # symlinks — sin esto, `from_hparams` explota con
            # `OSError: [WinError 1314]` en una instalación estándar.
            self._classifier = EncoderClassifier.from_hparams(
                source=_MODEL_SOURCE,
                savedir=_MODEL_CACHE_DIR,
                local_strategy=LocalStrategy.COPY,
            )
        except Exception as exc:  # noqa: BLE001 - falta el extra opcional, o falló la descarga
            self._unavailable = True
            print(
                f"[speaker_verification] no se pudo cargar el modelo de embeddings ({exc}); "
                "verificación de hablante desactivada. Instalá el extra opcional con: "
                "pip install -r workers/requirements-speaker.txt",
                flush=True,
            )
        return self._classifier

    def embed(self, audio: np.ndarray) -> np.ndarray | None:
        """`audio`: float32 mono a 16kHz (mismo formato que usa Whisper acá
        adentro). `None` si el modelo no está disponible o falla — nunca
        levanta excepción hacia el llamador."""
        classifier = self._ensure_classifier()
        if classifier is None:
            return None
        try:
            import torch

            tensor = torch.from_numpy(audio).unsqueeze(0)
            with torch.no_grad():
                emb = classifier.encode_batch(tensor)
            return emb.squeeze().cpu().numpy()
        except Exception as exc:  # noqa: BLE001 - un fallo puntual no debe tumbar la transcripción
            print(f"[speaker_verification] fallo calculando embedding: {exc}", flush=True)
            return None

    def similarity(self, audio: np.ndarray) -> float | None:
        """Similitud coseno [-1, 1] contra la voz de referencia enrolada.
        `None` si no hay referencia o si `embed()` falló."""
        if self._reference is None:
            return None
        emb = self.embed(audio)
        if emb is None:
            return None
        denom = float(np.linalg.norm(emb) * np.linalg.norm(self._reference))
        if denom == 0.0:
            return None
        return float(np.dot(emb, self._reference) / denom)

    def enroll(self, audio: np.ndarray) -> bool:
        """Calcula el embedding de `audio` y lo guarda como referencia.
        Devuelve `False` si el modelo no está disponible."""
        emb = self.embed(audio)
        if emb is None:
            return False
        self.embedding_path.parent.mkdir(parents=True, exist_ok=True)
        self.embedding_path.write_text(json.dumps({"embedding": emb.tolist()}))
        self._reference = emb
        return True
