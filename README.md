# Jarvis

Asistente de voz conversacional y **agéntico** en tiempo real (STT → LLM → TTS), en español, pensado para correr 100% local (sin ninguna API key) con opción de usar servicios en la nube.

Loop continuo: escuchás → VAD detecta la pausa natural → se transcribe → el LLM responde en streaming → cada frase se sintetiza y se reproduce mientras el LLM sigue generando el resto → vuelve a escuchar.

Además de conversar, Jarvis puede **usar herramientas**: consultar el estado del sistema y la fecha, abrir y cerrar aplicaciones, buscar y abrir archivos, ejecutar comandos, controlar el volumen, buscar en la web y recordar cosas entre sesiones — todo por voz, con confirmación hablada para las acciones riesgosas. Ver [Capacidades agénticas](#capacidades-agénticas-herramientas).

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

El modo agéntico necesita un modelo que soporte **tool calling**. `qwen2.5:7b` (el default) funciona, aunque a veces olvida llamar una herramienta o alucina argumentos. Para mejores resultados con herramientas se recomienda `qwen3:8b` (`ollama pull qwen3:8b`) o `llama3.1:8b`; con `qwen3` poné `llm.ollama.think: false` en `config.yaml` para que los tokens de razonamiento no se hablen en voz alta. Si preferís chat puro sin herramientas, poné `agent.enabled: false`.

### 3. Voz de Piper

```powershell
workers\.venv\Scripts\python.exe -m piper.download_voices es_ES-davefx-medium
```

El comando descarga los archivos a la carpeta actual — moveló a `voices/`:

```powershell
Move-Item es_ES-davefx-medium.onnx*, voices/
```

`config.yaml` ya apunta por defecto a `voices/es_ES-davefx-medium.onnx` (voz masculina, acento de España). Si preferís acento mexicano, probá `es_MX-ald-medium` con el mismo procedimiento y cambiá `voice_path`/`config_path` en `config.yaml`. Otras voces en español disponibles: buscá `es_MX` o `es_ES` en el catálogo de [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices/tree/main/es).

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
- **`agent`**: capa agéntica — activar/desactivar (`enabled`), límite de iteraciones por turno, timeouts, frases de relleno, listas de confirmación sí/no, el `risk_code`, y sub-config de `files`/`apps`/`web`/`memory`. Ver [Capacidades agénticas](#capacidades-agénticas-herramientas).

## Capacidades agénticas (herramientas)

Cuando `agent.enabled: true` (el default), Jarvis dispone de un conjunto de herramientas que el LLM decide usar según lo que pidas. Mientras las ejecuta, dice una frase breve ("Déjame revisar, señor") y luego responde con el resultado.

**Herramientas disponibles:**

| Herramienta | Qué hace | Riesgo |
|---|---|---|
| `get_datetime` | Fecha y hora actual (también se inyecta en el prompt cada turno). | — |
| `system_status` | Uso de CPU, RAM y batería. | — |
| `list_processes` | Procesos que más CPU o memoria consumen. | — |
| `open_app` | Abre una aplicación por nombre o alias. | — |
| `close_app` | Cierra los procesos de una app. | 🔸 confirmación |
| `find_files` / `open_file` | Busca archivos por nombre y los abre con su app por defecto. | — |
| `run_powershell` | Ejecuta un comando de PowerShell. | 🔸 confirmación / 🔴 código |
| `get_volume` / `set_volume` | Consulta y ajusta el volumen maestro. | — |
| `web_search` / `fetch_page` | Busca en la web (DuckDuckGo, sin API key) y lee páginas. | — |
| `remember` / `recall` / `forget` | Memoria persistente entre sesiones (SQLite local). | `forget` 🔸 |

**Tres niveles de seguridad**, clasificados de forma determinista en Rust (nunca los decide el LLM):

- **Lectura** (sin marca): se ejecutan directo.
- 🔸 **Confirmación**: acciones que modifican el sistema. Jarvis pregunta "¿Confirma, señor?" y espera un sí/no por voz.
- 🔴 **Código**: acciones de riesgo extremo (borrado recursivo, apagado, cambios en el registro, etc., detectadas por patrones sobre el comando). Jarvis describe el riesgo y exige que **pronuncies el código de aceptación** (`agent.risk_code`, por defecto `0201` — cámbialo). El código se verifica en Rust y nunca se pasa al LLM, así que el modelo no puede auto-confirmarse ni revelarlo. Un intento; si es incorrecto, se cancela.

La memoria persistente vive en `data/memory.db`. Las memorias recientes se inyectan en el prompt de cada turno, así que Jarvis "recuerda" sin necesitar `recall` para lo habitual. Ejemplo: decile "recuerda que mi cumpleaños es el 3 de marzo", reiniciá Jarvis, y preguntá "¿cuándo es mi cumpleaños?".

Para búsqueda de archivos instantánea sobre todo el disco, instalá [Everything](https://www.voidtools.com/) y apuntá `agent.files.everything_cli` a `es.exe`; si no, se usa un recorrido acotado de las carpetas en `agent.files.search_roots`.

### Detección de hardware (calibración medida)

Con `stt.whisper_model: auto` (el default), la elección del modelo no se adivina por specs: **se mide**. El primer arranque en una máquina sin GPU corre una calibración de una sola vez (~15-40s): transcribe un audio de prueba con el modelo candidato y calcula el RTF real (tiempo de transcripción / duración del audio) de *esta* máquina. Con eso decide:

- RTF ≤ 0.5 con `small` → `small`, beam 5, transcripción temprana habilitada (arranca a transcribir durante la pausa de silencio, ~0.3s menos de latencia por turno).
- RTF ≤ 1.0 → `small` con beam 3 (más rápido, precisión levemente menor).
- Más lento → baja a `base` (y a `tiny` como último recurso), siempre con beam 3.

El resultado queda cacheado en `workers/.cache/stt_profile.json` junto a un fingerprint del hardware: los arranques siguientes son instantáneos, y solo se re-mide si el hardware cambió. Para forzar una re-medición: `stt.recalibrate: true` en `config.yaml` (o borrá el archivo de caché).

Con GPU no hace falta benchmark (sobra velocidad): tiers por VRAM — ≥8GB → `large-v3-turbo`, ≥6GB → `medium`, ≥4GB → `small`, menos → `base`.

Otras palancas que se ajustan solas (y se pueden fijar a mano en `config.yaml`):

- **`cpu_threads`** (null = auto): ctranslate2 usa solo 4 hilos por defecto; el worker fija `OMP_NUM_THREADS` a ~los núcleos físicos de la máquina.
- **`beam_size`** (null = elegido por la calibración): bajarlo acelera, subirlo mejora precisión.
- **`initial_prompt`**: contexto en español para el decoder de Whisper, mejora la precisión de transcripción.

Cualquier override manual (`device`, `whisper_model` ≠ `auto`) salta la calibración por completo. El log de arranque muestra el perfil elegido: `STT worker listo device=cpu whisper_model=base beam_size=3 cpu_threads=4 rtf=0.59 perfil_cacheado=true`.

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
- ✅ Modo agéntico: tool calling con streaming en los cuatro proveedores de LLM, herramientas de sistema/PC/web/memoria, loop multi-paso, y seguridad por voz en tres niveles (lectura / confirmación / código de aceptación de riesgos).
