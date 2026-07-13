"""Worker de STT: dispatcher entre el motor nativo (PyAudio + Silero VAD +
faster-whisper directo, ver stt_engine.py) y RealtimeSTT (camino de
respaldo, `engine: realtimestt` en config.yaml).

Protocolo (ver README.md de este directorio):
  Rust -> Python (stdin):  init | mute | unmute | shutdown
  Python -> Rust (stdout): ready | transcript | error | fatal_error
  (motor nativo, además):  vad_start | vad_end | discarded

Con el motor nativo, "mute"/"unmute" no apagan el micrófono físico (PyAudio
no lo permite tan barato como RealtimeSTT.set_microphone) sino que activan
el modo "suppressed": el hilo de audio sigue leyendo el stream pero descarta
los frames antes del VAD, así que no hay costo de Whisper/GPU mientras
Jarvis habla. Con `engine: realtimestt` se conserva el comportamiento
original (apaga el micrófono real).

El perfil de rendimiento (modelo, beam size, hilos) sale de
hardware_detect.resolve_profile() en ambos caminos: calibracion medida en el
primer arranque, cacheada por fingerprint de hardware en los siguientes.

CLI de diagnóstico (no participa del protocolo IPC — se resuelve antes de
importar `ipc`, que redirige stdout a nivel de fd, para que estos comandos
impriman donde el usuario los puede ver):
  python stt_worker.py --list-devices          enumera dispositivos de entrada de PyAudio
  python stt_worker.py --calibrate [--device N] vúmetro en vivo del RMS del dispositivo
  python stt_worker.py --test-clap [--device N] RMS/ZCR en vivo + aviso de aplauso/doble aplauso
"""

import os
import sys
import threading
import time


def _cli_list_devices() -> None:
    import pyaudio

    pa = pyaudio.PyAudio()
    try:
        print(f"{'idx':>4}  {'canales':>7}  {'rate':>7}  nombre")
        for i in range(pa.get_device_count()):
            info = pa.get_device_info_by_index(i)
            if info.get("maxInputChannels", 0) > 0:
                print(
                    f"{i:>4}  {int(info['maxInputChannels']):>7}  "
                    f"{int(info['defaultSampleRate']):>7}  {info['name']}"
                )
    finally:
        pa.terminate()


def _cli_calibrate() -> None:
    import numpy as np
    import pyaudio

    device_index = None
    if "--device" in sys.argv:
        device_index = int(sys.argv[sys.argv.index("--device") + 1])

    pa = pyaudio.PyAudio()
    info = (
        pa.get_device_info_by_index(device_index)
        if device_index is not None
        else pa.get_default_input_device_info()
    )
    rate = int(info["defaultSampleRate"])
    frame = 512
    stream = pa.open(
        format=pyaudio.paInt16,
        channels=1,
        rate=rate,
        input=True,
        input_device_index=int(info["index"]),
        frames_per_buffer=frame,
    )
    print(
        f"Escuchando '{info['name']}' (índice {info['index']}) a {rate}Hz. Ctrl+C para salir."
    )
    try:
        while True:
            raw = stream.read(frame, exception_on_overflow=False)
            audio = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
            rms = float(np.sqrt(np.mean(audio**2)) + 1e-9)
            dbfs = 20 * np.log10(rms)
            bars = int(max(0.0, dbfs + 60))
            print(
                f"\r{dbfs:6.1f} dBFS  " + "#" * bars + " " * max(0, 40 - bars),
                end="",
                flush=True,
            )
    except KeyboardInterrupt:
        print()
    finally:
        stream.stop_stream()
        stream.close()
        pa.terminate()


def _cli_arg(flag: str) -> str | None:
    if flag in sys.argv:
        idx = sys.argv.index(flag)
        if idx + 1 < len(sys.argv):
            return sys.argv[idx + 1]
    return None


