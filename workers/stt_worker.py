"""Worker de STT: envuelve RealtimeSTT (faster-whisper + VAD + microfono).

Protocolo (ver README.md de este directorio):
  Rust -> Python (stdin):  init | mute | unmute | shutdown
  Python -> Rust (stdout): ready | transcript | error | fatal_error

Posee el microfono por completo. "mute"/"unmute" apagan y prenden el
microfono real via recorder.set_microphone(), en vez de descartar eventos,
para no gastar CPU/GPU mientras Jarvis esta hablando.
"""

import sys
import threading
import time

import ipc  # primer import: aplica la redireccion de stdout a nivel de fd


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

    import hardware_detect

    hw = hardware_detect.detect()
    device = (
        hw["device"] if init_msg.get("device", "auto") == "auto" else init_msg["device"]
    )
    whisper_model = hardware_detect.resolve_whisper_model(
        hw["vram_gb"], init_msg.get("model")
    )
    compute_type = hardware_detect.resolve_compute_type(
        device, init_msg.get("compute_type")
    )

    try:
        from RealtimeSTT import AudioToTextRecorder

        recorder = AudioToTextRecorder(
            model=whisper_model,
            language=init_msg.get("language", "es"),
            device=device,
            compute_type=compute_type,
            input_device_index=init_msg.get("input_device_index"),
            silero_sensitivity=init_msg.get("silero_sensitivity", 0.4),
            webrtc_sensitivity=init_msg.get("webrtc_sensitivity", 3),
            post_speech_silence_duration=init_msg.get(
                "post_speech_silence_duration", 0.6
            ),
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
            "device": device,
            "compute_type": compute_type,
            "whisper_model": whisper_model,
            "vram_gb": hw["vram_gb"],
            "sample_rate": 16000,
        }
    )

    shutdown = threading.Event()

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


if __name__ == "__main__":
    main()
