# Onboarding de micrófono con VU meter en vivo

## Contexto

Jarvis hoy asume un micrófono "que funciona" (`stt.input_device_index: null` = default del sistema, `stt.vad.energy_floor_dbfs: null` = autocalibración silenciosa al arrancar). No hay forma de **ver** ni **elegir** el micrófono: solo existe un CLI de diagnóstico en Python (`stt_worker.py --list-devices` / `--calibrate`) que corre en terminal, fuera de cualquier flujo de usuario normal. El objetivo es que Jarvis se adapte a cualquier micrófono dándole al usuario una pantalla donde vea el nivel de su voz en tiempo real, elija el dispositivo correcto, y esto quede guardado — tanto la primera vez que instala Jarvis como cada vez que quiera recalibrar después (cambió de micrófono, se mudó de cuarto, etc.).

Ya existe una web de configuración (`web/config-ui/`, React+Vite sobre una API axum en `src/config_ui/`) con un patrón de secciones maduro. La idea es extender esa misma infraestructura en vez de construir algo paralelo. Importante: el proyecto ya tiene una feature llamada **"welcome"** (saludo por doble aplauso al llegar a casa, `config.yaml` sección `welcome:`) que no tiene nada que ver con onboarding — para evitar confusión, esta nueva feature usa el nombre **"onboarding"** en todos lados (config, rutas, componentes).

Decisiones ya tomadas con el usuario:
- **VU meter en tiempo real** durante la calibración (no un botón "probar 3 segundos", no solo autocalibración pasiva).
- Se usa **en dos lugares**: como wizard de primer arranque Y como sección normal reutilizable del panel de configuración, para recalibrar cuando se quiera.
- Nombre: **`onboarding`**.
- El plan incluye tests automatizados donde aplica y una fase explícita de revisión con agentes sobre el diff antes de dar el trabajo por terminado.

Este plan fue revisado por un agente contra el código real (`src/config_ui/mod.rs`, `src/stt/protocol.rs`, `src/stt/mod.rs`, `src/ipc/process.rs`, `workers/stt_worker.py`, `src/main.rs`) antes de finalizarse; los ajustes de esa revisión ya están incorporados en las fases de abajo (marcados donde corresponde).

## Decisión arquitectónica central

Hoy toda la captura de **entrada** de audio vive en Python (`workers/stt_worker.py`, PyAudio), hablada por IPC stdio desde Rust. El servidor `config_ui` (`src/config_ui/mod.rs`) corre **desacoplado** del `Orchestrator`: no comparte estado en memoria, relee/reescribe `config.yaml` en cada request (ver comentario de cabecera del archivo). En modo standalone (`jarvis --config-ui`, `src/main.rs:103-122`) el `Orchestrator` ni siquiera existe.

Por eso, la calibración corre en **un worker Python dedicado y liviano, separado del `SttWorker` de producción** — no reutiliza ni pausa el worker que ya está transcribiendo:
- No hace falta tender un puente de estado nuevo entre `config_ui` y un `Orchestrator` que en modo standalone no existe.
- El `SttWorker` real (si Jarvis está corriendo con voz activa) sigue escuchando exactamente igual mientras alguien calibra desde el navegador — el único recurso compartido es el dispositivo físico, no el proceso.
- El propio `_cli_calibrate` de `stt_worker.py` (líneas 56-100) ya prueba que calibrar no necesita cargar Whisper/Silero — es solo PyAudio + RMS→dBFS, spawnea barato y funciona igual en standalone e integrado.
- Se reutiliza toda la tubería IPC existente (`WorkerHandle::spawn`, framing NDJSON en `src/ipc/process.rs`) en vez de un subproceso ad-hoc con parseo manual.

**Riesgos aceptados, con su mitigación concreta (no bloqueantes, pero ya no "solo esperar que funcione"):**
- *Dos streams PyAudio concurrentes* al mismo dispositivo físico si Jarvis está corriendo mientras se calibra: en Windows con modo compartido (default de PyAudio/WASAPI) esto normalmente no choca, **pero** si choca debe fallar con un error claro, no un traceback silencioso — ver Fase 2.
- *Worker de calibración huérfano* si el proceso Rust muere de golpe en modo standalone: el Job Object de Windows (que mata automáticamente a los hijos) hoy **no cubre el modo `--config-ui`** porque se instala dentro de `run()`, que esa rama nunca alcanza (`src/main.rs:103-122` hace `return` antes de llegar a `run()` en la línea 141/147) — ver Fase 5 para la corrección.

