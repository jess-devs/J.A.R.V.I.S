# Worker Python (TTS)

`tts_worker.py` es un proceso hijo spawneado por el binario Rust (`jarvis`) para la síntesis de voz (Piper). No se ejecuta directamente en uso normal, solo para debug manual (ver más abajo).

El reconocimiento de voz (STT) ya no es un worker Python: corre nativo dentro de `jarvis` (sherpa-onnx + Whisper, ver [`src/stt/`](../src/stt/)). El modelo de reconocimiento (~640MB) no vive acá — lo descarga `scripts/setup.ps1`/`scripts/setup.sh` a `models/stt/`.

## Setup del entorno

```powershell
py -3.12 -m venv workers/.venv
workers/.venv/Scripts/pip install -r workers/requirements.txt
```

En Linux/Mac:
```bash
python3.12 -m venv workers/.venv
workers/.venv/bin/pip install -r workers/requirements.txt
```

`config.yaml` (clave `workers.python_executable`) debe apuntar a este venv.

## Debug manual del worker

Cada worker lee mensajes NDJSON por stdin y escribe NDJSON (+ bytes crudos de audio en el caso de TTS) por stdout. Se puede probar a mano:

```powershell
workers/.venv/Scripts/python.exe workers/tts_worker.py
```

y luego pegar líneas JSON como:

```json
{"type": "init", "voice_path": "voices/es_ES-davefx-medium.onnx", "config_path": "voices/es_ES-davefx-medium.onnx.json"}
{"type": "synthesize", "request_id": "1", "text": "Hola, esto es una prueba."}
```

El worker responde `ready` y luego un header `audio` seguido de bytes PCM crudos (16-bit, mono) en stdout. Cualquier log de las librerías (onnxruntime, PortAudio) aparece en stderr, nunca en stdout — así se valida que el protocolo no está corrompido.
