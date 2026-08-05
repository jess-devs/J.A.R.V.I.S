# Personalización de voz: verificación de hablante (Parte A) + wake word acústico exploratorio (Parte B)

## Contexto

El usuario investigó cómo Apple implementa "Hey Siri" (detección de wake word por DNN sobre audio crudo + reconocimiento de hablante por speaker embeddings) y quiere algo equivalente para Jarvis, motivado por "no todos hablamos igual" — quiere que Jarvis se comporte mejor y de forma más segura adaptándose a su voz particular, con los ajustes reflejados en `config.yaml`.

Investigación previa (esta sesión) reveló algo clave: **Jarvis ya tiene una implementación de verificación de hablante, apagada y en "modo sombra"**. Calcula similitud coseno de cada frase contra una voz enrolada (ECAPA-TDNN vía `speechbrain`), pero hoy ese dato solo se loguea (`tracing::info!`) — nunca gatea nada, y el enrollment solo existe como comando CLI de una sola pasada (`python workers/stt_worker.py --enroll-voice`, 5s bloqueantes, sin reintentos ni feedback). El propio comentario de diseño explica por qué: es "una etapa de recolección de datos reales de umbral antes de aplicarlo a algo relevante a seguridad".

Esto es exactamente la pieza de "reconocimiento del hablante" que describe Apple — ya existe, solo falta exponerla. El usuario confirmó que quiere **ambas partes**: (a) un flujo de enrollment en la web, y (b) activar el gating real de confirmaciones de riesgo por voz. Se diseña de forma conservadora (flag nuevo y separado del modo sombra actual, con umbral configurable y sin lockouts silenciosos) para respetar la intención original de "no aplicar esto a seguridad sin datos".

Sobre el wake word acústico (DNN tipo "Hey Siri" corriendo en audio crudo, antes de transcribir): investigado a fondo por pedido explícito del usuario pese a la recomendación inicial en contra. Hallazgo relevante: **openWakeWord ya tiene un modelo pre-entrenado "hey_jarvis"** (Apache 2.0 el código, CC BY-NC-SA 4.0 el modelo — sin problema para uso personal), liviano (~0.42MB, ~100K parámetros, corre 15-20 instancias simultáneas en un solo núcleo de Raspberry Pi 3), compatible con el audio 16kHz mono que Jarvis ya captura. Pero **no resuelve "no todos hablamos igual"**: es un modelo genérico, no personalizado a una voz — su valor real sería ahorrar CPU (evitar transcribir con Whisper segmentos de voz que no van dirigidos a Jarvis) y no cambia con quién habla. Además cambiaría la UX de activación (frase fija "hey jarvis" vs. el "jarvis" flexible actual, en cualquier posición, con tolerancia a errores). Por eso queda como **Parte B, exploratoria, no se implementa en este trabajo** — la Parte A es la respuesta directa al pedido del usuario.

Plan revisado contra el código real (`src/orchestrator.rs`, `src/stt/mod.rs`, `src/config.rs`, `workers/speaker_verification.py`, `workers/stt_engine.py`, `src/config_ui/calibration.rs`) antes de finalizarse — los nombres de campos/funciones citados abajo existen tal cual en el repo hoy.

---

## Parte A — Enrollment web + gating conservador

### Decisión arquitectónica

**Reusar el worker de calibración ya construido** (`CalibrationWorker`/`calibration_engine.py`/`/api/onboarding/calibration/ws`) en vez de un cuarto worker Python. Enrolar una voz necesita la misma base que calibrar un micrófono (resolver dispositivo, abrir stream PyAudio, leer frames), ya probada. Lo único nuevo es "grabar N segundos con countdown" y pasarle el audio a `SpeakerVerifier.enroll()` — se engancha como comandos nuevos del mismo modo `calibrate`, mismo WebSocket, sin ruta nueva.

**Orden de fases**: el enrollment (A.1-A.2) va antes que el gating (A.3), aunque el pedido explícito fue "el gating real" — sin una forma decente de enrolar, el gating no tiene con qué trabajar (el único camino hoy, el CLI bloqueante de 5s, es demasiado frágil para depender de él en algo de seguridad).