## Fase 0 — Guardar este plan en el repo

Antes de tocar código, copiar este documento a `docs/planning/onboarding-mic-calibracion.md` dentro del repositorio, para que quede versionado junto al código que describe y visible para cualquiera que abra el proyecto — no solo en el directorio local de planes de la herramienta. Es el primer commit de la serie, antes de cualquier cambio funcional.

## Fase 1 — Dependencia

- `Cargo.toml:64`: `axum = "0.8"` → `axum = { version = "0.8", features = ["ws"] }`. Es el único cambio de dependencias (el frontend usa `WebSocket` nativo del navegador, sin librerías nuevas). Agregar una ruta WS junto a las REST existentes con `.route(path, get(handler))` no tiene fricción en axum 0.8.

## Fase 2 — Protocolo IPC (`src/stt/protocol.rs`, `workers/stt_worker.py`)

Todo aditivo sobre los enums existentes (`SttInMessage`/`SttOutMessage`, externally-tagged con `#[serde(tag = "type", rename_all = "snake_case")]`) — confirmado no-breaking: Rust ignora variantes desconocidas (`SttWorker::next_event`, `src/stt/mod.rs:263`) y Python nunca recibe variantes que no entiende porque son mensajes nuevos que solo el propio `CalibrationWorker` manda.

`src/stt/protocol.rs` — nuevas variantes:
```rust
// SttInMessage
Calibrate,                                    // alternativa a Init como primer mensaje
ListDevices,
StartCalibration { device_index: Option<u32> },
StopCalibration,
// (Shutdown ya existe, se reutiliza)

// SttOutMessage
CalibrationReady,
Devices { devices: Vec<AudioDeviceInfo> },
CalibrationStarted { device_index: u32, device_name: String, sample_rate: u32 },
// (Level{dbfs}, Error{..}, FatalError{..} ya existen, se reutilizan tal cual)
```
Nuevo struct `AudioDeviceInfo { index: u32, name: String, max_input_channels: u32, default_sample_rate: u32, is_default: bool }`.

**Tests (nuevo módulo `#[cfg(test)]` en `src/stt/protocol.rs`, o agregado al que ya exista en el archivo):** round-trip de serde para cada variante nueva — serializar `Calibrate`/`ListDevices`/`StartCalibration{device_index: Some(2)}`/`StopCalibration` y verificar el `"type"` esperado; deserializar JSON de ejemplo para `CalibrationReady`/`Devices`/`CalibrationStarted` y verificar los campos. Mismo estilo que cualquier test de serde ya presente en el crate.

`workers/stt_worker.py`:
- Nuevo módulo `workers/calibration_engine.py` con una clase pequeña (`_CalibrationEngine` o similar) que encapsula PyAudio + stream + `list_devices()`/`start(index)`/`stop()`. **Aclaración de alcance**: esto es código **nuevo, modelado sobre `_cli_calibrate`** (líneas 56-100 de `stt_worker.py`), *no* una extracción de `stt_engine.py::_Engine` — esa clase es mucho más grande (carga Silero+Whisper, coordina 3 hilos) y no tiene una pieza de captura reutilizable tal cual. Si es simple compartir el helper `_rms_dbfs` (hoy en `stt_engine.py:62-64`) entre ambos módulos, hacerlo; si no, duplicarlo es aceptable, no vale la pena forzar un acoplamiento nuevo entre los dos archivos por una función de 3 líneas.
- **Manejo de errores explícito**: envolver la apertura/reapertura del stream PyAudio (`pa.open(...)`) en `try/except`, igual que ya hace el resto del archivo (`_run_native`/`_run_realtimestt`, líneas 419-425 y 471-475) — si el dispositivo está ocupado, en modo exclusivo, o no soporta apertura concurrente con el worker de producción, emitir `{"type": "error", ...}` o `fatal_error` con mensaje claro en vez de dejar morir el proceso con un traceback no manejado (que del lado del navegador se vería solo como el WebSocket cerrándose sin explicación).
- **Mapeo de campos**: `pa.get_device_info_by_index(...)` devuelve claves camelCase (`maxInputChannels`, `defaultSampleRate`, ver `_cli_list_devices`, líneas 39-53); el struct `AudioDeviceInfo` usa snake_case — el mapeo explícito camelCase→snake_case al construir cada entrada de `list_devices()` es parte necesaria de esta fase, no un detalle implícito.
- `main()` (línea 538): hoy exige `init_msg["type"] == "init"`; aceptar también `"calibrate"` como primer mensaje válido, saltando a un nuevo `_run_calibration_mode(shutdown)` que **no** toca `cpu_threads`/`hardware_detect` (eso es solo del camino pesado de producción).
- `_run_calibration_mode`: manda `calibration_ready`, corre un hilo de control que reacciona a `list_devices`/`start_calibration`/`stop_calibration`/`shutdown`, y mientras hay un stream abierto emite `{"type": "level", "dbfs": ...}` cada ~100ms (mismo intervalo que `stt_engine.py:260-262`). `start_calibration` reabre el stream si ya había uno contra otro índice, para poder cambiar de dispositivo sin reiniciar el proceso.

