"""Motor liviano de calibración de micrófono: enumerar dispositivos PyAudio,
transmitir el nivel RMS->dBFS en vivo de un dispositivo elegido, y grabar el
audio de enrollment de voz — todo sin cargar ningún modelo pesado
(Whisper/Silero/torch) acá; el embedding de hablante lo calcula
`speaker_verification.SpeakerVerifier`, importado perezosamente por
`stt_worker.py` recién cuando termina una grabación de enrollment. Código
nuevo modelado sobre `stt_worker.py::_cli_calibrate`/`_cli_list_devices`/
`_cli_enroll_voice` (los CLI de diagnóstico por terminal), no una extracción
de `stt_engine.py::_Engine` — esa clase es mucho más grande (VAD,
transcripción, tres hilos) y no tiene una pieza de captura reutilizable tal
cual.

Usado por el modo `calibrate` de `stt_worker.py`, hablado desde Rust por
`crate::stt::calibration::CalibrationWorker` (ver `src/stt/protocol.rs` para
el shape exacto de los mensajes `list_devices`/`start_calibration`/
`stop_calibration`/`start_enroll`/`cancel_enroll`/`devices`/
`calibration_started`/`enroll_started`/`enroll_progress`/
`enroll_processing`/`enroll_complete`/`level`/`error`).
"""

from __future__ import annotations

import threading
import time

import numpy as np
import pyaudio

# Cada cuánto se reporta un nivel por IPC (ver `stt_worker.py::level_loop`).
# El tamaño de frame que se le pide a PyAudio (`start()`) se deriva de esto
# -- ver el comentario de `start()` sobre por qué NO conviene leer un frame
# chico y esperar aparte con `time.sleep`.
LEVEL_REPORT_INTERVAL_SECS = 0.1


def rms_dbfs(audio: np.ndarray) -> float:
    rms = float(np.sqrt(np.mean(np.square(audio))) + 1e-9)
    return 20.0 * np.log10(rms)


def _fix_mojibake(name: str) -> str:
    """PortAudio en Windows reporta el nombre de algunos dispositivos (sobre
    todo los expuestos vía el host API MME/legacy, ej. "Mezcla estéreo")
    con sus bytes UTF-8 originales mal reinterpretados como Latin-1/cp1252
    — el mismo nombre físico a veces llega bien y a veces mal según por qué
    host API lo esté enumerando ese proceso. Revertir ese doble-decodeo es
    seguro: un nombre ya correcto casi siempre contiene bytes que no forman
    una secuencia UTF-8 válida al reinterpretarse así, y ahí se deja tal
    cual (ver `workers/tests/test_calibration_engine.py` para los dos
    casos)."""
    try:
        return name.encode("latin-1").decode("utf-8")
    except (UnicodeEncodeError, UnicodeDecodeError):
        return name


def friendly_device_error(exc: Exception) -> str:
    """PyAudio/PortAudio lanzan mensajes técnicos (ej. `[Errno -9996]
    Invalid input device` o códigos de error de Windows) que no le dicen
    nada útil a alguien eligiendo un micrófono desde el wizard de
    onboarding. Antepone una explicación en español de las causas típicas,
    sin ocultar el detalle técnico (útil si el usuario reporta el problema)."""
    return (
        "No se pudo abrir ese micrófono — puede estar en uso por otra "
        f"aplicación, desconectado, o no disponible en este momento. ({exc})"
    )


