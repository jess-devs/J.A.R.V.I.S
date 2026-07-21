# Configuración de Jarvis

Referencia completa de `config.yaml`. Para un resumen rápido ver la sección
["Configuración"](README.md#configuración-configyaml) del README; esta guía
entra en el detalle de cada clave: qué controla, qué valores acepta, y cuándo
tiene sentido tocarla.

Todas las claves son opcionales, lo que se omita usa el valor por defecto
documentado acá. `config.yaml` se versiona en git, así que **nunca va una API
key ahí**: las claves de proveedores en la nube se leen de variables de
entorno (ver [`.env.example`](.env.example)).

## `workers`

Cómo Rust arranca el único proceso Python que queda (`tts_worker.py`) y qué
tan tolerante es a que se caiga. El STT ya no es un worker Python: corre
nativo dentro de `jarvis` en hilos propios (sherpa-onnx + Whisper, ver
[`src/stt/`](src/stt/)) — sin subproceso, sin IPC.

| Clave | Qué hace |
|---|---|
| `python_executable` | Ruta al intérprete del venv de `workers/`. |
| `tts_script` | Ruta al script de TTS que se spawnea. |
| `stt_init_timeout_secs` | Tiempo máximo esperando a que el motor STT nativo abra el micrófono y cargue los modelos (VAD + Whisper). |
| `tts_init_timeout_secs` | Ídem para Piper. |
| `shutdown_timeout_secs` | Cuánto se espera a que un worker/hilo cierre solo antes de darlo por perdido. |
| `restart_on_crash` | Si el motor de STT muere (panic de alguno de sus hilos), se reemplaza por uno nuevo en caliente en vez de tumbar Jarvis entero. |
| `max_restarts` | Tope de reinicios por corrida. Al agotarse, Jarvis termina, evita un loop de reinicios infinito si el problema es persistente (driver de audio roto, modelo corrupto, etc.). |

## `stt`

Todo lo relacionado con transcribir lo que decís. El motor es nativo
(sherpa-onnx + Whisper small, corriendo en dos hilos dentro del propio
proceso de Jarvis): captura con `cpal`, VAD Silero, y reconocimiento con
Whisper forzado a `language: "es"` (ver `stt.language` más abajo). Se probó
primero con NVIDIA Parakeet-TDT v3 (mejor WER en benchmarks generales),
pero ese modelo detecta el idioma solo por audio y el binding de
sherpa-onnx no tiene forma de fijarlo — en frases cortas sin contexto (la
wake word: "Jarvis" no es palabra real en ningún idioma) terminaba
adivinando inglés, confirmado en pruebas reales. El modelo (~640MB) no se
versiona — lo descarga `scripts/setup.ps1`/`setup.sh` a `models/stt/`.

### `stt.vad`

Controla cómo se detecta el inicio y el fin de una frase. Esta lógica vive
enteramente en Rust (`src/stt/vad.rs`) — el VAD de sherpa-onnx solo aporta
un booleano de "hay voz en este frame", el resto (pre-roll, histéresis,
silencio adaptativo) lo maneja Jarvis.

| Clave | Qué hace |
|---|---|
| `threshold` | Probabilidad de Silero (0-1) a partir de la que se considera que empezaste a hablar. Más bajo = detecta voz más floja, pero más sensible a ruido. No hay un `neg_threshold` separado (a diferencia del motor anterior): la robustez a micro-pausas la dan `silence_long_ms`/`silence_short_ms` de abajo. |
| `pre_roll_ms` | Audio previo que se antepone al buffer apenas se detecta voz, para no perder la primera sílaba (el VAD siempre tarda unos frames en confirmar que empezaste a hablar). |
| `min_speech_ms` | Si la voz detectada dura menos que esto, se descarta como blip (tos, golpe, clic) sin llegar a transcribir. |
| `silence_long_ms` / `silence_short_ms` / `long_utterance_ms` | El silencio necesario para cerrar la frase **cambia según cuánto llevás hablando**: por debajo de `long_utterance_ms` de locución exige `silence_long_ms` de silencio (para no cortar una pausa natural a mitad de una frase corta); superado ese umbral, exige solo `silence_short_ms` (para no hacerte esperar de más al final de frases largas). |
| `energy_floor_dbfs` | Piso de energía (dBFS): un segmento por debajo se descarta como ruido de fondo aunque el VAD lo haya marcado como voz. `null` (default) = se calibra solo al arrancar, escuchando el ambiente `calibration_secs` segundos. Fijalo a mano solo si la calibración automática te queda mal (por ejemplo, un ambiente que es ruidoso justo al arrancar Jarvis pero silencioso el resto del tiempo). |
| `calibration_secs` | Duración de esa calibración automática. |

**Si Jarvis te corta a mitad de frase**, subí `silence_long_ms` (o
`silence_short_ms` si el corte pasa en frases largas). **Si tarda en
reaccionar cuando dejás de hablar**, bajalos. **Si no te detecta al hablar
bajo**, bajá `threshold`; **si detecta ruido de fondo como si fuera voz**,
subilo (o subí `energy_floor_dbfs` a mano).

### `stt.filters`

Red de seguridad mínima contra alucinaciones. El binding de sherpa-onnx no
expone las métricas de confianza por segmento que sí tenía la integración
Python directa de Whisper (`no_speech_prob`/`avg_logprob`/
`compression_ratio`), así que lo único que queda acá es un filtro de
repetición degenerada — los descartes por duración/energía viven en
`stt.vad` de arriba.

| Clave | Qué hace |
|---|---|
| `max_word_repeat` | Descarta la transcripción si una palabra se repite esta cantidad de veces seguidas o más (alucinación típica en ruido/silencio). |

### `stt.clap` (detector de doble aplauso)

Corre siempre sobre cada frame, en el mismo hilo que el VAD
(`src/stt/clap.rs`), independientemente de si el [modo
bienvenida](README.md#modo-bienvenida-doble-aplauso) está activo:
`welcome.enabled` decide si Jarvis reacciona al evento, no esta sección.
Es una máquina de estados por frame: **onset** (la energía sube de golpe,
con timbre de banda ancha y sin pinta de voz sonora) → **decaimiento** (el
onset solo se confirma como aplauso si la energía vuelve a caer por debajo
del umbral que lo disparó dentro de `decay_ms`) → **doble** (dos aplausos
confirmados con un gap entre `double_min_gap_ms` y `double_max_gap_ms`
disparan el evento una sola vez, seguido de un refractario).

| Clave | Qué hace |
|---|---|
| `min_peak_dbfs` | dBFS mínimo del pico para considerar un posible aplauso. |
| `min_rise_db` | Subida mínima sobre el fondo (`bg_db`) para considerarlo onset. |
| `decay_ms` | Ventana en la que el onset debe volver a caer bajo su propio umbral para confirmarse como aplauso (y no como voz o música sostenida). |
| `reject_if_speech_active` | Rechaza el onset si el VAD detecta voz sostenida en ese frame. |
| `min_zcr` | Tasa mínima de cruces por cero, timbre de banda ancha, filtra golpes de teclado/trackpad. |
| `double_min_gap_ms` | Gap mínimo entre los dos aplausos (evita contar la reverb del mismo golpe como un segundo aplauso). |
| `double_max_gap_ms` | Gap máximo entre los dos aplausos. |
| `refractory_ms` | Tras confirmar el doble aplauso, ignora nuevos aplausos por este tiempo. |

### Dispositivo y modelo: `language`, `device`, `model_dir`, `vad_model_path`

`language`: código ISO 639-1 (`"es"` por defecto) que Whisper respeta
siempre — a diferencia de Parakeet-TDT (que se probó primero), acá sí hay
un parámetro de idioma explícito, así que no hace falta preocuparse por que
confunda español con otro idioma.

`device`: `auto` | `cpu` | `cuda` — provider de sherpa-onnx. `auto` usa
`cpu` hoy (no hay build con CUDA embebida todavía; correr en CPU con el
decoder int8 es razonablemente rápido y evita el problema clásico de
versiones de CUDA desalineadas entre driver y librería). `model_dir` y
`vad_model_path` apuntan a los archivos que descarga
`scripts/setup.ps1`/`setup.sh` — normalmente no hace falta tocarlos.

Otras claves: `input_device_index` (índice del micrófono dentro del
enumerado de `cpal`, `null` = el default del sistema), `cpu_threads`
(`null` = automático, ~núcleos físicos), y `stuck_state_timeout_secs` (si
alguno de los hilos del motor queda sin dar señales de vida más de esto, se
loguea un error — no hay reinicio automático de un hilo colgado, a
diferencia de un worker Python que Rust podía matar y reemplazar).

## `wake`

El "gate" de atención: decide si una frase transcrita merece respuesta o se
ignora.

| Clave | Qué hace |
|---|---|
| `enabled` | `false` = sin gate, Jarvis responde a absolutamente todo lo que transcribe (comportamiento previo a esta función). |
| `words` | Palabras que activan una respuesta. Se matchean normalizadas (sin tildes, minúsculas) y con tolerancia a distancia de edición 1 (así "yarvis" o "jarbis" también valen). |
| `attention_window_secs` | Tras cada respuesta, Jarvis sigue "atento" este tiempo sin que repitas su nombre, para conversaciones de ida y vuelta naturales. |
| `window_min_words` | Dentro de esa ventana, una frase sin el nombre necesita al menos esta cantidad de palabras para contar como pedido real, filtra alucinaciones de una sola palabra ("bip", "bien") que Whisper a veces inventa en silencio. `1` = sin este filtro. |
| `ignore_phrases` | Lista de frases-basura típicas que Whisper alucina con ruido/silencio (avisos de doblaje, "suscribete", etc.), se descartan siempre, comparación sin tildes. |
| `ambient_context` | Las frases ignoradas (dichas sin el nombre, fuera de ventana) no se pierden del todo: se guardan y se anteponen como contexto a tu siguiente pedido real, para que Jarvis "haya escuchado" la charla previa. |
| `ambient_context_max` / `ambient_context_ttl_secs` | Cuántas de esas frases ambientales se conservan como máximo, y por cuánto tiempo antes de descartarlas por viejas. |

## `barge_in`

Permite interrumpir a Jarvis mientras habla, dejó de ser half-duplex
estricto desde la Fase 3 del rediseño.

| Clave | Qué hace |
|---|---|
| `enabled` | `false` = comportamiento clásico: mic físicamente muteado mientras Jarvis responde, sin interrupciones posibles. |
| `mode` | **`wake_word`** (default): solo interrumpe si lo que dijiste mientras Jarvis hablaba contiene su nombre, fiable con parlantes, porque el eco de la propia voz de Jarvis rara vez incluye "Jarvis". **`any_voice`**: no exige el nombre, más natural — ver abajo cómo decide si de verdad hay que cortar. |
| `min_speech_ms` | Milisegundos de voz sostenida (medidos por el motor STT en modo "speaking") para confirmar que es una interrupción real y no un ruido puntual (una tos, un golpe). |
| `relevance_timeout_secs` | Solo aplica a `mode: any_voice`. Cuánto esperar la respuesta del LLM sobre si la interrupción tiene sentido (ver abajo) antes de rendirse. Si se agota o falla, Jarvis sigue hablando en vez de cortarse a ciegas. |

Cómo se dispara en cada modo: en `wake_word` se espera la transcripción
completa y se busca el nombre ahí (con la misma tolerancia a errores que
`wake.words`) — si no aparece, Jarvis sigue hablando sin cortarse.

En `any_voice`, apenas se supera `min_speech_ms` el motor avisa
(`speech_confirmed`) y Jarvis **pausa** (deja de decir frases nuevas, sin
cortar a mitad de palabra la que ya sonaba) mientras espera la
transcripción, que tarda cientos de milisegundos a un par de segundos más.
Si esa transcripción resulta ser eco propio (`echo_guard`) o el segmento se
descarta, Jarvis reanuda exactamente donde había quedado, sin perder nada.
Si no, antes de cortar de verdad se le pregunta al LLM configurado (una
consulta mínima y aparte de la conversación real) si lo que se escuchó
tiene sentido como algo dirigido a él o como continuación de lo que decía
— así una charla con otra persona cerca del micrófono no lo interrumpe.
Esto suma latencia real antes del corte (el tiempo de transcripción más el
de esa consulta al LLM, acotado por `relevance_timeout_secs`): quien
prefiera cero espera adicional y no le moleste tener que decir "Jarvis"
para interrumpirlo, puede quedarse con `mode: wake_word`.

### `barge_in.echo_guard`

Sin AEC (cancelación de eco) real, no hay wheels de las librerías
estándar (`webrtc-audio-processing`, `speexdsp`) para Windows —, este es un
filtro pragmático: compara por solapamiento de palabras la transcripción
capturada mientras Jarvis hablaba contra las frases que **efectivamente
dijo** hace poco, y si se parecen demasiado, la descarta como eco propio en
vez de tratarla como una interrupción real.

| Clave | Qué hace |
|---|---|
| `enabled` | `false` = nunca descarta por eco. Desaconsejado si usás parlantes sin auriculares. |
| `similarity_threshold` | Fracción de palabras de la transcripción que deben coincidir con algo que Jarvis dijo para descartarla como eco (0-1). Más alto = menos agresivo filtrando (podés colar más eco, pero también cortás menos interrupciones reales por error). |
| `vad_threshold_while_speaking` | Umbral de Silero para siquiera empezar a grabar mientras Jarvis habla, más alto que `stt.vad.threshold`, para que solo reaccione a voz sostenida y relativamente fuerte (vos hablando encima), no al murmullo de fondo del propio parlante. |
| `recent_tts_window_secs` | Cuánto tiempo se recuerdan las frases que Jarvis dijo, para comparar contra transcripciones que llegan justo después de que terminó de hablar (el eco puede llegar con un poco de latencia). |

**Con parlantes**, si Jarvis se autointerrumpe seguido, subí
`vad_threshold_while_speaking` y/o bajá `similarity_threshold` un poco (más
agresivo descartando eco), o directamente pasá a `mode: wake_word`.

## `llm`

Qué modelo genera las respuestas.

- `provider`: `ollama` | `anthropic` | `openai` | `deepseek` | `lmstudio`.
  `ollama` y `lmstudio` corren 100% local, sin API key ni costo por token;
  los demás son servicios en la nube.
- Cada proveedor tiene su propio sub-bloque (`ollama:`, `anthropic:`,
  etc.), pero **solo se lee el del `provider` activo**, no hace falta
  comentar los demás.
- `api_key_env` (en `anthropic`, `openai`, `deepseek`, `lmstudio`,
  `tts.elevenlabs`) **no es la API key en sí**, es el *nombre* de la
  variable de entorno que la contiene (ver `.env.example`). Así la key real
  nunca queda en este archivo, que sí se versiona en git.
- `ollama.think` / nota sobre modelos con razonamiento: `qwen3`, `qwen3.5`
  y `deepseek-r1` emiten tokens de "pensamiento" antes de la respuesta, que
  el TTS hablaría en voz alta si no se filtran, poné `think: false`. Con
  `qwen2.5` dejá `null` (el modelo rechaza la request si le mandás el
  campo). Con `model: "auto"` esto se ajusta solo.
- `ollama.auto_serve`: si `true` (default) y `base_url` apunta a esta
  máquina, Jarvis levanta `ollama serve` al arrancar cuando el servidor no
  responde. El proceso lanzado nace dentro del Job Object de Jarvis, así
  que se cierra automáticamente junto con él; si Ollama ya estaba corriendo
  (por ejemplo la app de bandeja), no se toca.
- `max_history_messages`: cuántos mensajes de la conversación actual se
  conservan (además de los 2 mensajes `system` fijos) antes de recortar los
  más viejos. Más alto = Jarvis recuerda más de la charla en curso, pero
  cada turno manda más tokens al proveedor (más lento y, en la nube, más
  caro).
- `request_timeout_secs`: timeout de la request HTTP al proveedor. Subilo
  si tu modelo local tarda en cargar en frío o compite por VRAM con el STT.
- `system_prompt`: la personalidad de Jarvis, en texto plano multilínea
  (bloque YAML `|`). Nunca debe pedirle markdown ni listas, el texto se
  convierte directo en audio.

## `tts`

Qué voz habla.

- `provider`: `piper` (offline, local, gratis) | `elevenlabs` | `cartesia`
  (nube, requieren API key y consumen créditos de la cuenta, pero suenan
  más naturales).
- `tts.piper`: `voice_path`/`config_path` apuntan al modelo `.onnx` de
  Piper y su `.json` de config (ver `voices/`, hay varias descargadas para
  comparar, comentadas). `use_cuda: true` sintetiza en GPU (más rápido,
  pero compite por VRAM con el STT si ese también usa CUDA).
  `length_scale` (ritmo: `<1` más rápido, `>1` más lento, `null` = original
  de la voz) y `noise_w_scale` (variación en la duración de los fonemas)
  son afinado fino por voz, retocalos después de escuchar la que elegiste.
- `tts.elevenlabs`: `voice_id` (desde tu cuenta, pestaña Voices → ⋮ → Copy
  Voice ID), `model_id`, `output_format` (formato PCM que exige la API —
  no tocar salvo necesidad concreta), `api_key_env`.
- `tts.cartesia`: `model_id` (`sonic-3.5` | `sonic-3` | `sonic-latest`),
  `voice_id` (desde tu cuenta de Cartesia), `language` (`null` =
  autodetectar), `output_format` (`container`: `raw` | `wav` | `mp3`;
  `encoding`: `pcm_s16le` | `pcm_f32le` | `pcm_mulaw` | `pcm_alaw`;
  `sample_rate`), `api_key_env`, `cartesia_version` (fecha de versión de la
  API, formato `AAAA-MM-DD`), `transport`: `websocket` (default, menor
  latencia, mantiene la conexión abierta entre frases) | `rest`.
- `synth_timeout_secs`: si sintetizar una frase tarda más que esto, se
  aborta el resto de la respuesta (mejor cortar que colgarse).

## `audio`

Reproducción de la voz de Jarvis. `output_device` (`null` = default del
sistema), `volume` (multiplicador, `1.0` = sin cambios),
`drain_timeout_secs` (límite de seguridad esperando a que termine de sonar
una respuesta, protege contra un dispositivo de audio caído o suspendido
por el SO).

## `pipeline`

Controla en qué unidades de texto se trocea la respuesta del LLM antes de
mandarla a sintetizar (streaming: cada frase se reproduce mientras el LLM
sigue generando el resto).

- `max_phrase_chars`: corta una frase para sintetizarla aunque el LLM no
  haya terminado la oración, evita esperar oraciones muy largas antes de
  que Jarvis empiece a hablar. Bajarlo reduce la latencia hasta la primera
  palabra, pero puede partir oraciones en cortes menos naturales.
- `min_phrase_chars`: fragmentos más cortos que esto se juntan con el
  siguiente antes de sintetizar, para no mandarle a Piper trozos sueltos
  tipo "Sí." (ineficiente y suena entrecortado).

## `agent`

La capa agéntica: herramientas que Jarvis puede ejecutar por vos. Ver
también la sección [Capacidades agénticas](README.md#capacidades-agénticas-herramientas)
del README para la tabla de herramientas disponibles.

Tres niveles de riesgo, clasificados de forma determinista **en Rust**
(nunca los decide el LLM): lectura libre, confirmación por voz ("¿confirma,
señor?"), o código de aceptación para acciones de riesgo extremo.

| Clave | Qué hace |
|---|---|
| `enabled` | `false` = chat puro sin herramientas. |
| `max_iterations` | Máximo de pasadas LLM↔herramientas por turno; al agotarse, Jarvis responde igual con lo que ya tenga en vez de seguir encadenando llamadas. |
| `tool_timeout_secs` | Si una herramienta tarda más que esto, se cancela y el LLM recibe un error como resultado (para que se disculpe o reintente). |
| `confirm_timeout_secs` | Tiempo para responder sí/no (o el código) antes de que la acción pendiente se cancele sola. |
| `confirm_mode` | `always` (default) = pide confirmación de voz para cada acción de riesgo `Confirm` | `free` = mano libre, las ejecuta directo sin preguntar. Las acciones de riesgo `Code` (extremo) **siempre** piden el código de aceptación, en cualquier modo — no se puede desactivar por config. |
| `max_tool_result_chars` | Truncado del resultado de cada herramienta antes de pasarlo al LLM, evita gastar contexto en salidas larguísimas (listados de procesos, páginas web enteras). |
| `filler_phrases` | Se dice una al azar mientras ejecuta una herramienta, solo si el modelo no dijo nada en su primera pasada, para no dejar un silencio muerto. |
| `disabled_tools` | Nombres de herramientas a excluir por completo, ej. `["run_powershell"]`. |
| `confirm_yes` / `confirm_no` | Palabras/frases que Rust interpreta como sí/no al confirmar una acción riesgosa. Esta interpretación **nunca la hace el LLM**: Rust busca si alguna de estas frases aparece dentro de tu respuesta (no hace falta que sea la respuesta completa, p.ej. "sí, ciérralo ya" matchea igual), para que el modelo no pueda auto-confirmarse. |
| `risk_code` | Código de aceptación para acciones de riesgo extremo (borrado recursivo, apagado, cambios de registro). Se verifica en Rust y nunca se le pasa al LLM. **Cambialo por uno propio.** |
| `high_risk_patterns` | Regex adicionales (se suman a los defaults del código) que elevan un comando de PowerShell a nivel "código". |

Sub-secciones:

- **`files`**: `search_roots` (carpetas donde busca `find_files`; default:
  el perfil del usuario), `max_results` (tope de resultados),
  `everything_cli` (ruta a `es.exe` de [Everything](https://www.voidtools.com/)
  para búsqueda instantánea sobre todo el disco; `null` = recorrido
  acotado con `walkdir` sobre `search_roots`).
- **`apps.aliases`**: mapea un alias hablado a un ejecutable real, para que
  `open_app`/`close_app` entiendan nombres coloquiales ("navegador" en vez
  de "brave.exe").
- **`web`**: `max_page_chars` (tope de caracteres de una página que se le
  pasan al LLM con `fetch_page`), `max_results` (tope de `web_search`).
- **`memory`**: `db_path` (SQLite de memoria persistente entre sesiones),
  `max_injected` (cuántas memorias recientes se inyectan en el prompt de
  cada turno, para que Jarvis las recuerde sin tener que llamar a `recall`
  explícitamente).
- **`translate`**: `default_target_lang`, idioma destino que usa `translate`
  cuando el pedido no especifica uno.
- **`reminders`**: `db_path` (SQLite de recordatorios), `poll_interval_secs`
  (cada cuánto se revisa si algún recordatorio venció), `max_active` (tope
  de recordatorios pendientes simultáneos).
- **`scripted_tools`**: `db_path` (SQLite de tools personalizadas creadas
  con `create_tool`), `max_tools` (tope de tools personalizadas
  simultáneas), `http_timeout_secs` (timeout de las recetas `http`), y la
  lista de hosts permitidos para esas recetas, una tool personalizada de
  tipo `http` solo puede pegarle a un host de esa lista, para que
  `create_tool` no se convierta en un proxy HTTP arbitrario.

## `welcome`

La escena de bienvenida disparada por doble aplauso (ver
[Modo bienvenida](README.md#modo-bienvenida-doble-aplauso) del README). El
detector de aplausos en sí vive en [`stt.clap`](#sttclap-detector-de-doble-aplauso)
y corre siempre, esta sección solo controla si Jarvis reacciona al evento
y con qué parámetros de escena.

| Clave | Qué hace |
|---|---|
| `enabled` | `false` = el doble aplauso se sigue detectando pero Jarvis lo ignora. |
| `music_path` | Ruta al mp3 de fondo. Es tuyo y nunca se versiona en git (ver `assets/music/.gitkeep`), si `enabled: true` y el archivo no existe, Jarvis falla el preflight de arranque con un mensaje accionable. |
| `greeting_phrase` | Frase que dice apenas arranca la escena, antes del resumen de recordatorios o noticias. |
| `music_volume` | Volumen de la música en reposo (nadie hablando). |
| `duck_volume` | Volumen reducido de la música mientras Jarvis o el usuario hablan (ducking). |
| `cooldown_secs` | Tras dispararse la escena, ignora nuevos dobles aplausos que la volverían a disparar durante este tiempo. |
| `news_when_no_reminders` | Si no hay recordatorios pendientes: `true` = cuenta las noticias del día (vía `web_search`); `false` = solo avisa que no hay pendientes. |

## `log_level`

`trace` | `debug` | `info` | `warn` | `error`. Con `debug` se ven además la
telemetría de VAD/transcripción (duración de la voz, RMS, confianza de
Whisper) y las decisiones de barge-in (por qué se descartó algo como eco,
por qué no se confirmó una interrupción), útil para calibrar los umbrales
de arriba con tus propios datos reales.