El punto de mayor riesgo de diseño es la **correlación asíncrona** entre `Transcript` (llega primero) y `SpeakerSimilarity` (llega después, calculada en un hilo Python aparte) — ver A.3.

### A.1 — Protocolo IPC de enrollment

`src/stt/protocol.rs` — variantes nuevas, aditivas (mismo criterio no-breaking que las de calibración):
```rust
// SttInMessage (válidas solo después de Calibrate/CalibrationReady)
StartEnroll { device_index: Option<u32>, seconds: f32 },
CancelEnroll,

// SttOutMessage
EnrollStarted { device_index: u32, device_name: String, total_ms: u32 },
EnrollProgress { elapsed_ms: u32, total_ms: u32 },   // cada ~200ms
EnrollProcessing,                                     // grabación lista, calculando embedding
EnrollComplete { embedding_path: String },
// Fallos reusan SttOutMessage::Error{code, message, recoverable} — mismo
// criterio que start_calibration, no hace falta una variante EnrollFailed.
```

`workers/calibration_engine.py` — extender `CalibrationEngine`:
- `start_enroll(seconds, on_progress, cancel_event)`: graba en un **hilo aparte** (nombrado, ej. `"enroll-record"`) — crítico porque el hilo de control de `_run_calibration_mode` está bloqueado en `ipc.read_line()`; si grabar+calcular el embedding (hasta ~14s la primera carga del modelo, más si descarga los ~80MB de pesos) corriera ahí, un `cancel_enroll`/`shutdown` durante ese lapso quedaría sin atender.
- Al terminar la grabación, importa `speaker_verification.SpeakerVerifier` perezosamente (igual que `_cli_enroll_voice` hoy) y llama `.enroll(audio)`. Emite `enroll_processing` apenas arranca esa etapa (puede tardar y sin este evento la UI se ve colgada).
- `cancel_enroll()`: setea el `cancel_event`, el hilo de grabación corta en la próxima iteración (frames de ~32ms, no hace falta interrumpir un read a mitad de camino).

`workers/stt_worker.py::_run_calibration_mode` (líneas ~588-626): dos ramas nuevas en el `if/elif`, mismo estilo try/except-con-`error{recoverable:true}` que `start_calibration`/`list_devices`.

`src/stt/calibration.rs` — nuevos métodos en `CalibrationWorker` (mismo patrón que `start`/`stop`): `start_enroll(device_index, seconds)`, `cancel_enroll()`; nuevas variantes de `CalibrationEvent` mapeadas en `next_event()` igual que `Devices`/`Level`.

`src/config_ui/calibration.rs` — extender el `ClientMessage` del WS ya existente (no ruta nueva): `StartEnroll { device_index, seconds }`, `CancelEnroll`, reenviados al `CalibrationWorker` en el mismo `match`. El resto del handler (shutdown garantizado, timeout de inactividad de 60s) no cambia — el frontend (A.4) manda `list_devices` (ya soportado, inocuo) cada ~20s como keepalive mientras el enrollment esté en curso, para no tocar `CLIENT_IDLE_TIMEOUT`.

**Tests**: round-trip serde en `src/stt/protocol.rs` para las 6 variantes nuevas (mismo estilo que los tests ya existentes ahí), y tests del mapeo de progreso en `workers/tests/test_calibration_engine.py`.

### A.2 — `SpeakerVerificationConfig` extendido + endpoint de estado

`src/config.rs` — `enabled` pasa a significar únicamente "modo sombra: calcular y loguear". Campos nuevos, todos con default conservador (gating apagado):
```rust
pub struct SpeakerVerificationConfig {
    pub enabled: bool,                    // ya existe: modo sombra
    pub gate_confirmations: bool,         // NUEVO, default false
    pub similarity_threshold: f32,        // NUEVO, default 0.6 — punto de partida, no
                                           // una recomendación; ajustar mirando los propios
                                           // logs de modo sombra
    pub gate_wait_ms: u32,                // NUEVO, default 600 — ver A.3
    pub on_uncertain: OnUncertainPolicy,  // NUEVO, default Deny
}
pub enum OnUncertainPolicy { Allow, Deny }
```
Pasa de `#[derive(Default)]` a `impl Default` manual por los valores no-cero.

