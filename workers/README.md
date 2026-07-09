# Workers Python

Estos dos scripts son procesos hijos spawneados por el binario Rust (`jarvis`). No se ejecutan directamente en uso normal, solo para debug manual (ver sección de pruebas más abajo).

## Por qué Python 3.11/3.12 (no 3.13 de Microsoft Store, no 3.14)

`PyAudio` (dependencia interna de `RealtimeSTT` para capturar el micrófono) todavía no publica wheel para Windows en Python 3.14. La versión de Python 3.13 de Microsoft Store además es poco confiable para proyectos con dependencias nativas (torch, onnxruntime) por cómo sandboxea las rutas.

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

### Aceleración GPU (opcional)

`pip install torch` en Windows instala por defecto la build CPU-only. Para usar CUDA, instalar torch primero con el índice correspondiente a tu versión de CUDA, ej.:

```powershell
workers/.venv/Scripts/pip install torch --index-url https://download.pytorch.org/whl/cu121
```

antes de instalar el resto de `requirements.txt`.

## Debug manual de un worker

Cada worker lee mensajes NDJSON por stdin y escribe NDJSON (+ bytes crudos de audio en el caso de TTS) por stdout. Se puede probar a mano:

```powershell
workers/.venv/Scripts/python.exe workers/tts_worker.py
```

y luego pegar líneas JSON como:

```json
{"type": "init", "voice_path": "voices/es_MX-claude-high.onnx", "config_path": "voices/es_MX-claude-high.onnx.json"}
{"type": "synthesize", "request_id": "1", "text": "Hola, esto es una prueba."}
```

El worker responde `ready` y luego un header `audio` seguido de bytes PCM crudos (16-bit, mono) en stdout. Cualquier log de las librerías (torch, onnxruntime, PortAudio) aparece en stderr, nunca en stdout — así se valida que el protocolo no está corrompido.