def _cli_test_clap() -> None:
    import numpy as np
    import pyaudio

    from clap_detector import ClapDetector

    device_index = None
    if "--device" in sys.argv:
        device_index = int(sys.argv[sys.argv.index("--device") + 1])

    # No lee config.yaml (el worker Python nunca lo parsea directo, solo
    # recibe config vía el mensaje "init" que le manda Rust) — para iterar
    # rápido sin editar archivos, los umbrales se pueden pisar acá:
    #   --test-clap --min-peak -20 --min-rise 10 --min-zcr 0.08
    overrides: dict = {}
    if _cli_arg("--min-peak") is not None:
        overrides["min_peak_dbfs"] = float(_cli_arg("--min-peak"))
    if _cli_arg("--min-rise") is not None:
        overrides["min_rise_db"] = float(_cli_arg("--min-rise"))
    if _cli_arg("--min-zcr") is not None:
        overrides["min_zcr"] = float(_cli_arg("--min-zcr"))
    if overrides:
        print(f"Umbrales pisados por CLI: {overrides}")

    sample_rate = 16000
    frame_samples = 512

    pa = pyaudio.PyAudio()
    info = (
        pa.get_device_info_by_index(device_index)
        if device_index is not None
        else pa.get_default_input_device_info()
    )
    resolved_index = int(info["index"])
    native_rate = int(info.get("defaultSampleRate", sample_rate))
    # Mismo criterio que _Engine._resolve_device() en stt_engine.py: si el
    # dispositivo no soporta 16kHz nativo, se lee a su rate nativo y se
    # remuestrea — igual que en producción. Si esto se salteara, los
    # umbrales que se afinen acá no valdrían para lo que corre `cargo run`.
    try:
        pa.is_format_supported(
            sample_rate,
            input_device=resolved_index,
            input_channels=1,
            input_format=pyaudio.paInt16,
        )
        rate = sample_rate
    except ValueError:
        rate = native_rate
    decimate = rate != sample_rate
    frame_native = (
        frame_samples
        if not decimate
        else max(1, round(frame_samples * rate / sample_rate))
    )

    stream = pa.open(
        format=pyaudio.paInt16,
        channels=1,
        rate=rate,
        input=True,
        input_device_index=resolved_index,
        frames_per_buffer=frame_native,
    )
    detector = ClapDetector(overrides)
    nota_remuestreo = " (remuestreado a 16kHz, igual que en producción)" if decimate else ""
    print(
        f"Escuchando '{info['name']}' (índice {resolved_index}) a {rate}Hz{nota_remuestreo}. "
        "Aplaudí dos veces seguidas. Ctrl+C para salir."
    )
    try:
        while True:
            raw = stream.read(frame_native, exception_on_overflow=False)
            audio = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
            if decimate:
                from scipy.signal import resample

                audio = resample(audio, frame_samples).astype(np.float32)
            rms = float(np.sqrt(np.mean(audio**2)) + 1e-9)
            dbfs = 20 * np.log10(rms)
            signs = np.signbit(audio)
            zcr = float(np.mean(signs[1:] != signs[:-1]))

            was_decaying = detector._decaying_since is not None
            had_first_clap = detector._first_clap_at is not None
            prev_lockout = detector._lockout_until
            umbral = max(detector.min_peak_dbfs, detector._bg_db + detector.min_rise_db)

            double_confirmed = detector.process(audio, prob=None)

            print(
                f"\r{dbfs:6.1f} dBFS  zcr={zcr:.2f}  fondo={detector._bg_db:6.1f}  "
                f"umbral={umbral:6.1f}   ",
                end="",
                flush=True,
            )
            if not was_decaying and detector._decaying_since is not None:
                print("\n  -> onset detectado, esperando decaimiento...")
            if detector._lockout_until > prev_lockout:
                print("\n  -> rechazado: la energía se sostuvo demasiado (¿voz, no aplauso?)")
            if double_confirmed:
                print("\n¡DOBLE!")
            elif not had_first_clap and detector._first_clap_at is not None:
                print("\nCLAP!")
    except KeyboardInterrupt:
        print()
    finally:
        stream.stop_stream()
        stream.close()
        pa.terminate()