# Substrings (siempre en minúscula) de nombres de dispositivo de ENTRADA
# que Windows expone pero que no son un micrófono real: monitores/loopback
# de lo que suena por los parlantes ("Mezcla estéreo"/"Stereo Mix"/"What U
# Hear"), mapeos genéricos al dispositivo default del sistema en vez de un
# dispositivo físico concreto ("Asignador de sonido"/"Sound Mapper",
# "Controlador primario de captura de sonido"/"Primary Sound Capture
# Driver"), y buses de monitoreo de software de terceros pensados para
# capturar con OBS, no para hablarle a Jarvis (el "Stream"/VAD de
# SteelSeries Sonar, distinto de su "Microphone"). No es exhaustivo — es
# best-effort contra los nombres que Windows/los fabricantes usan en la
# práctica, ver `workers/tests/test_calibration_engine.py`.
_NON_MIC_NAME_KEYWORDS = (
    "estéreo",
    "stereo mix",
    "what u hear",
    "primary sound capture",
    "controlador primario de captura",
    "asignador de sonido",
    "sound mapper",
    "stream wave",
    "loopback",
)


def _looks_like_real_microphone(name: str) -> bool:
    lowered = name.lower()
    return not any(keyword in lowered for keyword in _NON_MIC_NAME_KEYWORDS)


def device_info_to_dict(index: int, info: dict, default_index: int | None) -> dict:
    """Mapea el dict camelCase que devuelve PyAudio (`maxInputChannels`,
    `defaultSampleRate`, ver `get_device_info_by_index`) al shape snake_case
    de `AudioDeviceInfo` (Rust, `src/stt/protocol.rs`)."""
    name = info.get("name") or f"Dispositivo {index}"
    return {
        "index": index,
        "name": _fix_mojibake(name),
        "max_input_channels": int(info.get("maxInputChannels", 0)),
        "default_sample_rate": int(info.get("defaultSampleRate", 0)),
        "is_default": default_index is not None and index == default_index,
    }