**Tests (`workers/tests/test_calibration_engine.py`, mismo patrón pytest puro que `workers/tests/test_filters.py`, sin mockear PyAudio real):**
- `_rms_dbfs` (o el helper compartido/duplicado) con arrays sintéticos: silencio puro → dBFS muy negativo; tono conocido → valor esperado dentro de tolerancia.
- La función de mapeo camelCase→snake_case de `AudioDeviceInfo`: pasarle un dict con forma de lo que devuelve `get_device_info_by_index` (`{"name": ..., "maxInputChannels": 2, "defaultSampleRate": 44100.0}`) y verificar que el resultado tiene las claves snake_case correctas — sin abrir ningún stream real.

## Fase 3 — Wrapper Rust (`src/stt/calibration.rs`, nuevo)

Análogo chico a `SttWorker` (`src/stt/mod.rs`), sobre las mismas primitivas `crate::ipc::{WorkerHandle, WorkerFrame}`:
```rust
pub struct CalibrationWorker { handle: WorkerHandle, frames: mpsc::Receiver<WorkerFrame> }
impl CalibrationWorker {
    pub async fn spawn(workers: &WorkersConfig, runtime_dir: &Path) -> Result<Self, WorkerError>;
    pub async fn list_devices(&self) -> Result<(), WorkerError>;
    pub async fn start(&self, device_index: Option<u32>) -> Result<(), WorkerError>;
    pub async fn stop(&self) -> Result<(), WorkerError>;
    pub async fn next_event(&mut self) -> Option<CalibrationEvent>;
    pub async fn shutdown(&self);
}
```
`spawn()` envía `SttInMessage::Calibrate` y espera `CalibrationReady`. Para el timeout de arranque y de shutdown, seguir el patrón real ya usado por `SttWorker` (`src/stt/mod.rs:164, 217, 297`) en vez de un número mágico nuevo: reusar `workers.shutdown_timeout_secs` para `shutdown()`, y para el timeout de arranque (que hoy no tiene un campo dedicado porque `SttWorker` usa `stt_init_timeout_secs`, pensado para cargar modelos) usar una constante local documentada con un comentario que explique por qué es corta (~10s: no carga modelos) — no agregar un campo nuevo a `config.yaml` solo para esto salvo que en la práctica 10s resulte insuficiente en alguna máquina real durante la verificación.

Exportar desde `src/stt/mod.rs` con `mod calibration; pub use calibration::{CalibrationWorker, CalibrationEvent};`. El registro en el watchdog de Windows (`ipc::watchdog::register_worker_pid`, dentro de `WorkerHandle::spawn`) y la herencia del Job Object (cuando existe, ver Fase 5) aplican automáticamente sin código adicional, porque `CalibrationWorker` spawnea a través del mismo `WorkerHandle::spawn` — no hay `CREATE_BREAKAWAY_FROM_JOB` en ese código.

Red de seguridad ya existente que vale la pena confirmar en la verificación (no requiere código nuevo): si el handler WS de la Fase 4 nunca llegara a llamar `shutdown()` explícitamente (por ejemplo un panic en el handler), dropear el `CalibrationWorker` dropea su `WorkerHandle`, y el reaper interno de `WorkerHandle` (`src/ipc/process.rs`, tarea que espera en el `tokio::select!` a que `kill_tx` se cierre) mata igual al proceso hijo. Es un respaldo, no un sustituto de cerrar bien el WS.

## Fase 4 — Servidor Rust y WebSocket (`src/config_ui/`)

