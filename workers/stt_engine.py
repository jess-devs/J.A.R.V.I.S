"""Motor STT nativo: PyAudio + Silero VAD (ONNX, modelo crudo) + faster-whisper
directo. Camino `engine: native` de stt_worker.py — reemplaza a RealtimeSTT,
que tiene un bug conocido con `min_gap_between_recordings` y no puede emitir
eventos de VAD continuos durante la reproducción de TTS (requisito futuro
para barge-in) porque `recorder.text()` es bloqueante.

Tres hilos, arrancados desde `run()`:
  - stt-audio: captura PyAudio, VAD Silero por frame, segmentación con
    pre-roll e histéresis, encola audio segmentado para transcribir.
  - stt-transcribe: consume la cola, transcribe con faster-whisper, aplica
    filtros anti-alucinación sobre las métricas de Whisper.
  - stt-watchdog: fuerza la salida del proceso si algún hilo deja de dar
    señales de vida (Rust ya reinicia el worker si esto pasa).

El modo (listening/speaking/suppressed) lo controla `ModeState`, que
stt_worker.py actualiza desde su control_loop en respuesta a mute/unmute/
set_mode. En "suppressed" el hilo de audio sigue leyendo del stream (PyAudio
no se puede pausar/reanudar tan barato como RealtimeSTT.set_microphone) pero
descarta los frames antes del VAD, así que no hay costo de Whisper/GPU.
"""

from __future__ import annotations

import collections
import os
import queue
import threading
import time

import numpy as np
import pyaudio
import torch

import ipc

FRAME_SAMPLES = 512  # tamaño de frame que exige Silero VAD a 16kHz (32 ms)
SAMPLE_RATE = 16000


class ModeState:
    """Modo del motor, compartido entre el hilo de control y el de audio."""

    LISTENING = "listening"
    SPEAKING = "speaking"  # Jarvis hablando: umbral de VAD elevado (barge-in)
    SUPPRESSED = "suppressed"  # equivalente al mute actual

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._mode = self.LISTENING

    def set(self, mode: str) -> None:
        with self._lock:
            self._mode = mode

    def get(self) -> str:
        with self._lock:
            return self._mode


def _rms_dbfs(audio: np.ndarray) -> float:
    rms = float(np.sqrt(np.mean(np.square(audio))) + 1e-9)
    return 20.0 * np.log10(rms)


