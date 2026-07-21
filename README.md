# Jarvis

Asistente de voz conversacional y **agéntico** en tiempo real (STT → LLM → TTS), en español, pensado para correr 100% local con opción de usar servicios en la nube.

Además de conversar, **Jarvis** puede **usar herramientas**: consultar el estado del sistema y la fecha, abrir y cerrar aplicaciones, buscar y abrir archivos, ejecutar comandos, controlar el volumen, controlar el mouse y la pantalla, traducir texto, crear recordatorios, definir sus propias tools nuevas, buscar en la web y recordar cosas entre sesiones. Todo por voz, con confirmación hablada para las acciones peligrosas. Ver [Capacidades agénticas](#capacidades-agénticas-herramientas).

También reacciona a un doble aplauso, ver [Modo bienvenida](#modo-bienvenida-doble-aplauso).

## Arquitectura

- **Rust** (`src/`) es el orquestador: el loop principal, la configuración, todas las llamadas de red (Ollama, LM Studio, Anthropic, OpenAI, DeepSeek), la reproducción de audio, **el reconocimiento de voz** y el pipeline de streaming LLM→frases→TTS→reproducción.
  - El STT (`src/stt/`) es nativo: captura de micrófono (`cpal`), VAD y reconocimiento con Whisper (small, forzado a español) vía [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx), corriendo en hilos propios dentro del mismo proceso — sin subproceso ni IPC. Se probó primero con NVIDIA Parakeet-TDT v3 (mejor WER en benchmarks generales), pero ese modelo detecta el idioma solo por audio y confundía español con inglés en frases cortas como la wake word; Whisper sí tiene un parámetro de idioma explícito. También corre ahí, frame a frame, el detector de doble aplauso que dispara el [modo bienvenida](#modo-bienvenida-doble-aplauso).
- **Python** (`workers/`) queda reducido a un solo proceso, spawneado por Rust y hablado por stdio (no HTTP): `tts_worker.py`, que envuelve [Piper](https://github.com/OHF-voice/piper1-gpl) para síntesis de voz local offline en español.

Ver [`workers/README.md`](workers/README.md) para detalle del protocolo entre Rust y el worker de TTS.

## Requisitos

- **Rust** (toolchain estable, `cargo`/`rustc`).
- **Python → 3.12** (solo para el worker de TTS, Piper)
 > [!WARNING]
  > No uses el Python 3.14 del sistema ni el de Microsoft Store: el Python de Store da problemas con dependencias nativas (onnxruntime) en un venv.
  - **[Ollama](https://ollama.com)** instalado y corriendo (para el modo LLM local, que es el default).
    - *Alternativa :* [LM Studio](https://lmstudio.ai) con su servidor local activado (`llm.provider: lmstudio`).
- Un micrófono y parlantes/auriculares.

## Instalación

### Automática (recomendada)

Con Rust, Python 3.12 y Ollama ya instalados (ver [Requisitos](#requisitos)), un solo script deja todo listo: crea el venv del worker de TTS, detecta tu hardware (RAM/GPU) para recomendarte y descargar un modelo de Ollama acorde, baja la voz de Piper y el modelo de reconocimiento de voz (Whisper small, ~640MB, pregunta antes de bajarlo) que usa `config.yaml`, y crea el `.env`.

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

#### 1. Entorno Python del worker de TTS

```powershell
# Windows (PowerShell)
.\scripts\setup_python_env.ps1
```

```bash
# Linux/Mac
./scripts/setup_python_env.sh
```

Esto crea `workers/.venv` con Python 3.12 e instala `piper-tts` (junto con sus dependencias transitivas: `onnxruntime`). El reconocimiento de voz no usa este venv — es nativo en Rust, ver el paso 3.5.

#### 2. Ollama y el modelo

```bash
ollama pull qwen3.5:0.8b # tier mínimo; setup.ps1/.sh eligen el modelo según tu hardware
```

No hace falta arrancar el servidor a mano: con `llm.ollama.auto_serve: true` (el default) Jarvis levanta `ollama serve` solo si no está corriendo, y ese servidor muere junto con Jarvis. El modo agéntico necesita un modelo que soporte **tool calling**: los tiers van de `qwen3.5:0.8b` (hardware mínimo) a `qwen3.5:4b`, `qwen3:8b`, `qwen3:14b` y `qwen3:32b` según RAM/VRAM (`llm.ollama.model: auto` y `scripts/setup.ps1`/`.sh` usan la misma tabla; el `think: false` que necesitan qwen3/qwen3.5 se aplica solo con `auto`). Si preferís chat puro sin herramientas, poné `agent.enabled: false`.

#### 3. Voz de Piper

```powershell
workers\.venv\Scripts\python.exe -m piper.download_voices es_ES-davefx-medium
```

El comando descarga los archivos a la carpeta actual,moveló a `voices/`:

```powershell
Move-Item es_ES-davefx-medium.onnx*, voices/
```

`config.yaml` ya apunta por defecto a `voices/es_MX-ald-medium.onnx`. Otras voces en español disponibles: buscá `es_MX` o `es_ES` en el catálogo de [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices/tree/main/es) y cambiá `voice_path`/`config_path` en `config.yaml` si preferís otra.

#### 3.5 Modelo de reconocimiento de voz

`scripts/setup.ps1`/`setup.sh` (paso automático de arriba) descargan el modelo Whisper small (~640MB) y el VAD de Silero a `models/stt/`, preguntando antes de bajarlos. Para hacerlo a mano: bajá y extraé [`sherpa-onnx-whisper-small.tar.bz2`](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-small.tar.bz2) dentro de `models/stt/` (queda `models/stt/sherpa-onnx-whisper-small/` — usamos `small-encoder.onnx` + `small-decoder.int8.onnx` + `small-tokens.txt`; el tarball trae además `small-encoder.int8.onnx`/`small-decoder.onnx`, que se pueden borrar), y descargá [`silero_vad.onnx`](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx) directo a `models/stt/` — las rutas coinciden con `stt.model_dir`/`stt.vad_model_path` en `config.yaml`.

#### 4. Compilar y correr

```bash
cargo build --release
cargo run --release
```

Al arrancar, Jarvis corre una serie de chequeos (preflight) y falla rápido con un mensaje claro si falta algo: el venv de Python, sus dependencias, Ollama, el modelo, la voz de Piper, el modelo de reconocimiento de voz o el micrófono. Si todo está bien, vas a ver algo como:

```
INFO STT listo device="Micrófono (Realtek Audio)" native_sample_rate=48000 energy_floor_dbfs=-52.3
INFO TTS worker listo sample_rate=22050 channels=1 sample_width=2
INFO Jarvis listo. Escuchando...
```

Hablá una frase en español y esperá la pausa,vas a ver la transcripción, la respuesta en streaming y escuchar el audio.

## Configuración (`config.yaml`)

Todas las claves son opcionales:

- **`workers`**: ruta al Python del venv y al script del worker de TTS, timeouts de arranque/apagado, política de reinicio ante crash.
- **`stt`**: `stt.vad`/`stt.filters` (detección de voz y filtro anti-alucinación del motor nativo), `stt.clap` (detector de doble aplauso que dispara el modo bienvenida), `language` (idioma forzado para Whisper), `device`/`model_dir`/`vad_model_path` (provider de sherpa-onnx y rutas al modelo Whisper/VAD).
- **`wake`**: el "gate" de atención, qué palabra activa a Jarvis y por cuánto tiempo sigue atento sin repetirla.
- **`barge_in`**: interrumpir a Jarvis mientras habla, modo `wake_word` vs `any_voice`, y el `echo_guard` que evita que se autointerrumpa con sus propios parlantes.
- **`llm`**: `provider: ollama | anthropic | openai | deepseek | lmstudio`, configuración de cada uno (modelo, variable de entorno de la API key), prompt de sistema, cuántos mensajes de historial conservar.
- **`tts`**: único proveedor `piper` (offline, local); ruta al modelo de voz (`voice_path`/`config_path`).
- **`audio`**: dispositivo de salida (`null` = default del sistema) y volumen (1 a 100).
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

### Sobre el modelo de reconocimiento de voz

A diferencia del motor Python anterior (que calibraba el tier de Whisper
midiendo la velocidad real de la máquina), acá el modelo es fijo (Whisper
small, encoder fp32 + decoder int8): no hay calibración ni elección de tier
al arrancar. Corre en CPU por defecto (`stt.device: auto` → `cpu`) y no
depende de tener una versión concreta de CUDA instalada, que era la causa
más común de que el motor anterior cayera silenciosamente a un modelo más
débil sin avisar. `stt.cpu_threads` (`null` = auto, ~núcleos físicos) es la
única palanca de rendimiento relevante; soporte de GPU (CUDA/DirectML)
queda para una mejora futura.

Se evaluó primero NVIDIA Parakeet-TDT v3 (mejor WER en benchmarks
generales, corre en el mismo pipeline sherpa-onnx), pero ese modelo detecta
el idioma automáticamente por audio y el binding de sherpa-onnx no expone
forma de fijarlo — en frases cortas sin contexto (la wake word: "Jarvis" no
es palabra real en ningún idioma) terminaba adivinando inglés en vez de
español, confirmado con pruebas reales, no solo en teoría. Whisper sí tiene
un parámetro de idioma explícito (`stt.language`, ver arriba), que es
justamente el mecanismo que usaba el motor Python anterior para lo mismo.

`llm.ollama.model: auto` (el default cuando `llm.provider: ollama`) sí hace
una detección de hardware, pero para el modelo de *lenguaje*: en cada
arranque detecta VRAM (GPU NVIDIA) o RAM total (sin GPU) y elige un modelo
de la familia qwen acorde, sin benchmark, por tiers, ya que acá lo que
varía es cuánto modelo entra en memoria, no la velocidad. Si el modelo
elegido no está descargado, Jarvis lo indica al arrancar con el `ollama
pull` correspondiente en vez de bajarlo solo. Un nombre fijo en vez de
`auto` sigue funcionando igual que siempre.

## Modo bienvenida (doble aplauso)

Un doble aplauso dispara una escena de bienvenida, al estilo Iron Man: música de fondo, un saludo hablado y un resumen del día.

El detector vive en el motor STT nativo (`src/stt/clap.rs`, corre siempre frame a frame junto al VAD) y busca dos golpes de energía característicos de un aplauso (pico de dBFS + subida brusca sobre el fondo + timbre de banda ancha, descartando voz sonora vía el VAD) dentro de una ventana de tiempo entre sí. Es independiente de `welcome.enabled`: el detector siempre corre, esa clave solo decide si Jarvis reacciona al evento.

Al confirmarse el doble aplauso, Jarvis:

1. Reproduce `welcome.music_path` de fondo (un mp3 tuyo,ver más abajo), bajando el volumen a `welcome.duck_volume` mientras él o vos hablan y devolviéndolo a `welcome.music_volume` en las pausas.
2. Dice `welcome.greeting_phrase` ("Bienvenido a casa, señor." por defecto).
3. Resume tus recordatorios pendientes; si no tenés ninguno y `welcome.news_when_no_reminders: true`, cuenta en cambio las noticias más relevantes del día (vía `web_search`).

`welcome.cooldown_secs` evita que la escena se vuelva a disparar por un rato después de dispararse. Mientras suena la música, un nuevo doble aplauso la apaga (toggle); también podés pedirle a Jarvis por voz que la detenga, lo que dispara la tool `stop_music`.

El mp3 es tuyo y nunca se versiona en git (por derechos de autor),ver `assets/music/.gitkeep`. Si `welcome.enabled: true` y no existe el archivo en `welcome.music_path`, Jarvis lo avisa como error de arranque (ver [Solución de problemas](#solución-de-problemas)). Si no querés usar esta función, poné `welcome.enabled: false`.

Para calibrar el detector con tu propio micrófono, corré Jarvis con
`log_level: debug` en `config.yaml`: cada aplauso descartado o confirmado
queda en el log con su dBFS/ZCR, así podés ajustar `stt.clap.min_peak_dbfs`/
`min_rise_db`/`min_zcr` a partir de esos valores.

Referencia completa: [`stt.clap`](CONFIGURACION.md#sttclap-detector-de-doble-aplauso) para el detector, [`welcome`](CONFIGURACION.md#welcome) para la escena.

## Modo local vs. modo nube

El default es **100% local y no necesita ninguna API key**: `llm.provider: ollama` + `tts.provider: piper`.

Como alternativa local a Ollama, si preferís [LM Studio](https://lmstudio.ai) (por ejemplo porque ya tenés un modelo cargado ahí, o Ollama te resulta lento), poné `llm.provider: lmstudio` y activá el servidor local de LM Studio (pestaña Developer → Start Server, expone `http://localhost:1234/v1` por defecto). Tampoco necesita API key.

Para usar un LLM en la nube (el TTS solo soporta `piper`, local):

1. Cambiá `llm.provider` a `anthropic` / `openai` / `deepseek` en `config.yaml`.
2. Copiá `.env.example` a `.env` y completá la API key correspondiente (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY` o `DEEPSEEK_API_KEY`). `.env` nunca se versiona.
3. No hace falta tocar ningún código,el cambio de modo es enteramente por configuración.

Los tres proveedores de LLM en la nube están completamente implementados con streaming. DeepSeek y LM Studio usan el mismo cliente HTTP que OpenAI porque su API es explícitamente (DeepSeek) o por diseño (LM Studio) compatible con ese formato,solo cambia `base_url`, modelo y, si aplica, API key.

## Estilo de conversación

El `system_prompt` por defecto le pide al modelo respuestas breves (1-2 oraciones, salvo que pidas explícitamente más detalle) y sin markdown, porque esto es una conversación hablada. Como un modelo local chico no siempre respeta esa instrucción al pie de la letra, además hay un sanitizador (`src/text/sanitize.rs`) que limpia cualquier `**negrita**`, `# encabezado`, viñetas, code fences o links que se cuelen, justo antes de mandar cada frase al TTS,así nunca se sintetiza "asterisco asterisco" literal. Si querés ajustar el tono, editá `llm.system_prompt` en `config.yaml`.

## Solución de problemas

| Error al arrancar | Causa / solución |
|---|---|
| `no se encontró el ejecutable de Python en '...'` | Corré `scripts/setup_python_env.ps1` (o `.sh`) para crear el venv. |
| `el entorno Python no tiene las dependencias instaladas` | `workers\.venv\Scripts\pip install -r workers/requirements.txt` |
| `no se pudo conectar a Ollama` | Con `llm.ollama.auto_serve: true` (default) Jarvis intenta levantarlo solo; si aun así falla, corré `ollama serve` a mano y revisá que el puerto 11434 esté libre. |
| `el modelo '...' no está descargado en Ollama` | `ollama pull <modelo>` con el nombre exacto que indica el error. |
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