1. `src/config_ui/mod.rs`:
   - `section_crud!(get_onboarding, put_onboarding, onboarding, OnboardingConfig);` + ruta `.route("/api/config/onboarding", get(get_onboarding).put(put_onboarding))`, junto a las 8 secciones existentes (línea ~44-59) — mismo patrón mecánico, sin nada especial.
   - Ruta nueva: `.route("/api/onboarding/calibration/ws", get(calibration::calibration_ws))`.
   - Agregar `mod calibration;` en `mod.rs`. **`AppState` no necesita cambiar de visibilidad**: en Rust, un ítem privado es visible en su módulo y en todos los módulos descendientes, y `calibration` es un submódulo hijo de `config_ui` — ya ve `AppState` y su campo `config_path` sin tocar nada.
2. Nuevo `src/config_ui/calibration.rs`: handler `calibration_ws(ws: WebSocketUpgrade, State(state): State<AppState>)` que hace `ws.on_upgrade` y corre un loop `tokio::select!` entre `socket.recv()` (mensajes del navegador: `list_devices`/`start_calibration{device_index}`/`stop_calibration` → reenviados al `CalibrationWorker`) y `worker.next_event()` (eventos del worker → serializados y mandados por el WS). El túnel es NDJSON-sobre-WS con el mismo shape que `SttInMessage`/`SttOutMessage`, sin protocolo nuevo que inventar.
   - **Crítico**: al salir del loop por cualquier motivo (cierre normal, error, el usuario cierra la pestaña) llamar siempre `worker.shutdown().await`.
   - **Desconexión silenciosa**: un cierre limpio de pestaña manda un frame de cierre WS y se detecta rápido, pero una laptop que se suspende o un wifi que cae sin frame de cierre puede dejar la conexión "medio abierta" un rato antes de que el SO la reporte caída, y con ella el worker vivo de más. Agregar un timeout de inactividad simple (si no llega ningún mensaje del cliente — ni siquiera un ping — en, digamos, 60s, cerrar el socket y el worker) para acotar ese peor caso. No es estrictamente necesario para que el feature funcione (el reaper de `WorkerHandle` de la Fase 3 igual limpia el proceso si el `select!` eventualmente termina), pero acota cuánto tiempo puede quedar un stream de audio abierto sin que nadie lo esté mirando.
3. `src/main.rs`: no requiere cambios estructurales grandes salvo lo de la Fase 6.

## Fase 5 — Corregir la cobertura del Job Object en modo standalone

Hallazgo de la revisión: `JobObject::create_and_assign_current_process()` y `console_handler::install()` (`src/main.rs:166-184`) se instalan dentro de `run()`, pero la rama `if cli.config_ui { ...; return; }` (líneas 103-122) retorna **antes** de llegar a `run()` — así que hoy, si Jarvis corre como `--config-ui` puro y el proceso muere de golpe (Finalizar tarea, corte de energía) mientras hay una calibración activa, el Job Object no existe para arrastrar al worker Python huérfano.

Ajuste: mover la creación del Job Object (y, si tiene sentido, el console handler) a **antes** del branch `if cli.config_ui`, en `main()`, para que cubra ambos modos por igual. Es un cambio acotado (reordenar unas pocas líneas, moviendo el bloque `#[cfg(windows)] match ipc::job_object::JobObject::create_and_assign_current_process() { ... }` de dentro de `run()` a `main()` antes de la rama standalone) que además resuelve el problema para cualquier worker futuro, no solo para calibración.

## Fase 6 — Config schema (`src/config.rs`)

No hace falta tocar `stt.input_device_index` ni `stt.vad.energy_floor_dbfs` — el wizard/sección escriben ahí mismo vía el endpoint `PUT /api/config/stt` que **ya existe** (`section_crud!(get_stt, put_stt, stt, SttConfig)`, `src/config_ui/mod.rs:336`). Solo hace falta un flag de "onboarding completado", mismo patrón que `WelcomeConfig` (`src/config.rs:26` en `Config`, con su propio default):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OnboardingConfig {
    pub completed: bool,
}
```
Agregar `pub onboarding: OnboardingConfig` al struct `Config` y a su `impl Default` (`src/config.rs:15-58`). No requiere ninguna clave nueva en `config.yaml` (se completa con el default).

**Test:** verificar que `OnboardingConfig::default().completed == false` y que un `config.yaml` sin sección `onboarding:` carga igual (round-trip serde_yaml) — mismo patrón que cualquier test de default ya existente para otras subsecciones de `Config`, si lo hay; si no existe ningún test de este tipo hoy en `config.rs`, agregar un módulo `#[cfg(test)]` mínimo nuevo solo para este caso, sin intentar cubrir retroactivamente el resto del archivo.

