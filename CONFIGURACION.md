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

Cómo Rust arranca los dos procesos Python (`stt_worker.py`, `tts_worker.py`)
y qué tan tolerante es a que se caigan.

| Clave | Qué hace |
|---|---|
| `python_executable` | Ruta al intérprete del venv de `workers/`. |
| `stt_script` / `tts_script` | Rutas a los scripts que se spawnean. |
| `stt_init_timeout_secs` | Tiempo máximo esperando el mensaje `ready` del STT. Generoso (60s) porque cargar/calibrar Whisper la primera vez puede tardar, sobre todo en CPU. |
| `tts_init_timeout_secs` | Ídem para Piper, mucho más rápido de cargar. |
| `shutdown_timeout_secs` | Cuánto se espera a que un worker cierre solo antes de matarlo. |
| `restart_on_crash` | Si el worker de STT muere o queda colgado (su propio watchdog lo detecta), se reemplaza por uno nuevo en caliente en vez de tumbar Jarvis entero. |
| `max_restarts` | Tope de reinicios por corrida. Al agotarse, Jarvis termina, evita un loop de reinicios infinito si el problema es persistente (driver de audio roto, modelo corrupto, etc.). |

## `stt`

Todo lo relacionado con transcribir lo que decís. Desde la Fase 1 del
rediseño de detección de voz, hay **dos motores** intercambiables con
`stt.engine`:

- **`native`** (default): motor propio, PyAudio captura audio, Silero VAD
  detecta habla frame por frame, faster-whisper transcribe directo. Sin las
  limitaciones de RealtimeSTT (ver abajo). Es el único que soporta
  `barge_in` (interrumpir a Jarvis hablando).