class _Engine:
    def __init__(self, init_msg: dict, profile: dict) -> None:
        vad_cfg = init_msg.get("vad") or {}
        filters_cfg = init_msg.get("filters") or {}

        self.threshold = float(vad_cfg.get("threshold", 0.5))
        self.neg_threshold = float(vad_cfg.get("neg_threshold", 0.35))
        self.pre_roll_ms = int(vad_cfg.get("pre_roll_ms", 400))
        self.min_speech_ms = int(vad_cfg.get("min_speech_ms", 250))
        self.silence_long_ms = int(vad_cfg.get("silence_long_ms", 800))
        self.silence_short_ms = int(vad_cfg.get("silence_short_ms", 450))
        self.long_utterance_ms = int(vad_cfg.get("long_utterance_ms", 2500))
        self.calibration_secs = float(vad_cfg.get("calibration_secs", 1.5))
        energy_floor_override = vad_cfg.get("energy_floor_dbfs")
        self.energy_floor_dbfs = (
            float(energy_floor_override) if energy_floor_override is not None else None
        )

        self.max_no_speech_prob = float(filters_cfg.get("max_no_speech_prob", 0.6))
        self.min_avg_logprob = float(filters_cfg.get("min_avg_logprob", -1.0))
        self.max_compression_ratio = float(
            filters_cfg.get("max_compression_ratio", 2.4)
        )

        barge_in_cfg = init_msg.get("barge_in") or {}
        self.barge_in_min_speech_ms = int(barge_in_cfg.get("min_speech_ms", 400))
        self.vad_threshold_while_speaking = float(
            barge_in_cfg.get("vad_threshold_while_speaking", 0.75)
        )

        self.language = init_msg.get("language", "es")
        self.initial_prompt = init_msg.get("initial_prompt") or None
        self.input_device_index = init_msg.get("input_device_index")
        self.beam_size = profile["beam_size"]

        self.transcribe_queue: "queue.Queue[tuple[np.ndarray, dict] | None]" = (
            queue.Queue()
        )
        self._heartbeats: dict[str, float] = {}
        self._heartbeat_lock = threading.Lock()

        from faster_whisper import WhisperModel

        self.model = WhisperModel(
            profile["whisper_model"],
            device=profile["device"],
            compute_type=profile["compute_type"],
            cpu_threads=profile["cpu_threads"],
        )

        from silero_vad import load_silero_vad

        self.vad_model = load_silero_vad(onnx=True)

        self._pa = pyaudio.PyAudio()
        self._device_index, self._native_rate = self._resolve_device()
        self._decimate = self._native_rate != SAMPLE_RATE
        self._frame_native = (
            FRAME_SAMPLES
            if not self._decimate
            else max(1, round(FRAME_SAMPLES * self._native_rate / SAMPLE_RATE))
        )

        try:
            self._stream = self._pa.open(
                format=pyaudio.paInt16,
                channels=1,
                rate=self._native_rate,
                input=True,
                input_device_index=self._device_index,
                frames_per_buffer=self._frame_native,
            )
        except Exception as exc:  # noqa: BLE001 - se re-lanza con un mensaje accionable
            raise RuntimeError(
                f"no se pudo abrir el dispositivo de entrada (índice {self._device_index}): "
                f"{exc}. Corré `python workers/stt_worker.py --list-devices` para ver los "
                "índices válidos."
            ) from exc

        if self.energy_floor_dbfs is None:
            self.energy_floor_dbfs = self._calibrate_energy_floor()

    def _resolve_device(self) -> tuple[int, int]:
        info = (
            self._pa.get_device_info_by_index(self.input_device_index)
            if self.input_device_index is not None
            else self._pa.get_default_input_device_info()
        )
        device_index = int(info["index"])
        native_rate = int(info.get("defaultSampleRate", SAMPLE_RATE))
        try:
            self._pa.is_format_supported(
                SAMPLE_RATE,
                input_device=device_index,
                input_channels=1,
                input_format=pyaudio.paInt16,
            )
            return device_index, SAMPLE_RATE
        except ValueError:
            return device_index, native_rate

    def _read_frame(self) -> np.ndarray:
        raw = self._stream.read(self._frame_native, exception_on_overflow=False)
        audio = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
        if self._decimate:
            from scipy.signal import resample

            audio = resample(audio, FRAME_SAMPLES).astype(np.float32)
        return audio

    def _calibrate_energy_floor(self) -> float:
        n_frames = max(1, int(self.calibration_secs * SAMPLE_RATE / FRAME_SAMPLES))
        levels = [_rms_dbfs(self._read_frame()) for _ in range(n_frames)]
        # Margen de 6dB sobre el ambiente medido: por debajo de esto se
        # considera ruido de fondo, no habla.
        return (sum(levels) / len(levels)) + 6.0

    def _heartbeat(self, name: str) -> None:
        with self._heartbeat_lock:
            self._heartbeats[name] = time.monotonic()

    def _stuck_threads(self, timeout: float) -> list[str]:
        now = time.monotonic()
        with self._heartbeat_lock:
            return [name for name, ts in self._heartbeats.items() if now - ts > timeout]

    def audio_loop(self, shutdown: threading.Event, mode_state: ModeState) -> None:
        pre_roll: collections.deque[np.ndarray] = collections.deque(
            maxlen=max(1, self.pre_roll_ms // 32)
        )
        recording: list[np.ndarray] = []
        speech_frames = 0
        recording_state = "listening"  # listening | recording
        recording_while_tts = False
        speech_confirmed_sent = False
        utterance_started_at = 0.0
        last_voiced_at = 0.0
        self.vad_model.reset_states()

        while not shutdown.is_set():
            self._heartbeat("audio")
            try:
                frame = self._read_frame()
            except Exception as exc:  # noqa: BLE001 - un fallo puntual no debe matar el hilo
                ipc.send(
                    {
                        "type": "error",
                        "code": "audio_read_error",
                        "message": str(exc),
                        "recoverable": True,
                    }
                )
                continue

            mode = mode_state.get()
            if mode == ModeState.SUPPRESSED:
                pre_roll.clear()
                recording_state = "listening"
                recording = []
                continue

            prob = float(self.vad_model(torch.from_numpy(frame), SAMPLE_RATE).item())
            now = time.monotonic()

            if recording_state == "listening":
                pre_roll.append(frame)
                # Mientras Jarvis habla, exige un umbral más alto: filtra
                # ruido/eco de fondo, solo reacciona a voz sostenida y
                # relativamente fuerte (el usuario hablando encima).
                entry_threshold = (
                    self.vad_threshold_while_speaking
                    if mode == ModeState.SPEAKING
                    else self.threshold
                )
                if prob >= entry_threshold:
                    recording_state = "recording"
                    recording = list(pre_roll)
                    speech_frames = 1
                    recording_while_tts = mode == ModeState.SPEAKING
                    speech_confirmed_sent = False
                    utterance_started_at = now
                    last_voiced_at = now
                    ipc.send({"type": "vad_start", "while_tts": recording_while_tts})
                continue

            # recording_state == "recording"
            recording.append(frame)
            if prob >= self.threshold:
                speech_frames += 1
            if prob >= self.neg_threshold:
                last_voiced_at = now

            # Barge-in: en cuanto la voz sostenida alcanza el umbral mientras
            # Jarvis habla, avisar de inmediato sin esperar a que la frase
            # cierre ni a que termine de transcribirse (eso tarda cientos de
            # ms más). La política de qué hacer con esto vive en Rust.
            if (
                recording_while_tts
                and not speech_confirmed_sent
                and speech_frames * 32 >= self.barge_in_min_speech_ms
            ):
                ipc.send({"type": "speech_confirmed", "while_tts": True})
                speech_confirmed_sent = True

            utterance_ms = (now - utterance_started_at) * 1000
            silence_needed_ms = (
                self.silence_short_ms
                if utterance_ms > self.long_utterance_ms
                else self.silence_long_ms
            )
            silence_elapsed_ms = (now - last_voiced_at) * 1000
            if silence_elapsed_ms < silence_needed_ms:
                continue

            self._finalize_utterance(recording, speech_frames, recording_while_tts)
            recording_state = "listening"
            recording = []
            pre_roll.clear()

    def _finalize_utterance(
        self, recording: list[np.ndarray], speech_frames: int, while_tts: bool
    ) -> None:
        speech_ms = speech_frames * 32
        audio = np.concatenate(recording)
        meta = {"speech_ms": speech_ms, "rms_dbfs": round(_rms_dbfs(audio), 1)}

        if speech_ms < self.min_speech_ms:
            ipc.send({"type": "discarded", "reason": "too_short", "meta": meta})
            return
        if meta["rms_dbfs"] < self.energy_floor_dbfs:
            ipc.send(
                {"type": "discarded", "reason": "below_energy_floor", "meta": meta}
            )
            return

        ipc.send({"type": "vad_end", "speech_ms": speech_ms, "while_tts": while_tts})
        self.transcribe_queue.put((audio, {**meta, "while_tts": while_tts}))

    def transcribe_loop(self, shutdown: threading.Event) -> None:
        while not shutdown.is_set():
            self._heartbeat("transcribe")
            try:
                item = self.transcribe_queue.get(timeout=0.5)
            except queue.Empty:
                continue
            if item is None:
                continue
            audio, meta = item
            while_tts = bool(meta.pop("while_tts", False))

            start = time.perf_counter()
            try:
                segments, _info = self.model.transcribe(
                    audio,
                    language=self.language,
                    beam_size=self.beam_size,
                    initial_prompt=self.initial_prompt,
                    vad_filter=False,
                )
                segments = list(segments)
            except Exception as exc:  # noqa: BLE001 - un fallo puntual no debe matar el hilo
                ipc.send(
                    {
                        "type": "error",
                        "code": "transcription_error",
                        "message": str(exc),
                        "recoverable": True,
                    }
                )
                continue
            transcribe_ms = round((time.perf_counter() - start) * 1000)

            text = " ".join(s.text.strip() for s in segments).strip()
            if not segments or not text:
                ipc.send(
                    {
                        "type": "discarded",
                        "reason": "empty",
                        "meta": {**meta, "transcribe_ms": transcribe_ms},
                    }
                )
                continue

            no_speech_prob = max(s.no_speech_prob for s in segments)
            avg_logprob = min(s.avg_logprob for s in segments)
            compression_ratio = max(s.compression_ratio for s in segments)
            meta = {
                **meta,
                "transcribe_ms": transcribe_ms,
                "no_speech_prob": round(no_speech_prob, 3),
                "avg_logprob": round(avg_logprob, 3),
            }

            if no_speech_prob > self.max_no_speech_prob:
                ipc.send(
                    {"type": "discarded", "reason": "no_speech_prob", "meta": meta}
                )
            elif avg_logprob < self.min_avg_logprob:
                ipc.send({"type": "discarded", "reason": "avg_logprob", "meta": meta})
            elif compression_ratio > self.max_compression_ratio:
                ipc.send(
                    {"type": "discarded", "reason": "compression_ratio", "meta": meta}
                )
            else:
                ipc.send(
                    {
                        "type": "transcript",
                        "text": text,
                        "timestamp": time.time(),
                        "while_tts": while_tts,
                        "meta": meta,
                    }
                )

    def watchdog_loop(
        self, shutdown: threading.Event, stuck_state_timeout: float
    ) -> None:
        while not shutdown.is_set():
            time.sleep(0.25)
            stuck = self._stuck_threads(stuck_state_timeout)
            if stuck:
                ipc.send(
                    {
                        "type": "fatal_error",
                        "code": "engine_stuck",
                        "message": f"hilo(s) sin señales de vida por más de {stuck_state_timeout}s: {stuck}",
                    }
                )
                shutdown.set()
                os._exit(1)

    def close(self) -> None:
        try:
            self._stream.stop_stream()
            self._stream.close()
        except Exception:  # noqa: BLE001 - cierre best-effort
            pass
        self._pa.terminate()


def run(
    init_msg: dict, profile: dict, shutdown: threading.Event, mode_state: ModeState
) -> None:
    """Construye el motor, manda `ready` y corre hasta que se dispare `shutdown`."""
    engine = _Engine(init_msg, profile)

    ipc.send(
        {
            "type": "ready",
            "device": profile["device"],
            "compute_type": profile["compute_type"],
            "whisper_model": profile["whisper_model"],
            "vram_gb": profile["vram_gb"],
            "beam_size": profile["beam_size"],
            "cpu_threads": profile["cpu_threads"],
            "rtf": profile["rtf"],
            "from_cache": profile["from_cache"],
            "energy_floor_dbfs": round(engine.energy_floor_dbfs, 1),
            "sample_rate": SAMPLE_RATE,
        }
    )

    threads = [
        threading.Thread(
            target=engine.audio_loop,
            args=(shutdown, mode_state),
            daemon=True,
            name="stt-audio",
        ),
        threading.Thread(
            target=engine.transcribe_loop,
            args=(shutdown,),
            daemon=True,
            name="stt-transcribe",
        ),
        threading.Thread(
            target=engine.watchdog_loop,
            args=(shutdown, init_msg.get("stuck_state_timeout_secs", 30)),
            daemon=True,
            name="stt-watchdog",
        ),
    ]
    for t in threads:
        t.start()

    while not shutdown.is_set():
        time.sleep(0.2)

    engine.close()
