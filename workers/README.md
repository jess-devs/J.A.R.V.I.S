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

Si `requirements.txt` ya se instaló (ej. RealtimeSTT ya trajo la build CPU-only de torch como dependencia transitiva), `pip install torch --index-url ...` no la reemplaza por sí solo: pip ve el mismo número de versión base y no reinstala. Hace falta forzarlo:

```powershell
workers/.venv/Scripts/pip install torch --index-url https://download.pytorch.org/whl/cu121 --force-reinstall --no-deps
```

Verificar con `python -c "import torch; print(torch.cuda.is_available())"` — debe imprimir `True`. RealtimeSTT decide internamente si usa la GPU mirando `torch.cuda.is_available()` (no la config de Jarvis): con torch CPU-only, ignora silenciosamente `device: cuda` y cae a CPU sin avisar.

Si al arrancar Jarvis falla el preflight con un error de `torchaudio` (típicamente `OSError: [WinError 127]` al cargar su extensión nativa), es porque `torchaudio` quedó compilado contra la versión de torch anterior. Reinstalarlo a la versión que corresponde a tu torch (misma versión base), con el mismo índice CUDA:

```powershell
workers/.venv/Scripts/pip install torchaudio==<version-de-torch> --index-url https://download.pytorch.org/whl/cu121 --force-reinstall --no-deps
```

## Debug manual de un worker

Cada worker lee mensajes NDJSON por stdin y escribe NDJSON (+ bytes crudos de audio en el caso de TTS) por stdout. Se puede probar a mano:

```powershell
workers/.venv/Scripts/python.exe workers/tts_worker.py
```

y luego pegar líneas JSON como:

```json
{"type": "init", "voice_path": "voices/es_ES-davefx-medium.onnx", "config_path": "voices/es_ES-davefx-medium.onnx.json"}
{"type": "synthesize", "request_id": "1", "text": "Hola, esto es una prueba."}
```

El worker responde `ready` y luego un header `audio` seguido de bytes PCM crudos (16-bit, mono) en stdout. Cualquier log de las librerías (torch, onnxruntime, PortAudio) aparece en stderr, nunca en stdout — así se valida que el protocolo no está corrompido.
