"""Worker de TTS: envuelve Piper (sintesis local offline).

Protocolo (ver README.md de este directorio):
  Rust -> Python (stdin):  init | synthesize | shutdown
  Python -> Rust (stdout): ready | audio (+ bytes crudos PCM) | error | fatal_error

Loop de un solo hilo: el orquestador Rust llama a synthesize() de forma
estrictamente secuencial (una frase a la vez), asi que no hace falta
concurrencia ni un hilo de control separado como en el worker de STT.
"""

import sys

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

    try:
        from piper import PiperVoice
        from piper.config import SynthesisConfig

        voice = PiperVoice.load(
            init_msg["voice_path"],
            config_path=init_msg.get("config_path"),
            use_cuda=init_msg.get("use_cuda", False),
        )
    except Exception as exc:  # noqa: BLE001 - cualquier fallo de carga debe reportarse, no crashear silencioso
        ipc.send(
            {"type": "fatal_error", "code": "voice_load_failed", "message": str(exc)}
        )
        sys.exit(1)

    # None = usa el valor propio de la voz (ver PiperVoice.phoneme_ids_to_audio).
    syn_config = SynthesisConfig(
        length_scale=init_msg.get("length_scale"),
        noise_w_scale=init_msg.get("noise_w_scale"),
    )

    sample_rate = voice.config.sample_rate
    ipc.send(
        {"type": "ready", "sample_rate": sample_rate, "channels": 1, "sample_width": 2}
    )

    while True:
        msg = ipc.read_line()
        if msg is None or msg.get("type") == "shutdown":
            break
        if msg.get("type") != "synthesize":
            continue

        request_id = msg.get("request_id")
        text = msg.get("text", "")
        try:
            # synthesize() entrega PCM s16le mono directo (un AudioChunk por
            # frase interna): no hace falta codificar un WAV en memoria para
            # releerlo al instante. Se acumula todo el audio de la frase en
            # un solo mensaje porque el protocolo IPC espera exactamente un
            # "audio" por request_id (PendingSlot unico en el lado Rust).
            pcm = b"".join(
                chunk.audio_int16_bytes
                for chunk in voice.synthesize(text, syn_config=syn_config)
            )
            header = {
                "type": "audio",
                "request_id": request_id,
                "sample_rate": sample_rate,
                "channels": 1,
                "sample_width": 2,
            }
            ipc.send_audio(header, pcm)
        except Exception as exc:  # noqa: BLE001 - un fallo de sintesis puntual no debe matar el worker
            ipc.send(
                {
                    "type": "error",
                    "request_id": request_id,
                    "code": "synthesis_failed",
                    "message": str(exc),
                }
            )

    sys.exit(0)


if __name__ == "__main__":
    main()
