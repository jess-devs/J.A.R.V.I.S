# Jarvis

Asistente de voz conversacional en tiempo real (STT → LLM → TTS), en español, pensado para correr 100% local (sin ninguna API key) con opción de usar servicios en la nube.

Loop continuo: escuchás → VAD detecta la pausa natural → se transcribe → el LLM responde en streaming → cada frase se sintetiza y se reproduce mientras el LLM sigue generando el resto → vuelve a escuchar.

## Arquitectura

- **Rust** (`src/`) es el orquestador: el loop principal, la configuración, todas las llamadas de red (Ollama, Anthropic, OpenAI, DeepSeek, ElevenLabs), la reproducción de audio y el pipeline de streaming LLM→frases→TTS→reproducción.
- **Python** (`workers/`) queda reducido a dos procesos de inferencia ML pura, spawneados por Rust y hablados por stdio (no HTTP):
  - `stt_worker.py`: envuelve [RealtimeSTT](https://github.com/KoljaB/RealtimeSTT) (`faster-whisper` + VAD), posee el micrófono.
  - `tts_worker.py`: envuelve [Piper](https://github.com/OHF-voice/piper1-gpl) para síntesis de voz local offline en español.

Ver [`workers/README.md`](workers/README.md) para detalle del protocolo entre Rust y los workers.

## Requisitos

- **Rust** (toolchain estable, `cargo`/`rustc`).
- **Python 3.11 o 3.12** — no uses el Python 3.14 del sistema ni el de Microsoft Store: `PyAudio` (dependencia de RealtimeSTT) todavía no tiene wheel de Windows para 3.14, y el Python de Store da problemas con dependencias nativas (torch, onnxruntime) en un venv.
- **[Ollama](https://ollama.com)** instalado y corriendo (para el modo LLM local, que es el default).
- Un micrófono y parlantes/auriculares.

## Instalación

### 1. Entorno Python de los workers

```powershell
# Windows (PowerShell)
.\scripts\setup_python_env.ps1
```

```bash
# Linux/Mac
./scripts/setup_python_env.sh
```

Esto crea `workers/.venv` con Python 3.12 e instala `RealtimeSTT` y `piper-tts` (junto con sus dependencias transitivas: `torch`, `faster-whisper`, `pyaudio`, `onnxruntime`).

Si tenés GPU NVIDIA y querés aceleración CUDA para Whisper, instalá `torch` con el índice de CUDA correspondiente *antes* de correr el script de setup (por defecto `pip install torch` en Windows instala la build CPU-only):

```powershell
workers\.venv\Scripts\pip install torch --index-url https://download.pytorch.org/whl/cu121
```

### 2. Ollama y el modelo

```bash
ollama serve            # si no corre ya como servicio
ollama pull qwen2.5:7b  # modelo default en config.yaml
```

### 3. Voz de Piper

```powershell
workers\.venv\Scripts\python.exe -m piper.download_voices es_MX-claude-high
```

El comando descarga los archivos a la carpeta actual — moveló a `voices/`:

```powershell
Move-Item es_MX-claude-high.onnx*, voices/
```

`config.yaml` ya apunta por defecto a `voices/es_MX-claude-high.onnx`. Otras voces en español disponibles: buscá `es_MX` o `es_ES` en el catálogo de [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices/tree/main/es).

### 4. Compilar y correr

```bash
cargo build --release
cargo run --release
```

Al arrancar, Jarvis corre una serie de chequeos (preflight) y falla rápido con un mensaje claro si falta algo: el venv de Python, sus dependencias, Ollama, el modelo, la voz de Piper o el micrófono. Si todo está bien, vas a ver algo como:

```
INFO STT worker listo device=cpu compute_type=int8 whisper_model=small
INFO TTS worker listo sample_rate=22050 channels=1 sample_width=2
INFO Jarvis listo. Escuchando...
```

Hablá una frase en español y esperá la pausa — vas a ver la transcripción, la respuesta en streaming y escuchar el audio.

## Configuración (`config.yaml`)

Todas las claves son opcionales — lo que no se especifique usa el valor por defecto. Secciones principales:

- **`workers`**: ruta al Python del venv y a los scripts de los workers, timeouts de arranque/apagado, política de reinicio ante crash.
- **`stt`**: idioma, `device`/`whisper_model`/`compute_type` (todos `auto` por defecto — se detectan según haya o no GPU CUDA disponible y cuánta VRAM tenga; podés forzarlos manualmente), sensibilidad del VAD.
- **`llm`**: `provider: ollama | anthropic | openai | deepseek`, configuración de cada uno (modelo, variable de entorno de la API key), prompt de sistema, cuántos mensajes de historial conservar.
- **`tts`**: `provider: piper | elevenlabs`, ruta a la voz de Piper o config de ElevenLabs (`voice_id`, `output_format`).
- **`audio`**: dispositivo de salida (`null` = default del sistema) y volumen.
- **`pipeline`**: longitud mínima/máxima de las frases que se mandan a sintetizar.

### Detección de hardware

`stt.device`, `stt.whisper_model` y `stt.compute_type` son `auto` por defecto: el worker de STT detecta si hay GPU CUDA disponible (`torch.cuda.is_available()`) y cuánta VRAM tiene, y elige el tamaño de modelo Whisper acorde (≥8GB de VRAM → `medium`; con menos VRAM o sin GPU → `small`). Se eligió `small` como piso (en vez de `base`/`tiny`) porque el modelo más chico se equivoca bastante seguido transcribiendo español — en la práctica el cuello de botella de la conversación es el LLM, no el STT, así que vale la pena la precisión extra incluso en CPU. Podés sobreescribir cualquiera de los tres manualmente en `config.yaml` (ej. `whisper_model: medium` si tenés CPU potente y querés aún más precisión, o `base`/`tiny` si priorizás latencia sobre precisión).

## Modo local vs. modo nube

El default es **100% local y no necesita ninguna API key**: `llm.provider: ollama` + `tts.provider: piper`.

Para usar un LLM o TTS en la nube:

1. Cambiá `llm.provider` a `anthropic` / `openai` / `deepseek`, o `tts.provider` a `elevenlabs`, en `config.yaml`.
2. Copiá `.env.example` a `.env` y completá la API key correspondiente (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY` o `ELEVENLABS_API_KEY`). `.env` nunca se versiona.
3. No hace falta tocar ningún código — el cambio de modo es enteramente por configuración.

Los cuatro proveedores de nube están completamente implementados (streaming incluido para los LLM). DeepSeek usa el mismo cliente HTTP que OpenAI porque su API es explícitamente compatible con ese formato — solo cambia `base_url`, modelo y API key.

## Estilo de conversación

El `system_prompt` por defecto le pide al modelo respuestas breves (1-2 oraciones, salvo que pidas explícitamente más detalle) y sin markdown, porque esto es una conversación hablada. Como un modelo local chico no siempre respeta esa instrucción al pie de la letra, además hay un sanitizador (`src/text/sanitize.rs`) que limpia cualquier `**negrita**`, `# encabezado`, viñetas, code fences o links que se cuelen, justo antes de mandar cada frase al TTS — así nunca se sintetiza "asterisco asterisco" literal. Si querés ajustar el tono, editá `llm.system_prompt` en `config.yaml`.

## Solución de problemas

| Error al arrancar | Causa / solución |
|---|---|
| `no se encontró el ejecutable de Python en '...'` | Corré `scripts/setup_python_env.ps1` (o `.sh`) para crear el venv. |
| `el entorno Python no tiene las dependencias instaladas` | `workers\.venv\Scripts\pip install -r workers/requirements.txt` |
| `no se pudo conectar a Ollama` | Corré `ollama serve` (o confirmá que corre como servicio). |
| `el modelo '...' no está descargado en Ollama` | `ollama pull qwen2.5:7b` (o el modelo que hayas configurado). |
| `faltan archivos de voz Piper` | `workers\.venv\Scripts\python.exe -m piper.download_voices <voz>` y moveé los `.onnx`/`.onnx.json` a `voices/`. |
| `no se detectó ningún micrófono` | Revisá que el sistema tenga un dispositivo de entrada de audio conectado y habilitado. |
| `el dispositivo de salida espera muestras en formato ...; por ahora Jarvis solo sabe reproducir en f32` | El dispositivo de salida por defecto no usa f32 (poco común). Elegí otro dispositivo con `audio.output_device` en `config.yaml`, o probá con los parlantes/auriculares "reales" en vez de un dispositivo de audio virtual (ej. software de mezcla de un headset gaming). |
| `falta la variable de entorno ANTHROPIC_API_KEY` (u otra) | Solo aplica si activaste un proveedor de nube — completá `.env`. |

## Estado del proyecto

- ✅ Modo local completo: STT (RealtimeSTT/faster-whisper), LLM (Ollama, streaming), TTS (Piper), pipeline de streaming frase-por-frase con reproducción superpuesta a la síntesis.
- ✅ Modo nube: LLM (Anthropic, OpenAI, DeepSeek, streaming SSE) y TTS (ElevenLabs), intercambiables por configuración.