## Fase 7 — Flujo de primer arranque

No existe hoy ningún concepto de "primer arranque bloqueante" — `startup_checks.rs` nunca exige `input_device_index`. El onboarding es valor agregado de UX, no un gate:

1. **La decisión de mostrar el wizard es del frontend**: al montar `App.tsx`, `GET /api/config/onboarding`; si `!completed`, renderizar `<OnboardingWizard>` en vez del shell normal. Al terminar (o "Saltar por ahora"), `PUT /api/config/onboarding {completed: true}` y pasa a la vista normal. El mismo build sirve standalone e integrado — la condición vive en los datos, no en el modo de arranque.
2. **Auto-abrir el navegador**: solo en modo `--config-ui` standalone (`src/main.rs:103-122`), tras un bind exitoso — ahí es una acción explícita del usuario ("quiero configurar"). Mecanismo confirmado viable: un `Option<oneshot::Sender<()>>` agregado a `config_ui::serve()`, disparado justo después del `TcpListener::bind` exitoso (línea 71-81); en el branch standalone de `main.rs` se crea el oneshot y al recibir la señal se ejecuta `cmd /C start "" <url>` en Windows (con `#[cfg(unix)]` de fallback a `open`/`xdg-open`) — no hace falta la crate `webbrowser`. El call site integrado (`main.rs:208`) pasa `None`. Alternativa igualmente válida y algo más simple: en vez de un canal + una rama extra de `tokio::select!`, pasar un callback síncrono `on_bound: Option<impl FnOnce()>` invocado justo después del bind — evaluar cuál conviene más al momento de escribir el código, ninguna es claramente superior.
3. **Modo integrado** (`web_ui.enabled: true`, Jarvis corriendo normal): no auto-abrir navegador (sería intrusivo desde un proceso de fondo). Si `!config.onboarding.completed` al arrancar, loguear una vez algo como `"Primer arranque: abrí http://127.0.0.1:<port> para elegir tu micrófono (opcional)"`.

## Fase 8 — Frontend (`web/config-ui/`, mismo proyecto Vite existente)

Un componente compartido, montado en dos lugares (wizard condicional + sección de sidebar) — evita duplicar un segundo proyecto Vite.

- `src/api/types.ts` / `src/api/client.ts` (extender): `OnboardingConfig`, `AudioDeviceInfo`, tipos de mensaje WS, `getOnboarding`/`putOnboarding` (mismo patrón get/put que el resto).
- `src/hooks/useCalibrationSocket.ts` (nuevo): abre el WS al montar, expone `{devices, dbfs, connected, error, selectDevice, start, stop}`, **cierra el socket en el cleanup del `useEffect`** (dispara el shutdown del lado Rust).
- `src/components/calibration/VuMeter.tsx` (nuevo): barra presentacional, prop `dbfs: number`, mapeo -60..0 dBFS → 0..100% con transición suave.
- `src/components/calibration/DeviceCalibrationPanel.tsx` (nuevo): dropdown de dispositivos (reusa `Select` de `components/form/Controls.tsx`) + `VuMeter` en vivo + ciclo start/stop al cambiar de dispositivo. No persiste nada él mismo — expone el `device_index` elegido vía callback, así es reutilizable en los dos lugares.
- `src/sections/OnboardingSection.tsx` (nuevo): mismo esqueleto que `WelcomeSection.tsx` (`PageHeader` + panel + `SaveBar`), usa `useConfigSection<SttConfig>({load: api.getStt, save: api.putStt})` para guardar `input_device_index`, embebe `DeviceCalibrationPanel`.
- `src/onboarding/OnboardingWizard.tsx` (nuevo): 2-3 pasos (intro → panel de calibración + "usar este micrófono" → confirmación) con "Saltar por ahora"; al terminar, `putOnboarding({completed: true})`.
- `src/config/sections.ts`: nueva entrada en `SECTIONS` (`{id: 'onboarding', label: 'Micrófono', icon: Mic, group: 'voice_in'}`) — mismo patrón que las 11 existentes.
- `src/App.tsx`: gate inicial (`GET /api/config/onboarding`; si `!completed`, `<OnboardingWizard>`; si no, shell normal) + `{activeId === 'onboarding' && <OnboardingSection />}` junto a las demás secciones.