`src/stt/mod.rs::SttWorker::spawn` (línea ~135, donde hoy construye `SpeakerVerificationInit { enabled: speaker_verification.enabled }`): cambia a `enabled: speaker_verification.enabled || speaker_verification.gate_confirmations` — así activar `gate_confirmations` sin `enabled` igual dispara el cálculo de similitud en Python (si no, el gating no tendría señal). Documentar este efecto secundario en el doc-comment.

Endpoint de estado nuevo, mismo patrón que `status_welcome`/`status_llm` (`src/config_ui/mod.rs`):
```rust
.route("/api/status/speaker_verification", get(status_speaker_verification))
```
Chequea `config.runtime_dir().join("speaker_embedding.json")` (`.is_file()` + mtime opcional) — sin abrir el JSON ni spawnear nada. Le permite al frontend mostrar "ya hay una voz enrolada" o bloquear el toggle de `gate_confirmations` si no hay ninguna.

### A.3 — Correlación Transcript↔SpeakerSimilarity y gating (núcleo del plan)

**Por qué no alcanza un chequeo pasivo**: la similitud de una frase llega *después* de esa misma frase (Python la calcula en un hilo aparte). Cuando Rust recibe el `Transcript` con el "sí" y necesita decidir, la `SpeakerSimilarity` correspondiente casi seguro todavía no salió. Hace falta espera activa y acotada.

**Paso 0**: dejar de descartar `text_preview` en `src/stt/mod.rs` (hoy `Ok(SttOutMessage::SpeakerSimilarity { similarity, .. })` lo tira) — pasa a `SttEvent::SpeakerSimilarity { similarity, text_preview }`.

**Paso 1 — diseño**: nuevo helper en `Orchestrator`:
```rust
enum SpeakerCheck { Verified, Rejected, Inconclusive }
async fn verify_speaker(&mut self, said_text: &str) -> SpeakerCheck
```
Corre su propio loop acotado sobre `self.stt.next_event()` con `tokio::time::timeout(gate_wait_ms, ...)` — mismo patrón "loop + timeout" que ya usan `SttWorker::spawn`/`CalibrationWorker::spawn` al arrancar, aplicado acá en caliente:
- `SpeakerSimilarity{similarity, text_preview}` con `text_preview` matcheando `said_text` normalizado → resuelve ya: `Verified` si `similarity >= threshold`, si no `Rejected`. El contenido de la frase es la clave de correlación (no hace falta timestamp): Python solo empieza a calcular similitud después de mandar ese mismo transcript, así que no puede llegar una similitud de "sí" antes de que Rust haya recibido ese "sí".
- `Level{dbfs}`: se reenvía a `self.mic_level_tx.send_replace(dbfs)` (campo ya existente, `src/orchestrator.rs:113,451`) para no congelar el medidor durante la espera.
- `Transcript{text,..}` (si el usuario sigue hablando en esta ventana de <1s): se guarda en `self.deferred_transcript: Option<String>` y se reprocesa con `self.on_transcript(text)` apenas resuelve `verify_speaker` — no se pierde.
- `WorkerDied`: corta la espera, `Inconclusive`.
- Resto de eventos: se ignoran.
- Deadline vencido: `Inconclusive`.

**Paso 2 — dónde se llama**: en `handle_confirmation` (`src/orchestrator.rs:883-952`), solo si `gate_confirmations` es `true`, solo en la rama de aprobación (`ConfirmDecision::Yes`/`CodeDecision::Correct` — nunca en cancelación, no vale la pena la latencia ahí):
```rust
ConfirmDecision::Yes => {
    if gate_confirmations {
        match self.verify_speaker(&text).await {
            Verified => self.approve_pending(pending).await,
            Rejected => self.deny_by_speaker(pending, "no coincide con la voz registrada").await,
            Inconclusive => match on_uncertain {
                Allow => self.approve_pending(pending).await, // + tracing::warn!
                Deny => self.deny_by_speaker(pending, "no se pudo verificar a tiempo").await,
            },
        }
    } else {
        self.approve_pending(pending).await; // sin cambios
    }
}
```
`deny_by_speaker`: variante chica de `cancel_and_acknowledge` que **siempre habla una razón concreta y deja la puerta abierta a reintentar** — nunca un cancel silencioso. Ej.: *"Perdón, señor, esa voz no coincide con la registrada. Cancelé la acción por seguridad — pedímelo de nuevo cuando quieras."* Esto cumple lo acordado: nunca un lockout duro sin explicación ni salida.