if len(sys.argv) > 1 and sys.argv[1] in ("--list-devices", "--calibrate", "--test-clap"):
    if sys.argv[1] == "--list-devices":
        _cli_list_devices()
    elif sys.argv[1] == "--calibrate":
        _cli_calibrate()
    else:
        _cli_test_clap()
    sys.exit(0)

import ipc  # primer import "real": aplica la redireccion de stdout a nivel de fd


def watchdog_loop(
    recorder, shutdown: threading.Event, stuck_state_timeout: float
) -> None:
    """Vigila el estado interno del recorder para recuperarse de dos fallas
    conocidas de RealtimeSTT que, sin esto, cuelgan el worker para siempre.
    Solo aplica al camino `engine: realtimestt` — el motor nativo tiene su
    propio watchdog liviano en stt_engine.py.

    1. Bug confirmado: una deteccion de voz demasiado cerca (en el tiempo) de
       la grabacion anterior cae dentro de `min_gap_between_recordings` y
       RealtimeSTT la descarta silenciosamente (log "Attempted to start
       recording too soon after stopping"), pero de todos modos desarma
       `start_recording_on_voice_activity` sin volver a armarlo. El recorder
       queda "escuchando" para siempre sin reaccionar a nada. Se corrige
       rearmando el flag apenas se detecta el patron.
    2. Red de seguridad generica: si el recorder queda mas de
       `stuck_state_timeout` segundos en un estado ocupado ("recording" o
       "transcribing", nunca deberian tardar mas de unos pocos segundos) sin
       cambiar, se asume trabado por una causa distinta (ej. el proceso de
       transcripcion o el lector de audio dejan de responder) y se fuerza la
       salida del proceso para que Rust lo detecte como worker caido y lo
       reinicie.
    """
    last_state = None
    last_state_change = time.monotonic()
    while not shutdown.is_set():
        time.sleep(0.25)
        state = getattr(recorder, "state", None)
        if state != last_state:
            last_state = state
            last_state_change = time.monotonic()
            continue

        if (
            state == "listening"
            and not recorder.is_recording
            and not recorder.start_recording_on_voice_activity
            and not getattr(recorder, "wakeword_detected", False)
        ):
            recorder.start_recording_on_voice_activity = True
            continue

        if (
            state != "listening"
            and time.monotonic() - last_state_change > stuck_state_timeout
        ):
            ipc.send(
                {
                    "type": "fatal_error",
                    "code": "recorder_stuck",
                    "message": (
                        f"el recorder quedo trabado en el estado '{state}' "
                        f"por mas de {stuck_state_timeout}s"
                    ),
                }
            )
            shutdown.set()
            os._exit(1)


def _run_native(init_msg: dict, profile: dict, shutdown: threading.Event) -> None:
    import stt_engine

    mode_state = stt_engine.ModeState()

    def control_loop() -> None:
        while not shutdown.is_set():
            msg = ipc.read_line()
            if msg is None or msg.get("type") == "shutdown":
                shutdown.set()
                break
            msg_type = msg.get("type")
            if msg_type == "mute":
                mode_state.set(stt_engine.ModeState.SUPPRESSED)
            elif msg_type == "unmute":
                mode_state.set(stt_engine.ModeState.LISTENING)
            elif msg_type == "set_mode":
                mode = msg.get("mode")
                if mode in (
                    stt_engine.ModeState.LISTENING,
                    stt_engine.ModeState.SPEAKING,
                    stt_engine.ModeState.SUPPRESSED,
                ):
                    mode_state.set(mode)

    threading.Thread(target=control_loop, daemon=True, name="stt-control").start()

    try:
        stt_engine.run(init_msg, profile, shutdown, mode_state)
    except Exception as exc:  # noqa: BLE001 - cualquier fallo de carga/apertura debe reportarse
        ipc.send(
            {"type": "fatal_error", "code": "model_load_failed", "message": str(exc)}
        )
        sys.exit(1)

    sys.exit(0)