**Sobre tests en el frontend:** hoy `web/config-ui/package.json` no tiene ningún framework de test configurado (sin Vitest/Jest). No se introduce uno de forma implícita en este plan — el mapeo dBFS→porcentaje de `VuMeter` y el flujo del wizard quedan cubiertos por la verificación manual de la Fase 10. Si más adelante se quiere cobertura automatizada del frontend, es una decisión aparte (elegir y configurar un test runner) que vale la pena tratar como su propio pedido, no colarla acá.

## Fase 9 — Documentación

- `README.md`: agregar entrada `**onboarding**` junto a `**welcome**` en la lista de secciones de `config.yaml`; mencionar el wizard y el auto-open del navegador en la sección de la página de configuración local.
- `docs/CONFIGURACION.md`: en la sección que documenta `energy_floor_dbfs`/`input_device_index`, agregar un párrafo remitiendo a la nueva UI con VU meter como forma recomendada de elegir micrófono, dejando `--list-devices`/`--calibrate` documentados como alternativa sin navegador.

## Fase 10 — Verificación end-to-end (manual)

1. `npm run build` en `web/config-ui/`, `cargo run --release -- --config-ui`: confirmar que el navegador se abre solo, la barra reacciona en tiempo real al hablar (~100ms de latencia), y cambiar de dispositivo en el dropdown sigue funcionando sin recargar la página.
2. Completar el wizard y verificar en `config.yaml` que `stt.input_device_index` y `onboarding.completed: true` quedaron guardados correctamente (comparar el índice contra `python workers/stt_worker.py --list-devices`).
3. Con Jarvis corriendo normal (`cargo run --release`), abrir la web en paralelo, calibrar/cambiar de micrófono desde ahí mientras se le sigue hablando a Jarvis — confirmar que STT normal no se corta ni crashea (valida los dos streams PyAudio concurrentes) y que, si el dispositivo elegido para calibrar realmente choca con el worker de producción, aparece un error legible en la UI en vez de que el WS se cierre sin explicación (valida el manejo de excepción de la Fase 2).
4. Repetir el paso 1 en modo standalone puro (sin Jarvis corriendo en otro lado) para confirmar que no depende del `Orchestrator`. Adicionalmente: con una calibración activa en standalone, finalizar el proceso `jarvis.exe` a la fuerza desde el Administrador de tareas y confirmar (`Get-Process python` o similar) que el worker de calibración también murió — valida la corrección de la Fase 5.
5. Cerrar la pestaña del navegador a mitad de una calibración (cierre normal) y confirmar que el proceso Python de calibración termina solo, sin quedar huérfano.
6. Con `onboarding.completed: true`, entrar a la sección "Micrófono" del sidebar y confirmar que el mismo panel funciona ahí (reutilización real, no una copia).

## Fase 11 — Revisión con agentes sobre el diff (antes de dar el trabajo por terminado)

Una vez implementadas las fases 1-9 (o en tandas razonables: primero backend Rust+Python, después frontend), correr una revisión de código con un agente sobre el diff real antes de cerrar el trabajo — no alcanza con que compile y pase la verificación manual feliz. Puntos a pedirle explícitamente al agente revisor que mire (son los mismos riesgos identificados al diseñar este plan, ahora verificados contra código escrito de verdad en vez de contra el plan):

- ¿El `try/except` alrededor de `pa.open(...)` en `calibration_engine.py` realmente cubre el caso de dispositivo ocupado/exclusivo, y el mensaje de error que llega al frontend es entendible por un usuario no técnico?
- ¿`worker.shutdown().await` se llama en *todas* las rutas de salida del loop del handler WS (`calibration.rs`), incluyendo error de deserialización de un mensaje del cliente y panic dentro del `select!`?
- ¿El reordenamiento del Job Object (Fase 5) no rompió nada del flujo normal de `run()` (orden de logging, `console_shutdown_rx`, etc.)?
- ¿Las nuevas variantes de `SttInMessage`/`SttOutMessage` siguen siendo genuinamente aditivas (no cambiaron el orden ni el nombre de ninguna variante existente)?
- ¿El timeout de arranque del `CalibrationWorker` y el reuso de `workers.shutdown_timeout_secs` están donde el plan dice, sin números mágicos sin comentario?
- Consistencia general del frontend nuevo con el resto de `web/config-ui` (mismos componentes de `form/Controls.tsx`, mismo manejo de toasts/errores que las demás secciones).

Usar el skill `/code-review` del repo (o un agente de propósito general con este mismo checklist como prompt) apuntando al diff acumulado de la feature, no a un archivo suelto.