**Caso sin enrollment**: `stt_engine.py` ya deja `self.speaker_verifier = None` si no hay `enrolled` — Python nunca manda `speaker_similarity`, `verify_speaker` siempre da `Inconclusive` por timeout, sin caso especial en Rust. Sí loguear un `tracing::warn!` una vez al arrancar si `gate_confirmations` está prendido pero no hay embedding (chequeo de sanidad en `SttWorker::spawn`).

**Alcance**: aplica a ambas ramas de `handle_confirmation` (con y sin `risk_code`). No aplica con `confirm_mode: free` (no hay pregunta que gatear) — documentar esta limitación: la seguridad de esto depende de que Jarvis efectivamente pregunte.

**Tests**: extraer la lógica de matching de `text_preview` (normalización + comparación) a una función pura testeable en aislamiento, mismo criterio que `confirm.rs::interpret` (puro, sin IPC). `orchestrator.rs` no tiene arnés de test de integración hoy — no forzarlo.

### A.4 — Frontend: enrollment en la sección "Micrófono"

**Dónde**: dentro de `OnboardingSection.tsx` (label "Micrófono" en el sidebar), un `FieldGroup` nuevo "Mi voz" debajo del panel de calibración de dispositivo ya existente — no una sección aparte.

**Sin selector de dispositivo propio**: graba siempre contra `cfg.input_device_index` (mostrado como texto de solo lectura, "Se va a grabar desde: ..."). Evita ambigüedad y evita compartir estado con `DeviceCalibrationPanel`.

**WebSocket propio**: el panel de enrollment abre su propia conexión (mismo endpoint, mismo patrón "bajo demanda con botón explícito" ya aplicado en `useCalibrationSocket`) en vez de compartir la del panel de calibración de arriba — evita refactorizar un componente ya en producción por un ahorro marginal. Dos streams PyAudio concurrentes contra el mismo dispositivo ya es un riesgo aceptado y mitigado en la feature de calibración; acá es aún menor (acción puntual de segundos).

**Nuevos archivos**:
- `web/config-ui/src/hooks/useVoiceEnrollment.ts`: WS bajo demanda (solo al expandir el panel, no al entrar a la página), expone `{connected, status: 'idle'|'recording'|'processing'|'done'|'error', progress, dbfs, error, start(), cancel()}`. Manda `list_devices` cada ~20s como keepalive mientras `status !== 'idle'`. Cierra el socket en cleanup, igual que `useCalibrationSocket`.
- `web/config-ui/src/components/enrollment/EnrollmentPanel.tsx`: botón "Grabar mi voz (5s)" → countdown visual (reusa `VuMeter` para el nivel en vivo) → "Procesando…" (con aviso de que la primera vez puede tardar por la descarga del modelo) → éxito con "Volver a grabar" o error con reintento. Consulta `GET /api/status/speaker_verification` al montar.
- Tipos WS nuevos en `api/types.ts`/`client.ts` + `getSpeakerVerificationStatus()`.

`web/config-ui/src/sections/AgentSection.tsx` (`FieldGroup` "Auditoría y verificación de hablante", líneas ~245-258): agregar toggle `gate_confirmations` (**deshabilitado con hint si no hay voz enrolada**, según el status endpoint), `NumberInput` para `similarity_threshold` (hint: "punto de partida, ajustalo mirando tus logs de modo sombra") y `gate_wait_ms`, `Select` para `on_uncertain` con hint explicando la diferencia UX entre Allow/Deny.

### A.5 — Documentación

- [`docs/CONFIGURACION.md`](../CONFIGURACION.md): reescribir el párrafo de
  `speaker_verification` (líneas 387-401) — ya no es solo modo sombra,
  documentar los 4 campos nuevos, remitir a la sección "Micrófono" como forma
  recomendada de enrolar (dejando `--enroll-voice` como alternativa sin
  navegador), mantener explícito que el umbral default es un placeholder.