def _run_realtimestt(init_msg: dict, profile: dict, shutdown: threading.Event) -> None:
    try:
        from RealtimeSTT import AudioToTextRecorder

        recorder = AudioToTextRecorder(
            model=profile["whisper_model"],
            language=init_msg.get("language", "es"),
            device=profile["device"],
            compute_type=profile["compute_type"],
            input_device_index=init_msg.get("input_device_index"),
            silero_sensitivity=init_msg.get("silero_sensitivity", 0.4),
            webrtc_sensitivity=init_msg.get("webrtc_sensitivity", 3),
            post_speech_silence_duration=init_msg.get(
                "post_speech_silence_duration", 0.6
            ),
            min_length_of_recording=init_msg.get("min_length_of_recording", 1.0),
            min_gap_between_recordings=init_msg.get("min_gap_between_recordings", 1.0),
            silero_deactivity_detection=init_msg.get(
                "silero_deactivity_detection", True
            ),
            beam_size=profile["beam_size"],
            initial_prompt=init_msg.get("initial_prompt") or None,
            early_transcription_on_silence=profile["early_transcription"],
            spinner=False,
        )
    except Exception as exc:  # noqa: BLE001 - cualquier fallo de carga debe reportarse, no crashear silencioso
        ipc.send(
            {"type": "fatal_error", "code": "model_load_failed", "message": str(exc)}
        )
        sys.exit(1)

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
            "sample_rate": 16000,
        }
    )

    def control_loop() -> None:
        while not shutdown.is_set():
            msg = ipc.read_line()
            if msg is None or msg.get("type") == "shutdown":
                shutdown.set()
                recorder.abort()
                break
            msg_type = msg.get("type")
            if msg_type == "mute":
                recorder.set_microphone(False)
            elif msg_type == "unmute":
                recorder.set_microphone(True)

    threading.Thread(target=control_loop, daemon=True, name="stt-control").start()
    threading.Thread(
        target=watchdog_loop,
        args=(recorder, shutdown, init_msg.get("stuck_state_timeout_secs", 30)),
        daemon=True,
        name="stt-watchdog",
    ).start()

    while not shutdown.is_set():
        try:
            text = recorder.text()
        except Exception as exc:  # noqa: BLE001 - un fallo puntual de transcripcion no debe matar el worker
            ipc.send(
                {
                    "type": "error",
                    "code": "transcription_error",
                    "message": str(exc),
                    "recoverable": True,
                }
            )
            continue

        if shutdown.is_set():
            break
        if text and text.strip():
            ipc.send(
                {"type": "transcript", "text": text.strip(), "timestamp": time.time()}
            )

    recorder.shutdown()
    sys.exit(0)


def main() -> None:
    init_msg = ipc.read_line()
    if init_msg is None or init_msg.get("type") != "init":
        ipc.send(
            {
                "type": "fatal_error",
                "code": "protocol_error",
                "message": "esperaba mensaje 'init' como primer mensaje",
            }
        )
        sys.exit(1)

    # ctranslate2 lee OMP_NUM_THREADS al cargarse (via faster-whisper) y por
    # defecto usa solo 4 hilos: hay que fijarla ANTES de importar torch o
    # RealtimeSTT. Hasta este punto, solo stdlib.
    cpu_threads = init_msg.get("cpu_threads") or max(2, (os.cpu_count() or 4) // 2)
    os.environ["OMP_NUM_THREADS"] = str(cpu_threads)

    import hardware_detect

    profile = hardware_detect.resolve_profile(init_msg, cpu_threads)
    shutdown = threading.Event()

    if init_msg.get("engine", "native") == "realtimestt":
        _run_realtimestt(init_msg, profile, shutdown)
    else:
        _run_native(init_msg, profile, shutdown)


if __name__ == "__main__":
    main()