- **`realtimestt`**: envuelve la librería [RealtimeSTT](https://github.com/KoljaB/RealtimeSTT).
  Queda como respaldo por si el motor nativo da problemas en tu hardware —
  cambiar a este valor no requiere recompilar, es solo esta línea. Tiene un
  bug conocido (`min_gap_between_recordings` puede dejar el recorder sordo)
  parcheado por un watchdog, y no puede dar eventos de voz continuos
  mientras Jarvis habla, así que con este motor **`barge_in` no tiene
  efecto** sin importar cómo lo configures, el mic simplemente se mutea
  físicamente mientras Jarvis responde, como antes de la Fase 1.

### `stt.vad` (solo `engine: native`)

Controla cómo se detecta el inicio y el fin de una frase.

| Clave | Qué hace |
|---|---|
| `threshold` | Probabilidad de Silero (0-1) a partir de la que se considera que empezaste a hablar. Más bajo = detecta voz más floja, pero más sensible a ruido. |
| `neg_threshold` | Probabilidad por debajo de la que se considera que dejaste de hablar. Deliberadamente **menor** que `threshold` (histéresis): una vez que empezó a grabar, hace falta una caída más marcada para cortar, así que las micro-pausas de respiración no parten la frase en dos. |
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

### `stt.filters` (solo `engine: native`)

Filtros anti-alucinación sobre la salida de Whisper, Whisper a veces
"transcribe" algo con silencio o ruido puro. Cada segmento trae sus propias
métricas de confianza; si alguna se pasa del umbral, la transcripción se
descarta antes de llegar a Jarvis (queda un evento `discarded` en los logs
con la razón, visible con `log_level: debug`).

| Clave | Qué hace |
|---|---|
| `max_no_speech_prob` | Se descarta si la probabilidad de "esto no es habla" que calcula Whisper supera este valor. |
| `min_avg_logprob` | Se descarta si la confianza promedio del decoder cae por debajo (más negativo = menos confianza). |
| `max_compression_ratio` | Se descarta si el texto es demasiado repetitivo (síntoma clásico de alucinación en bucle). |

Si notás transcripciones inventadas que se cuelan, hacé estos umbrales más
estrictos (`max_no_speech_prob` más bajo, `min_avg_logprob` más alto,
`max_compression_ratio` más bajo). Si al revés Jarvis descarta cosas que sí
dijiste, aflojalos.

### `stt.clap` (detector de doble aplauso)

Corre siempre sobre cada frame en el motor nativo, en el mismo hilo que el
VAD (`workers/clap_detector.py`), independientemente de si el [modo
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
| `max_vad_prob` | Rechaza el onset si Silero cree que es voz sonora por encima de este umbral. |
| `min_zcr` | Tasa mínima de cruces por cero, timbre de banda ancha, filtra golpes de teclado/trackpad. |
| `double_min_gap_ms` | Gap mínimo entre los dos aplausos (evita contar la reverb del mismo golpe como un segundo aplauso). |
| `double_max_gap_ms` | Gap máximo entre los dos aplausos. |
| `refractory_ms` | Tras confirmar el doble aplauso, ignora nuevos aplausos por este tiempo. |

Para calibrar estos umbrales en vivo con tu micrófono: `python
workers/stt_worker.py --test-clap` (ver [`workers/README.md`](workers/README.md)).

### Detección de hardware: `device`, `whisper_model`, `compute_type`

Aplican a **ambos motores**. Con los tres en `auto` (default):

- **Con GPU CUDA** (detectada vía `ctranslate2`, no vía `torch`, el venv
  trae la build CPU-only de torch, más liviana): se elige el modelo por
  tiers de VRAM sin medir nada (con 4GB, por ejemplo, `small` en
  `float16`, más preciso y más rápido que `base` en CPU).
- **Sin GPU** (o si forzás `device: cpu`): el primer arranque **calibra de
  verdad**, transcribe un audio de referencia con distintos modelos y mide
  el RTF (tiempo de transcripción / duración del audio) real de tu máquina,
  y elige modelo y `beam_size` según qué tan holgada vaya. El resultado
  queda cacheado en `workers/.cache/stt_profile.json`: los arranques
  siguientes son instantáneos, y solo se vuelve a medir si cambia el
  hardware o ponés `recalibrate: true`.

`compute_type: auto` usa `int8_float16` en GPU (funciona en cualquier CUDA,
mientras que `float16` puro falla sin tensor cores) e `int8` en CPU.

Otras claves de esta sección: `language` (código ISO para Whisper),
`input_device_index` (índice de PyAudio del micrófono, `null` = el
default del sistema; usá `python workers/stt_worker.py --list-devices` para
ver los índices reales, o `--calibrate` para un vúmetro en vivo),
`beam_size`/`cpu_threads` (override manual de lo que decidiría la
calibración), `initial_prompt` (contexto que se le da a Whisper para que
transcriba mejor "Jarvis"), `recalibrate`, y `stuck_state_timeout_secs`
(si el worker queda trabado grabando/transcribiendo más de esto, se
reinicia solo, aplica a los dos motores).

### Claves solo de `engine: realtimestt`

`silero_sensitivity`, `webrtc_sensitivity`, `post_speech_silence_duration`,
`min_length_of_recording`, `min_gap_between_recordings`,
`silero_deactivity_detection` son parámetros propios de RealtimeSTT, sin
efecto alguno con `engine: native` (que usa `stt.vad`/`stt.filters` en su
lugar). Se conservan para cuando necesites el camino de respaldo.

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
estricto desde la Fase 3 del rediseño. **Solo funciona con `stt.engine:
native`**; con `realtimestt` este bloque entero no tiene efecto (ver
arriba).

| Clave | Qué hace |
|---|---|
| `enabled` | `false` = comportamiento clásico: mic físicamente muteado mientras Jarvis responde, sin interrupciones posibles. |
| `mode` | **`wake_word`** (default): solo interrumpe si lo que dijiste mientras Jarvis hablaba contiene su nombre, fiable con parlantes, porque el eco de la propia voz de Jarvis rara vez incluye "Jarvis". **`any_voice`**: interrumpe con cualquier voz sostenida, sin exigir el nombre, más natural, pero **recomendado solo con auriculares**: con parlantes sin cancelación de eco, el propio audio de Jarvis puede autointerrumpirlo (mitigado, no eliminado, por `echo_guard`). |
| `min_speech_ms` | Milisegundos de voz sostenida (medidos por el motor STT en modo "speaking") para confirmar que es una interrupción real y no un ruido puntual (una tos, un golpe). |

Cómo se dispara en cada modo: en `any_voice`, apenas se supera
`min_speech_ms` el motor avisa (`speech_confirmed`) y Rust cancela **de
inmediato**, sin esperar a que termine de transcribirse la frase, eso
tarda cientos de milisegundos más. En `wake_word` se espera la
transcripción completa y se busca el nombre ahí (con la misma tolerancia a
errores que `wake.words`).

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
- `ollama.think` / nota sobre modelos con razonamiento: `qwen3` y
  `deepseek-r1` emiten tokens de "pensamiento" antes de la respuesta, que
  el TTS hablaría en voz alta si no se filtran, poné `think: false`. Con
  `qwen2.5` dejá `null` (el modelo rechaza la request si le mandás el
  campo).
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