- [`README.md`](../../README.md): agregar `speaker_verification` a la lista de
  secciones de `config.yaml` (hoy no aparece) + mención del enrollment guiado.

### A.6 — Verificación end-to-end

1. Enrolar la voz desde la web (countdown/progreso en vivo), confirmar `GET /api/status/speaker_verification` → `enrolled: true`.
2. Re-grabar y confirmar que sobrescribe sin dejar residuos.
3. `enabled: true` sin gating: confirmar que los logs de modo sombra siguen igual (regresión cero).
4. `gate_confirmations: true`, umbral bajo (ej. 0.3): confirmación de riesgo se aprueba normal con la propia voz.
5. Umbral alto (ej. 0.99, imposible de superar): se cancela con mensaje hablado claro, Jarvis queda listo para reintentar.
6. `gate_wait_ms` muy chico (ej. 5ms) para forzar `Inconclusive`: confirmar `Allow` (deja pasar + warning) y `Deny` (cancela) por separado.
7. `gate_confirmations: true` sin ningún embedding: confirmar warning al arrancar y que toda confirmación cae en `Inconclusive` sin colgarse.
8. Medir la latencia real agregada al aprobar con gating activo (debería quedar bien por debajo del segundo con el modelo ya cargado).
9. Revisión de código con agente sobre el diff acumulado, atención particular a: que `verify_speaker` no deje estado inconsistente si `WorkerDied` corta la espera a mitad de camino, y que `deferred_transcript` se reprocese sin perderse.

---

## Parte B — Wake word acústico con openWakeWord (exploratoria, NO se implementa ahora)

Diseño de referencia para una sesión aparte si el usuario decide avanzar — fuera del alcance de este trabajo.

- **Dónde correría**: hilo nuevo en `workers/stt_engine.py` (similar a `_check_speaker_async`, pero *síncrono en el hot path*, ya que el propósito es decidir *antes* de transcribir), acumulando los frames de 512 muestras/32ms ya leídos en `audio_loop` hasta juntar múltiplos de ~80ms (1280 muestras) para el modelo ONNX.
- **Supresión durante TTS**: reusar la señal de `ModeState.SUPPRESSED`/`echo_guard` que ya mitiga el mismo problema de eco para el VAD actual.
- **Mensaje IPC nuevo** (si se expone a Rust): `SttOutMessage::AcousticWakeDetected { confidence: f32 }` — señal adicional para decidir si vale la pena transcribir, no un reemplazo del mecanismo de texto actual.
- **Config nueva**: `wake.acoustic.enabled` (default `false`), `wake.acoustic.threshold`, dentro de `WakeConfig`.
- **Trade-off de UX a resolver antes de implementar**: el modelo pre-entrenado espera la frase fija "hey jarvis"; `wake.rs::contains_wake_word` hoy acepta "jarvis" en cualquier posición, con tolerancia Levenshtein≤1, sin repetirlo en la ventana de atención activa. Como reemplazo cambia la UX de activación; como complemento (filtro previo a Whisper) no la cambia pero tampoco resuelve "no todos hablamos igual" — eso es rol de la Parte A.
- **Licencia**: modelo pre-entrenado CC BY-NC-SA 4.0 (no comercial) — sin problema para uso personal, documentar como restricción si algún día se distribuye Jarvis.
- **Dependencia nueva**: `pip install openwakeword` (trae `onnxruntime`), extra opcional, mismo patrón que `requirements-speaker.txt`.

### Critical Files

- `src/orchestrator.rs` — núcleo del gating: `handle_confirmation`, nuevo `verify_speaker`.
- `src/stt/mod.rs` / `src/stt/protocol.rs` — propagación de `text_preview`, `enabled || gate_confirmations`, variantes IPC de enrollment.
- `src/stt/calibration.rs` / `src/config_ui/calibration.rs` — extensión del worker/WS de calibración con comandos de enrollment.
- `src/config.rs` — extensión de `SpeakerVerificationConfig`.
- `workers/calibration_engine.py` / `workers/stt_worker.py` — grabación + hilo de enrollment, reusando `workers/speaker_verification.py::SpeakerVerifier.enroll()`.
- `web/config-ui/src/sections/OnboardingSection.tsx` / `AgentSection.tsx` — UI de enrollment y campos de gating.