class CalibrationEngine:
    """Encapsula PyAudio + un stream de entrada opcional. No carga ningún
    modelo: pensada para spawnear/cerrar barato bajo demanda desde la web de
    configuración, en paralelo (o no) al worker de STT de producción — ver
    "Decisión arquitectónica central" en
    docs/planning/onboarding-mic-calibracion.md sobre por qué son procesos
    separados."""

    def __init__(self) -> None:
        self._pa = pyaudio.PyAudio()
        self._stream: "pyaudio.Stream | None" = None
        self._frames_per_read: int = 0
        self._lock = threading.Lock()
        self.device_index: int | None = None
        self.device_name: str = ""
        self.sample_rate: int = 0
        # true mientras `start_enroll` está leyendo el stream desde su
        # propio hilo -- `level_loop` (stt_worker.py) lo chequea para NO
        # llamar `read_level_dbfs()` en paralelo: ambos hilos leyendo del
        # mismo stream se robarían frames entre sí (un `read()` es
        # consumidor, no hay forma de que dos lectores vean el mismo audio).
        self.is_enrolling: bool = False

    def list_devices(self) -> list[dict]:
        """Enumera micrófonos reales, no "todos los dispositivos con canales
        de entrada" — PortAudio en Windows expone cada micrófono físico una
        vez por cada host API que lo soporta (MME, DirectSound, WASAPI,
        WDM-KS), así que sin filtrar esto la lista muestra el mismo
        micrófono 3-4 veces (con nombres truncados a 31 caracteres en el
        caso de MME) más entradas que no son micrófonos en absoluto
        (monitores de lo que suena por los parlantes, mapeos genéricos al
        dispositivo default del sistema).

        Se prefiere WASAPI (el host API moderno de Windows): ahí cada
        dispositivo físico aparece una sola vez, con el nombre completo. Si
        no está disponible (build de PortAudio sin WASAPI, u otra
        plataforma), no se filtra por host API — mejor mostrar duplicados
        que una lista vacía. Sobre el resultado, además se descartan los
        nombres que no parecen un micrófono real (ver
        `_looks_like_real_microphone`)."""
        preferred_host_api_index, default_index = self._preferred_host_api()

        devices = []
        for i in range(self._pa.get_device_count()):
            info = self._pa.get_device_info_by_index(i)
            if info.get("maxInputChannels", 0) <= 0:
                continue
            if preferred_host_api_index is not None and info.get("hostApi") != preferred_host_api_index:
                continue
            name = info.get("name") or f"Dispositivo {i}"
            if not _looks_like_real_microphone(_fix_mojibake(name)):
                continue
            devices.append(device_info_to_dict(i, info, default_index))
        return devices

    def _preferred_host_api(self) -> tuple[int | None, int | None]:
        """Devuelve `(índice del host API preferido o None, índice del
        dispositivo de entrada default dentro de ese host API o None)`."""
        try:
            wasapi = self._pa.get_host_api_info_by_type(pyaudio.paWASAPI)
        except OSError:
            try:
                return None, self._pa.get_default_input_device_info()["index"]
            except OSError:
                return None, None

        default_index = wasapi.get("defaultInputDevice")
        if default_index is None or default_index < 0:
            default_index = None
        return wasapi["index"], default_index

    def start(self, device_index: int | None) -> dict:
        """Abre (o reabre, si ya había uno abierto contra otro índice) un
        stream de entrada. Puede lanzar `OSError`/`ValueError` si el
        dispositivo no existe, está ocupado o en modo exclusivo — el
        llamador (`stt_worker.py`) debe capturarlo y reportarlo como error
        IPC recuperable, nunca dejarlo propagarse (mataría el proceso de
        calibración con un traceback crudo que del lado del navegador se ve
        solo como el WebSocket cerrándose sin explicación).

        El tamaño de frame (`frames_per_buffer`) se calcula para que dure
        aproximadamente `LEVEL_REPORT_INTERVAL_SECS` a la tasa nativa del
        dispositivo, en vez de un tamaño fijo chico (~10ms) — ver el
        comentario de `read_level_dbfs` sobre por qué importa."""
        with self._lock:
            self._stop_locked()
            info = (
                self._pa.get_device_info_by_index(device_index)
                if device_index is not None
                else self._pa.get_default_input_device_info()
            )
            rate = int(info["defaultSampleRate"])
            resolved_index = int(info["index"])
            frames_per_read = max(1, round(rate * LEVEL_REPORT_INTERVAL_SECS))
            stream = self._pa.open(
                format=pyaudio.paInt16,
                channels=1,
                rate=rate,
                input=True,
                input_device_index=resolved_index,
                frames_per_buffer=frames_per_read,
            )
            self._stream = stream
            self._frames_per_read = frames_per_read
            self.device_index = resolved_index
            self.device_name = info["name"]
            self.sample_rate = rate
            return {
                "device_index": resolved_index,
                "device_name": self.device_name,
                "sample_rate": rate,
            }

    def read_level_dbfs(self) -> float:
        """Lee un frame del stream activo y devuelve su nivel en dBFS.
        Lanza si no hay stream abierto o si el dispositivo falló entre
        medio (desconectado, cerrado por el sistema) — el llamador decide
        si reintentar o reportar error.

        El tamaño de frame se ajustó en `start()` para durar
        `LEVEL_REPORT_INTERVAL_SECS` de audio real, así que este `read()`
        bloqueante YA hace de temporizador — `level_loop` (en
        `stt_worker.py`) no necesita (ni debe) agregar un `time.sleep()`
        aparte. Antes se leía un frame chico (~10ms) y se dormía 100ms sin
        drenar el resto: el buffer de PortAudio acumulaba ~90ms de audio no
        leído en cada vuelta, así que el nivel mostrado iba quedando cada
        vez más atrasado respecto de lo que se estaba diciendo en ese
        momento (hasta que el buffer se desbordaba y saltaba). Leer
        exactamente el tramo que corresponde al intervalo de reporte
        mantiene el stream al día.

        El `read()` corre DENTRO de `self._lock` a propósito: el hilo de
        control (`start()`/`stop()`, reaccionando al dropdown de la UI) toma
        el mismo lock antes de cerrar el stream. Sin esto, agarrar la
        referencia al stream y soltar el lock antes de leer deja una
        ventana real donde `stop()`/`start()` puede cerrar (o reemplazar) el
        stream mientras este hilo todavía está leyendo de él — carrera sobre
        el handle nativo de PortAudio, que no es thread-safe para esa
        combinación. El costo es que cambiar de dispositivo espera como
        mucho la duración de un frame (~`LEVEL_REPORT_INTERVAL_SECS`) a que
        termine la lectura en curso, imperceptible."""
        with self._lock:
            if self._stream is None:
                raise RuntimeError("no hay stream de calibración abierto")
            raw = self._stream.read(self._frames_per_read, exception_on_overflow=False)
        audio = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
        return rms_dbfs(audio)

    def record_enroll(
        self,
        seconds: float,
        on_progress,
        cancel_event: threading.Event,
    ) -> np.ndarray:
        """Graba `seconds` de audio del stream YA ABIERTO (por una llamada
        previa a `start()`) EN EL HILO QUE LLAMA a esta función — el
        llamador (`stt_worker.py::_run_calibration_mode`) debe correr esto
        en un hilo aparte, nunca en el hilo de control de IPC (bloqueado en
        `ipc.read_line()`): si grabar+calcular el embedding corriera ahí,
        un `cancel_enroll`/`shutdown` durante ese lapso quedaría sin
        atender.

        `on_progress(elapsed_ms, total_ms)` se llama cada ~200ms. Si
        `cancel_event` se dispara antes de completar `seconds`, corta ahí y
        devuelve el audio grabado hasta ese punto (puede ser corto o
        vacío) — es responsabilidad del llamador decidir qué hacer con un
        audio parcial (en la práctica, no calcular el embedding).

        NO toca `self.is_enrolling` — eso es responsabilidad del llamador
        (`stt_worker.py::run_enrollment`), que lo mantiene en `true` durante
        TODO el enrollment (grabación + cálculo del embedding), no solo
        mientras este método está leyendo el stream. Mientras esté en
        `true`, `read_level_dbfs()` no debe llamarse desde otro hilo: ambos
        leerían del mismo stream y se robarían frames entre sí (un `read()`
        es consumidor) — y aunque acá ya no hay lectura de stream durante el
        cálculo del embedding, `level_loop` retomando justo en ese momento
        solo generaría tráfico de WS sin sentido mientras la UI muestra
        "procesando"."""
        total_samples = max(1, int(seconds * self.sample_rate))
        # ~20ms por chunk: grano fino para que el progreso y la cancelación
        # respondan rápido, sin relación con `_frames_per_read` (pensado
        # para el caso de uso del vúmetro).
        chunk_samples = max(1, round(self.sample_rate * 0.02))
        chunks: list[np.ndarray] = []
        read_samples = 0
        total_ms = int(seconds * 1000)
        last_progress = time.monotonic()
        while read_samples < total_samples and not cancel_event.is_set():
            with self._lock:
                if self._stream is None:
                    break
                n = min(chunk_samples, total_samples - read_samples)
                raw = self._stream.read(n, exception_on_overflow=False)
            chunks.append(np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0)
            read_samples += n
            now = time.monotonic()
            if now - last_progress >= 0.2:
                on_progress(int(read_samples / self.sample_rate * 1000), total_ms)
                last_progress = now
        return np.concatenate(chunks) if chunks else np.zeros(0, dtype=np.float32)

    def stop(self) -> None:
        with self._lock:
            self._stop_locked()

    def _stop_locked(self) -> None:
        if self._stream is not None:
            try:
                self._stream.stop_stream()
                self._stream.close()
            except Exception:  # noqa: BLE001 - el stream ya puede estar en mal estado, no debe tumbar el worker
                pass
            self._stream = None
            self.device_index = None

    def close(self) -> None:
        self.stop()
        self._pa.terminate()
