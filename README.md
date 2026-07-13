# Jarvis

Asistente de voz conversacional y **agéntico** en tiempo real (STT → LLM → TTS), en español, pensado para correr 100% local con opción de usar servicios en la nube.

Además de conversar, **Jarvis** puede **usar herramientas**: consultar el estado del sistema y la fecha, abrir y cerrar aplicaciones, buscar y abrir archivos, ejecutar comandos, controlar el volumen, controlar el mouse y la pantalla, traducir texto, crear recordatorios, definir sus propias tools nuevas, buscar en la web y recordar cosas entre sesiones. Todo por voz, con confirmación hablada para las acciones peligrosas. Ver [Capacidades agénticas](#capacidades-agénticas-herramientas).

También reacciona a un doble aplauso, ver [Modo bienvenida](#modo-bienvenida-doble-aplauso).

## Arquitectura

- **Rust** (`src/`) es el orquestador: el loop principal, la configuración, todas las llamadas de red (Ollama, LM Studio, Anthropic, OpenAI, DeepSeek, ElevenLabs, Cartesia), la reproducción de audio y el pipeline de streaming LLM→frases→TTS→reproducción.
- **Python** (`workers/`) queda reducido a dos procesos de inferencia ML pura, spawneados por Rust y hablados por stdio (no HTTP):
  - `stt_worker.py`: envuelve [RealtimeSTT](https://github.com/KoljaB/RealtimeSTT) (`faster-whisper` + VAD), posee el micrófono. Con `engine: native` también corre, frame a frame, el detector de doble aplauso (`clap_detector.py`) que dispara el [modo bienvenida](#modo-bienvenida-doble-aplauso).
  - `tts_worker.py`: envuelve [Piper](https://github.com/OHF-voice/piper1-gpl) para síntesis de voz local offline en español.

Ver [`workers/README.md`](workers/README.md) para detalle del protocolo entre Rust y los workers.

## Requisitos

- **Rust** (toolchain estable, `cargo`/`rustc`).
- **Python → 3.12**
 > [!WARNING]
  > No uses el Python 3.14 del sistema ni el de Microsoft Store: `PyAudio` (dependencia de RealtimeSTT) todavía no tiene wheel de Windows para 3.14, y el Python de Store da problemas con dependencias nativas (torch, onnxruntime) en un venv.
  - **[Ollama](https://ollama.com)** instalado y corriendo (para el modo LLM local, que es el default).
    - *Alternativa :* [LM Studio](https://lmstudio.ai) con su servidor local activado (`llm.provider: lmstudio`).
- Un micrófono y parlantes/auriculares.

## Instalación

### Automática (recomendada)

Con Rust, Python 3.12 y Ollama ya instalados (ver [Requisitos](#requisitos)), un solo script deja todo listo: crea el venv de los workers, detecta tu hardware (RAM/GPU) para recomendarte y descargar un modelo de Ollama acorde, baja la voz de Piper que usa `config.yaml` y crea el `.env`.

```powershell
# Windows (PowerShell)
.\scripts\setup.ps1
```

```bash
# Linux/Mac
./scripts/setup.sh
```
> [!WARNING]
> El script solo verifica que Rust/Python/Ollama estén instalados (no los instala por vos).

> Si falta alguno, te va a decir exactamente cuál y dónde conseguirlo. Es seguro volver a correrlo: cada paso se saltea si ya está hecho.


Si preferís entender o ajustar cada paso a mano, seguí la sección de abajo, es exactamente lo que hace el script por dentro.

### Manual, paso a paso

#### 1. Entorno Python de los workers

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

**Si ves un error como `library cublas64_12.dll ... not found or cannot be
loaded`**: tenés el driver de NVIDIA pero no el runtime de cómputo CUDA
(cuBLAS/cuDNN) que `ctranslate2` necesita para transcribir en GPU, no
alcanza con el driver solo, hace falta el CUDA Toolkit (o los paquetes
redistribuibles de cuBLAS/cuDNN) en la versión que espera `ctranslate2`.
Jarvis detecta este caso automáticamente al arrancar y cae solo a CPU, así
que el error debería desaparecer en el siguiente arranque sin que hagas
nada; si igual querés aceleración por GPU, instalá el runtime CUDA
correspondiente.

#### 2. Ollama y el modelo

```bash
ollama serve                    # si no corre ya como servicio
ollama pull qwen2.5:3b-instruct # modelo default en config.yaml
```

El modo agéntico necesita un modelo que soporte **tool calling**. El default (`qwen2.5:3b-instruct`) está pensado para hardware sin GPU con 4-8GB de RAM libres. Con más RAM o GPU disponible convienen `qwen2.5:7b` o `qwen3:8b` (mejor tool calling; con `qwen3` poné además `llm.ollama.think: false` en `config.yaml` para que los tokens de razonamiento no se hablen en voz alta), `scripts/setup.ps1`/`.sh` eligen entre estas opciones automáticamente según tu RAM/VRAM. Si preferís chat puro sin herramientas, poné `agent.enabled: false`.

#### 3. Voz de Piper

```powershell
workers\.venv\Scripts\python.exe -m piper.download_voices es_ES-davefx-medium
```

El comando descarga los archivos a la carpeta actual,moveló a `voices/`:

```powershell
Move-Item es_ES-davefx-medium.onnx*, voices/
```

`config.yaml` ya apunta por defecto a `voices/es_MX-ald-medium.onnx`. Otras voces en español disponibles: buscá `es_MX` o `es_ES` en el catálogo de [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices/tree/main/es) y cambiá `voice_path`/`config_path` en `config.yaml` si preferís otra.

#### 4. Compilar y correr

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

Hablá una frase en español y esperá la pausa,vas a ver la transcripción, la respuesta en streaming y escuchar el audio.

## Configuración (`config.yaml`)

Todas las claves son opcionales:

- **`workers`**: ruta al Python del venv y a los scripts de los workers, timeouts de arranque/apagado, política de reinicio ante crash.
- **`stt`**: `engine: native | realtimestt` (motor propio vs. el camino de respaldo con RealtimeSTT), `stt.vad`/`stt.filters` (detección de voz y filtros anti-alucinación del motor nativo), `stt.clap` (detector de doble aplauso que dispara el modo bienvenida), idioma, `device`/`whisper_model`/`compute_type` (todos `auto` por defecto,se detectan según haya o no GPU CUDA disponible y cuánta VRAM tenga, podés forzarlos manualmente).
- **`wake`**: el "gate" de atención, qué palabra activa a Jarvis y por cuánto tiempo sigue atento sin repetirla.
- **`barge_in`**: interrumpir a Jarvis mientras habla (solo con `engine: native`),modo `wake_word` vs `any_voice`, y el `echo_guard` que evita que se autointerrumpa con sus propios parlantes.
- **`llm`**: `provider: ollama | anthropic | openai | deepseek | lmstudio`, configuración de cada uno (modelo, variable de entorno de la API key), prompt de sistema, cuántos mensajes de historial conservar.
- **`tts`**: `provider: piper | elevenlabs | cartesia`, ruta a la voz de Piper o config de ElevenLabs/Cartesia (`voice_id`, `output_format`).
- **`audio`**: dispositivo de salida (`null` = default del sistema) y volumen.
- **`pipeline`**: longitud mínima/máxima de las frases que se mandan a sintetizar.
- **`agent`**: capa agéntica,activar/desactivar (`enabled`), límite de iteraciones por turno, timeouts, frases de relleno, listas de confirmación sí/no, el `risk_code`, y sub-config de `files`/`apps`/`web`/`memory`/`translate`/`reminders`/`scripted_tools`. Ver [Capacidades agénticas](#capacidades-agénticas-herramientas).
- **`welcome`**: la escena de bienvenida disparada por doble aplauso,activar/desactivar, música, frase de saludo, volúmenes. Ver [Modo bienvenida](#modo-bienvenida-doble-aplauso).

Guía completa de configuración: [`CONFIGURACION.md`](CONFIGURACION.md).

## Capacidades agénticas (herramientas)

Cuando `agent.enabled: true` (el default), Jarvis dispone de un conjunto de herramientas que el LLM decide usar según lo que pidas. Mientras las ejecuta, dice una frase breve ("Déjame revisar, señor") y luego responde con el resultado.

**Herramientas disponibles:**

| Herramienta | Qué hace | Riesgo |
|---|---|---|
| `get_datetime` | Fecha y hora actual (también se inyecta en el prompt cada turno). | |
| `system_status` | Uso de CPU, RAM y batería. | |
| `list_processes` | Procesos que más CPU o memoria consumen. | |
| `open_app` | Abre una aplicación por nombre o alias. | |
| `close_app` | Cierra los procesos de una app. | confirmación |
| `open_url` | Abre una URL en el navegador por defecto. | |
| `find_files` / `open_file` | Busca archivos por nombre y los abre con su app por defecto. | |
| `run_powershell` | Ejecuta un comando de PowerShell. | confirmación / código |
| `get_volume` / `set_volume` | Consulta y ajusta el volumen maestro. | |
| `media_control` | Play/pausa, siguiente y anterior en la sesión de medios activa del sistema (Spotify, navegador, etc.). | |
| `take_screenshot` | Captura la pantalla actual. | confirmación |
| `mouse_move` | Mueve el cursor a una coordenada. | |
| `mouse_click` / `click_at` | Hace clic en la posición actual o en una coordenada dada. | confirmación |
| `translate` | Traduce texto entre idiomas. | |
| `create_reminder` / `list_reminders` | Crea y lista recordatorios que Jarvis anuncia por voz al llegar su hora. | |
| `cancel_reminder` | Cancela un recordatorio pendiente. | confirmación |
| `create_tool` | Define una tool nueva (comando de PowerShell o petición HTTP con placeholders) que queda disponible desde el próximo turno. | código |
| `list_custom_tools` | Lista las tools personalizadas creadas con `create_tool`. | |
| `delete_custom_tool` | Borra una tool personalizada. | confirmación |
| `stop_music` | Detiene la música de fondo del [modo bienvenida](#modo-bienvenida-doble-aplauso) (no controla apps externas, para eso usá `media_control`). | |
| `web_search` / `fetch_page` | Busca en la web (DuckDuckGo, sin API key) y lee páginas. | |
| `remember` / `recall` / `forget` | Memoria persistente entre sesiones (SQLite local). | `forget`: confirmación |

**Tres niveles de seguridad**, clasificados de forma determinista en Rust (nunca los decide el LLM):

- **Lectura** (sin marca): se ejecutan directo.
- **Confirmación**: acciones que modifican el sistema. Jarvis pregunta "¿Confirma, señor?" y espera un sí/no por voz.
- **Código**: acciones de riesgo extremo (borrado recursivo, apagado, cambios en el registro, crear una tool personalizada nueva, etc.). Jarvis describe el riesgo y exige que **pronuncies el código de aceptación** (`agent.risk_code`, por defecto `0201`,cámbialo). El código se verifica en Rust y nunca se pasa al LLM, así que el modelo no puede auto-confirmarse ni revelarlo. Un intento; si es incorrecto, se cancela.

Con `agent.confirm_mode: free` ("mano libre"), las acciones de riesgo **Confirmación** se ejecutan directo, sin preguntar. Las de riesgo **Código** siempre piden el código de aceptación, en cualquier modo — es la red de seguridad final y no se puede desactivar por config.

La memoria persistente vive en `data/memory.db`. Las memorias recientes se inyectan en el prompt de cada turno, así que Jarvis "recuerda" sin necesitar `recall` para lo habitual. Ejemplo: decile "recuerda que mi cumpleaños es el 3 de marzo", reiniciá Jarvis, y preguntá "¿cuándo es mi cumpleaños?".

Para búsqueda de archivos instantánea sobre todo el disco, instalá [Everything](https://www.voidtools.com/) y apuntá `agent.files.everything_cli` a `es.exe`; si no, se usa un recorrido acotado de las carpetas en `agent.files.search_roots`.

### Detección de hardware (calibración medida)

Con `stt.whisper_model: auto` (el default), la elección del modelo no se adivina por specs: **se mide**. El primer arranque en una máquina sin GPU corre una calibración de una sola vez (~15-40s): transcribe un audio de prueba con el modelo candidato y calcula el RTF real (tiempo de transcripción / duración del audio) de *esta* máquina. Con eso decide:

- RTF ≤ 0.5 con `small` → `small`, beam 5, transcripción temprana habilitada (arranca a transcribir durante la pausa de silencio, ~0.3s menos de latencia por turno).
- RTF ≤ 1.0 → `small` con beam 3 (más rápido, precisión levemente menor).
- Más lento → baja a `base` (y a `tiny` como último recurso), siempre con beam 3.

El resultado queda cacheado en `workers/.cache/stt_profile.json` junto a un fingerprint del hardware: los arranques siguientes son instantáneos, y solo se re-mide si el hardware cambió. Para forzar una re-medición: `stt.recalibrate: true` en `config.yaml` (o borrá el archivo de caché).

Con GPU no hace falta benchmark (sobra velocidad): tiers por VRAM,≥8GB → `large-v3-turbo`, ≥6GB → `medium`, ≥4GB → `small`, menos → `base`.

Otras palancas que se ajustan solas (y se pueden fijar a mano en `config.yaml`):

- **`cpu_threads`** (null = auto): ctranslate2 usa solo 4 hilos por defecto; el worker fija `OMP_NUM_THREADS` a ~los núcleos físicos de la máquina.
- **`beam_size`** (null = elegido por la calibración): bajarlo acelera, subirlo mejora precisión.
- **`initial_prompt`**: contexto en español para el decoder de Whisper, mejora la precisión de transcripción.

Cualquier override manual (`device`, `whisper_model` ≠ `auto`) salta la calibración por completo. El log de arranque muestra el perfil elegido: `STT worker listo device=cpu whisper_model=base beam_size=3 cpu_threads=4 rtf=0.59 perfil_cacheado=true`.

`llm.ollama.model: auto` (el default cuando `llm.provider: ollama`) hace lo mismo para el modelo de lenguaje: en cada arranque detecta VRAM (GPU NVIDIA) o RAM total (sin GPU) y elige un modelo de la familia qwen acorde,sin benchmark, por tiers, ya que acá lo que varía es cuánto modelo entra en memoria, no la velocidad. Si el modelo elegido no está descargado, Jarvis lo indica al arrancar con el `ollama pull` correspondiente en vez de bajarlo solo. Un nombre fijo en vez de `auto` sigue funcionando igual que siempre.

## Modo bienvenida (doble aplauso)

Un doble aplauso dispara una escena de bienvenida, al estilo Iron Man: música de fondo, un saludo hablado y un resumen del día.

El detector vive en el motor STT nativo (`workers/clap_detector.py`, corre siempre frame a frame junto al VAD) y busca dos golpes de energía característicos de un aplauso (pico de dBFS + subida brusca sobre el fondo + timbre de banda ancha, descartando voz sonora vía Silero) dentro de una ventana de tiempo entre sí. Es independiente de `welcome.enabled`: el detector siempre corre, esa clave solo decide si Jarvis reacciona al evento.

Al confirmarse el doble aplauso, Jarvis:

1. Reproduce `welcome.music_path` de fondo (un mp3 tuyo,ver más abajo), bajando el volumen a `welcome.duck_volume` mientras él o vos hablan y devolviéndolo a `welcome.music_volume` en las pausas.
2. Dice `welcome.greeting_phrase` ("Bienvenido a casa, señor." por defecto).
3. Resume tus recordatorios pendientes; si no tenés ninguno y `welcome.news_when_no_reminders: true`, cuenta en cambio las noticias más relevantes del día (vía `web_search`).

`welcome.cooldown_secs` evita que la escena se vuelva a disparar por un rato después de dispararse. Mientras suena la música, un nuevo doble aplauso la apaga (toggle); también podés pedirle a Jarvis por voz que la detenga, lo que dispara la tool `stop_music`.

El mp3 es tuyo y nunca se versiona en git (por derechos de autor),ver `assets/music/.gitkeep`. Si `welcome.enabled: true` y no existe el archivo en `welcome.music_path`, Jarvis lo avisa como error de arranque (ver [Solución de problemas](#solución-de-problemas)). Si no querés usar esta función, poné `welcome.enabled: false`.

Para calibrar el detector con tu propio micrófono, corré:

```powershell
.\workers\.venv\Scripts\python.exe workers\stt_worker.py --test-clap
```

Esto imprime en vivo, frame a frame, el dBFS de pico, el ZCR y el fondo/umbral actuales, y avisa "CLAP!" al detectar un candidato y "¡DOBLE!" al confirmar el doble aplauso. Podés pasarle `--min-peak`, `--min-rise` y `--min-zcr` para probar valores antes de tocar el yaml (equivalentes a `stt.clap.min_peak_dbfs`/`min_rise_db`/`min_zcr`). Aplaudí un par de veces frente al micrófono, mirá los valores que aparecen, y volcá los que te funcionaron a esas mismas claves dentro de `stt.clap` en `config.yaml`.

Referencia completa: [`stt.clap`](CONFIGURACION.md#sttclap-detector-de-doble-aplauso) para el detector, [`welcome`](CONFIGURACION.md#welcome) para la escena.

## Modo local vs. modo nube

El default es **100% local y no necesita ninguna API key**: `llm.provider: ollama` + `tts.provider: piper`.

Como alternativa local a Ollama, si preferís [LM Studio](https://lmstudio.ai) (por ejemplo porque ya tenés un modelo cargado ahí, o Ollama te resulta lento), poné `llm.provider: lmstudio` y activá el servidor local de LM Studio (pestaña Developer → Start Server, expone `http://localhost:1234/v1` por defecto). Tampoco necesita API key.

Para usar un LLM o TTS en la nube:

1. Cambiá `llm.provider` a `anthropic` / `openai` / `deepseek`, o `tts.provider` a `elevenlabs` / `cartesia`, en `config.yaml`.
2. Copiá `.env.example` a `.env` y completá la API key correspondiente (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, `ELEVENLABS_API_KEY` o `CARTESIA_API_KEY`). `.env` nunca se versiona.
3. No hace falta tocar ningún código,el cambio de modo es enteramente por configuración.

Los cinco proveedores de nube están completamente implementados (streaming incluido para los LLM). DeepSeek y LM Studio usan el mismo cliente HTTP que OpenAI porque su API es explícitamente (DeepSeek) o por diseño (LM Studio) compatible con ese formato,solo cambia `base_url`, modelo y, si aplica, API key. Cartesia (TTS) usa WebSocket por defecto (`tts.cartesia.transport`) para menor latencia; también soporta `rest`.

## Estilo de conversación

El `system_prompt` por defecto le pide al modelo respuestas breves (1-2 oraciones, salvo que pidas explícitamente más detalle) y sin markdown, porque esto es una conversación hablada. Como un modelo local chico no siempre respeta esa instrucción al pie de la letra, además hay un sanitizador (`src/text/sanitize.rs`) que limpia cualquier `**negrita**`, `# encabezado`, viñetas, code fences o links que se cuelen, justo antes de mandar cada frase al TTS,así nunca se sintetiza "asterisco asterisco" literal. Si querés ajustar el tono, editá `llm.system_prompt` en `config.yaml`.

## Solución de problemas

| Error al arrancar | Causa / solución |
|---|---|
| `no se encontró el ejecutable de Python en '...'` | Corré `scripts/setup_python_env.ps1` (o `.sh`) para crear el venv. |
| `el entorno Python no tiene las dependencias instaladas` | `workers\.venv\Scripts\pip install -r workers/requirements.txt` |
| `no se pudo conectar a Ollama` | Corré `ollama serve` (o confirmá que corre como servicio). |
| `el modelo '...' no está descargado en Ollama` | `ollama pull qwen2.5:7b` (o el modelo que hayas configurado). |
| `no se pudo conectar a LM Studio` | Abrí LM Studio, cargá un modelo y activá el servidor local (pestaña Developer → Start Server). |
| `el modelo '...' no está cargado en LM Studio` | Cargá el modelo desde LM Studio, o ajustá `llm.lmstudio.model` al identificador exacto que aparece ahí. |
| `faltan archivos de voz Piper` | `workers\.venv\Scripts\python.exe -m piper.download_voices <voz>` y moveé los `.onnx`/`.onnx.json` a `voices/`. |
| `no se detectó ningún micrófono` | Revisá que el sistema tenga un dispositivo de entrada de audio conectado y habilitado. |
| `welcome.enabled=true pero no se encontró '...'` | Falta el mp3 del [modo bienvenida](#modo-bienvenida-doble-aplauso). Poné tu archivo en `welcome.music_path` (default `assets/music/welcome.mp3`, ver `assets/music/.gitkeep`) o desactivá `welcome.enabled` en `config.yaml`. |
| `el dispositivo de salida espera muestras en formato ...; por ahora Jarvis solo sabe reproducir en f32` | El dispositivo de salida por defecto no usa f32 (poco común). Elegí otro dispositivo con `audio.output_device` en `config.yaml`, o probá con los parlantes/auriculares "reales" en vez de un dispositivo de audio virtual (ej. software de mezcla de un headset gaming). |
| `falta la variable de entorno ANTHROPIC_API_KEY` (u otra) | Solo aplica si activaste un proveedor de nube,completá `.env`. |

## Estado del proyecto

- [x] Modo local
- [x] Modo nube
- [x] Modo agéntico
- [x] Modo bienvenida
- [ ] Testeo en Linux/Mac
- [ ] Aplicación de escritorio nativa para Windows/Linux/Mac
- [ ] Ejecución al arranque del equipo, Jarvis en segundo plano durante el uso de la PC
