//! Loop principal: espera una transcripción, silencia el micrófono mientras
//! Jarvis responde, corre el turno agéntico (LLM ↔ herramientas) y reactiva
//! la escucha. Si una herramienta requiere aprobación, el orquestador queda
//! en `AwaitingConfirmation`: pregunta por voz, reabre el micrófono y la
//! siguiente transcripción se interpreta como sí/no (o como el código de
//! aceptación de riesgos, verificado acá en Rust — nunca por el LLM).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::agent::{
    self,
    confirm::{self, CodeDecision, ConfirmDecision},
    AgentTurnResult, PendingConfirmation, TurnContext,
};
use crate::audio::{AudioPlayer, MusicPlayer};
use crate::config::{AgentConfig, BargeInConfig, BargeInMode, Config, SttEngineKind};
use crate::echo_gate::EchoGate;
use crate::errors::{Result, WorkerError};
use crate::llm::{self, ChatMessage, LlmProvider, Role};
use crate::memory::MemoryStore;
use crate::pipeline;
use crate::reminders::{self, DueReminder, ReminderStore};
use crate::stt::{SttEvent, SttMode, SttWorker};
use crate::tools::scripted_store::ScriptedToolStore;
use crate::tools::{system_info, ToolRegistry};
use crate::tts::{self, TtsProvider};
use crate::tui::{UiState, VisualState};
use crate::wake::{AttentionGate, GateDecision};

enum AgentState {
    Idle,
    AwaitingConfirmation {
        pending: PendingConfirmation,
        deadline: Instant,
    },
}

pub struct Orchestrator {
    config: Config,
    stt: SttWorker,
    llm: Arc<dyn LlmProvider>,
    tts: Arc<dyn TtsProvider>,
    player: AudioPlayer,
    history: Vec<ChatMessage>,
    gate: AttentionGate,
    registry: ToolRegistry,
    memory: Arc<MemoryStore>,
    /// Canal por el que el poller de recordatorios (`reminders::run_poller`,
    /// corriendo en su propia tarea) avisa recordatorios vencidos. El
    /// poller no puede hablar directo: no tiene acceso al `AudioPlayer`
    /// (`&mut self`-only, vive acá).
    reminder_rx: tokio::sync::mpsc::Receiver<DueReminder>,
    /// Recordatorios que vencieron mientras había un turno o una
    /// confirmación en curso; se hablan al volver a `Idle` (`finish_turn`)
    /// para no pisar el barge-in/la confirmación en marcha.
    pending_reminders: Vec<DueReminder>,
    /// Frases que Jarvis dijo hace poco, para descartar como eco propio
    /// transcripciones que lleguen mientras habla (barge-in). `Arc<Mutex<_>>`
    /// porque también lo necesita `pipeline::run_speaking_turn`, que corre
    /// en la misma tarea pero no puede tomar `&mut self`.
    echo_gate: Arc<Mutex<EchoGate>>,
    /// Bloque estático del system prompt (prompt base + memorias) cacheado
    /// como `(generación del MemoryStore, contenido)`. Ver
    /// [`Self::static_system_content`].
    system_static_cache: Option<(u64, String)>,
    state: AgentState,
    /// Cuántas veces se reinició el worker de STT en esta corrida, contra
    /// `config.workers.max_restarts`.
    stt_restarts: u32,
    /// Música de fondo del modo bienvenida (ver `crate::audio::music`).
    music: MusicPlayer,
    /// Clon propio para que `run_welcome` pueda leer los recordatorios
    /// activos sin competir con el que ya se movió a `reminders::run_poller`
    /// (ver `new`).
    reminder_store: Arc<ReminderStore>,
    /// Último doble aplauso que disparó la escena de bienvenida (no el
    /// toggle de apagar música), para el cooldown de `welcome.cooldown_secs`.
    last_welcome: Option<Instant>,
    /// Hook de prueba temporal para la Fase 2 (cancelación sin barge-in de
    /// voz todavía): si la variable de entorno `JARVIS_TEST_CANCEL` está
    /// presente, un solo hilo de fondo (vive todo el proceso) lee líneas de
    /// stdin y cancela el token del turno vigente en este slot. La Fase 3 lo
    /// reemplaza por el disparo real desde el motor de STT.
    test_cancel_slot: Option<Arc<Mutex<Option<CancellationToken>>>>,
    /// Publica transiciones de estado para la TUI (ver `crate::tui`). Se
    /// escribe siempre, aunque `config.ui.enabled` sea `false`: sin
    /// receptores activos, `set()` no cuesta nada relevante.
    ui: UiState,
    /// Nivel de energía del micrófono (0.0-1.0, normalizado desde dBFS), para
    /// que la TUI anime `UserSpeaking` con el volumen real de voz. Igual que
    /// `ui`, se escribe siempre aunque nadie la lea.
    mic_level_tx: watch::Sender<f32>,
}

impl Orchestrator {
    pub async fn new(config: Config, ui: UiState) -> Result<Self> {
        let stt = SttWorker::spawn(&config.workers, &config.stt, &config.barge_in).await?;
        let llm_provider = llm::build_provider(&config)?;
        let tts_provider = tts::build_provider(&config).await?;
        let player = AudioPlayer::new(
            config.audio.output_device.as_deref(),
            config.audio.volume,
            config.audio.drain_timeout_secs,
        )?;
        let music = MusicPlayer::new(&config.welcome);

        // Dos mensajes system: [0] el bloque estático (prompt base +
        // memorias, estable entre turnos para el prompt caching de los
        // proveedores) y [1] el dinámico (fecha/hora del turno).
        let history = vec![
            ChatMessage::system(config.llm.system_prompt.clone()),
            ChatMessage::system(String::new()),
        ];
        let gate = AttentionGate::new(config.wake.clone());
        let memory = Arc::new(MemoryStore::open(&config.agent.memory.db_path)?);
        let reminder_store = Arc::new(ReminderStore::open(&config.agent.reminders.db_path)?);
        let reminder_store_for_welcome = reminder_store.clone();
        let scripted_store = Arc::new(ScriptedToolStore::open(
            &config.agent.scripted_tools.db_path,
        )?);
        let registry = ToolRegistry::build(
            &config.agent,
            memory.clone(),
            reminder_store.clone(),
            scripted_store,
            if config.welcome.enabled {
                Some(music.shared())
            } else {
                None
            },
        )
        .await;
        let echo_gate = Arc::new(Mutex::new(EchoGate::new(
            config.barge_in.echo_guard.clone(),
        )));

        let (reminder_tx, reminder_rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(reminders::run_poller(
            reminder_store,
            reminder_tx,
            Duration::from_secs(config.agent.reminders.poll_interval_secs),
        ));

        let test_cancel_slot = if std::env::var_os("JARVIS_TEST_CANCEL").is_some() {
            tracing::warn!(
                "JARVIS_TEST_CANCEL activo: Enter en stdin cancela el turno en curso (hook de prueba de la Fase 2)"
            );
            let slot: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
            let slot_for_thread = slot.clone();
            std::thread::spawn(move || loop {
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line).is_err() || line.is_empty() {
                    break;
                }
                if let Some(token) = slot_for_thread.lock().unwrap().clone() {
                    tracing::info!("JARVIS_TEST_CANCEL: Enter detectado, cancelando el turno");
                    token.cancel();
                }
            });
            Some(slot)
        } else {
            None
        };

        Ok(Self {
            config,
            stt,
            llm: llm_provider,
            tts: tts_provider,
            player,
            history,
            gate,
            registry,
            memory,
            reminder_rx,
            pending_reminders: Vec::new(),
            echo_gate,
            system_static_cache: None,
            state: AgentState::Idle,
            stt_restarts: 0,
            music,
            reminder_store: reminder_store_for_welcome,
            last_welcome: None,
            test_cancel_slot,
            ui,
            mic_level_tx: watch::channel(0.0).0,
        })
    }

    /// Nivel de audio en reproducción (ver `AudioPlayer::level_rx`), para que
    /// `main.rs` se lo pase a la TUI sin exponer el `AudioPlayer` entero.
    pub fn audio_level_rx(&self) -> tokio::sync::watch::Receiver<f32> {
        self.player.level_rx()
    }

    /// Nivel de energía del micrófono (0.0-1.0), para animar `UserSpeaking`
    /// con el volumen real de voz en vez de un pulso sintético.
    pub fn mic_level_rx(&self) -> tokio::sync::watch::Receiver<f32> {
        self.mic_level_tx.subscribe()
    }

    /// Token de cancelación para un turno nuevo. Con `JARVIS_TEST_CANCEL` lo
    /// deja disponible para el hilo que vigila stdin (ver `new`).
    fn new_turn_cancellation(&self) -> CancellationToken {
        let token = CancellationToken::new();
        if let Some(slot) = &self.test_cancel_slot {
            *slot.lock().unwrap() = Some(token.clone());
        }
        token
    }

    /// Barge-in solo funciona con el motor nativo: RealtimeSTT no puede dar
    /// eventos de voz continuos mientras suena el TTS (`recorder.text()` es
    /// bloqueante), así que con `engine: realtimestt` siempre se usa el mute
    /// físico de siempre, sin importar `barge_in.enabled`.
    fn barge_in_supported(&self) -> bool {
        self.config.barge_in.enabled && self.config.stt.engine == SttEngineKind::Native
    }

    /// Al empezar a hablar: con barge-in soportado, pasa el STT a modo
    /// "speaking" (sigue escuchando, con umbral de VAD elevado) en vez de
    /// mutear físicamente — así se puede detectar que el usuario habla
    /// encima. Si no, el comportamiento de siempre (mute real).
    async fn begin_speaking(&mut self) {
        // "Pensando": arranca el turno (mic en modo speaking/mute) antes de
        // que haya audio de Jarvis. La TUI promueve esto a "hablando" sola
        // en cuanto detecta nivel de audio real (ver `crate::tui::run`), sin
        // que el orquestador necesite saber cuándo empieza a sonar el TTS.
        self.ui.set(VisualState::Thinking);
        if self.music.is_playing() {
            self.music.duck();
        }
        if self.barge_in_supported() {
            if let Err(e) = self.stt.set_mode(SttMode::Speaking).await {
                tracing::warn!(error = %e, "no se pudo poner el STT en modo speaking, sigo igual");
            }
        } else if let Err(e) = self.stt.mute().await {
            tracing::warn!(error = %e, "no se pudo silenciar el micrófono, sigo igual");
        }
    }

    /// Contraparte de `begin_speaking`: vuelve a escucha normal.
    async fn end_speaking(&mut self) {
        self.ui.set(VisualState::Listening);
        if self.music.is_playing() {
            self.music.unduck();
        }
        if self.barge_in_supported() {
            if let Err(e) = self.stt.set_mode(SttMode::Listening).await {
                tracing::warn!(error = %e, "no se pudo volver el STT a modo listening, sigo igual");
            }
        } else if let Err(e) = self.stt.unmute().await {
            tracing::warn!(error = %e, "no se pudo reactivar el micrófono");
        }
    }

    /// Doble aplauso confirmado por el motor STT nativo (ver `ClapInit`).
    /// Con música sonando es un toggle: la apaga y no dispara la escena. Si
    /// no, dispara `run_welcome` solo si el modo está habilitado, Jarvis
    /// está libre (`Idle`) y no está dentro del cooldown del último disparo.
    async fn handle_clap(&mut self) {
        tracing::info!("doble aplauso detectado");
        if !self.config.welcome.enabled {
            tracing::debug!("doble aplauso ignorado: welcome.enabled=false");
            return;
        }
        if self.music.is_playing() {
            tracing::info!("doble aplauso: música sonando, la apago (toggle)");
            self.music.stop();
            return;
        }
        if !matches!(self.state, AgentState::Idle) {
            tracing::debug!("doble aplauso ignorado: hay un turno o confirmación en curso");
            return;
        }
        if let Some(last) = self.last_welcome {
            let cooldown = Duration::from_secs(self.config.welcome.cooldown_secs);
            if last.elapsed() < cooldown {
                tracing::debug!("doble aplauso ignorado: dentro del cooldown de bienvenida");
                return;
            }
        }
        tracing::info!("disparando escena de bienvenida");
        self.last_welcome = Some(Instant::now());
        self.run_welcome().await;
    }

    /// Escena de bienvenida: música + saludo + resumen de recordatorios
    /// pendientes (o noticias del día si no hay ninguno).
    async fn run_welcome(&mut self) {
        let music_path = self.config.welcome.music_path.clone();
        let output_device = self.config.audio.output_device.clone();
        if let Err(e) = self.music.play_file(&music_path, output_device.as_deref()) {
            tracing::warn!(error = %e, "no se pudo reproducir la música de bienvenida, sigo sin música");
        }

        self.begin_speaking().await;
        agent::speak(
            &self.tts,
            &mut self.player,
            &self.config.welcome.greeting_phrase,
        )
        .await;
        self.end_speaking().await;

        match self.reminder_store.list_active().await {
            Ok(reminders) if !reminders.is_empty() => {
                let listado = reminders
                    .iter()
                    .map(|r| format!("- {} ({})", r.text, r.trigger_at))
                    .collect::<Vec<_>>()
                    .join("\n");
                let prompt = format!(
                    "[Evento del sistema: el usuario acaba de llegar a casa (modo bienvenida). \
                     Ya lo saludaste recién, no repitas el saludo ni digas 'bienvenido' de \
                     nuevo — andá directo al resumen. Recordatorios pendientes:\n{listado}\n\
                     Resumíselos brevemente.]"
                );
                self.handle_utterance(prompt).await;
            }
            Ok(_) if self.config.welcome.news_when_no_reminders => {
                let prompt = "[Evento del sistema: el usuario acaba de llegar a casa (modo \
                    bienvenida) y no tiene recordatorios pendientes. Ya lo saludaste recién, no \
                    repitas el saludo ni digas 'bienvenido' de nuevo. Contale brevemente las \
                    noticias más relevantes de hoy usando web_search.]"
                    .to_string();
                self.handle_utterance(prompt).await;
            }
            Ok(_) => {
                self.begin_speaking().await;
                agent::speak(
                    &self.tts,
                    &mut self.player,
                    "No tiene recordatorios pendientes, señor.",
                )
                .await;
                self.finish_turn().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "no se pudieron leer los recordatorios activos para el modo bienvenida");
                self.finish_turn().await;
            }
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("Jarvis listo. Escuchando...");
        loop {
            let event = tokio::select! {
                biased;
                due = self.reminder_rx.recv() => {
                    if let Some(due) = due {
                        self.handle_due_reminder(due).await;
                    }
                    continue;
                }
                event = self.stt.next_event() => event,
            };
            let Some(event) = event else { break };
            match event {
                SttEvent::Transcript { text, meta, .. } => {
                    if let Some(meta) = &meta {
                        tracing::debug!(
                            speech_ms = ?meta.speech_ms,
                            transcribe_ms = ?meta.transcribe_ms,
                            rms_dbfs = ?meta.rms_dbfs,
                            no_speech_prob = ?meta.no_speech_prob,
                            avg_logprob = ?meta.avg_logprob,
                            "telemetría de transcripción"
                        );
                    }
                    if text.trim().is_empty() {
                        continue;
                    }
                    self.on_transcript(text).await;
                }
                SttEvent::VadStart => {
                    tracing::debug!("VAD: inicio de voz detectado");
                    self.ui.set(VisualState::UserSpeaking);
                    // Red de seguridad: si el VAD dispara pero no llega a
                    // generar un turno (el gate lo dropea/ignora), el
                    // `unduck()` de contrapartida es el de VadEnd, no el de
                    // `end_speaking` (que acá no se llama).
                    if matches!(self.state, AgentState::Idle) && self.music.is_playing() {
                        self.music.duck();
                    }
                }
                SttEvent::VadEnd { speech_ms } => {
                    tracing::debug!(speech_ms = ?speech_ms, "VAD: fin de voz detectado");
                    self.ui.set(VisualState::Listening);
                    if matches!(self.state, AgentState::Idle) && self.music.is_playing() {
                        self.music.unduck();
                    }
                }
                SttEvent::SpeechConfirmed => {
                    // Fuera de un turno en curso esto no debería pasar (el
                    // modo "speaking" solo se activa mientras Jarvis habla),
                    // pero si llega tarde/desfasado no hay nada que hacer.
                    tracing::debug!("barge-in: speech_confirmed fuera de turno, se ignora");
                }
                SttEvent::Discarded { reason } => {
                    tracing::debug!(reason = %reason, "audio descartado por el motor STT");
                }
                SttEvent::ClapDetected => self.handle_clap().await,
                SttEvent::Level { dbfs } => {
                    self.mic_level_tx.send_replace(normalize_mic_level(dbfs));
                }
                SttEvent::WorkerDied => {
                    self.restart_stt_or_die().await?;
                }
            }
        }
        Ok(())
    }

    /// Un recordatorio venció. Si Jarvis está libre (`Idle`), lo dice de
    /// inmediato; si hay un turno o una confirmación en curso, lo encola
    /// para hablarlo al volver a `Idle` (`finish_turn`), en vez de pisar el
    /// barge-in o la confirmación en marcha.
    async fn handle_due_reminder(&mut self, due: DueReminder) {
        if matches!(self.state, AgentState::Idle) {
            self.speak_reminder(&due).await;
        } else {
            self.pending_reminders.push(due);
        }
    }

    async fn speak_reminder(&mut self, due: &DueReminder) {
        tracing::info!(id = due.id, text = %due.text, "recordatorio vencido");
        self.begin_speaking().await;
        agent::speak(
            &self.tts,
            &mut self.player,
            &format!("Recordatorio, señor: {}", due.text),
        )
        .await;
        self.end_speaking().await;
    }

    /// El worker de STT murió (crash real, o se auto-terminó porque su propio
    /// watchdog lo detectó irrecuperablemente colgado — ver
    /// `workers/stt_worker.py::watchdog_loop`). Si `restart_on_crash` está
    /// activo y queda presupuesto en `max_restarts`, lo reemplaza por uno
    /// nuevo y Jarvis sigue escuchando sin perder historial ni el estado de
    /// la conversación. Si no, propaga el error (comportamiento previo: la
    /// app termina).
    async fn restart_stt_or_die(&mut self) -> Result<()> {
        if !self.config.workers.restart_on_crash
            || self.stt_restarts >= self.config.workers.max_restarts
        {
            return Err(WorkerError::Crashed(None).into());
        }
        self.stt_restarts += 1;
        tracing::warn!(
            intento = self.stt_restarts,
            maximo = self.config.workers.max_restarts,
            "el worker de STT se cayó o quedó colgado; reiniciándolo"
        );
        self.ui
            .set(VisualState::Error("worker STT reiniciado".to_string()));
        self.stt = SttWorker::spawn(
            &self.config.workers,
            &self.config.stt,
            &self.config.barge_in,
        )
        .await?;
        tracing::info!("worker de STT reiniciado, Jarvis sigue escuchando");
        Ok(())
    }

    async fn on_transcript(&mut self, text: String) {
        // ¿Hay una confirmación pendiente? Toda transcripción cuenta como
        // respuesta (bypass del wake gate: el usuario ya está en diálogo).
        if matches!(self.state, AgentState::AwaitingConfirmation { .. }) {
            let AgentState::AwaitingConfirmation { pending, deadline } =
                std::mem::replace(&mut self.state, AgentState::Idle)
            else {
                unreachable!();
            };
            self.handle_confirmation(pending, deadline, text).await;
            return;
        }

        self.dispatch_by_gate(text).await;
    }

    async fn dispatch_by_gate(&mut self, text: String) {
        match self.gate.decide(&text) {
            GateDecision::Drop => {
                tracing::info!(text = %text, "ignorado: probable alucinación o frase-basura");
            }
            GateDecision::Ignore => {
                tracing::info!(text = %text, "ignorado: sin wake word y fuera de ventana");
                // Con música sonando, lo que transcribe el motor suele ser
                // letra de la canción: no envenenar el contexto ambiental
                // con eso.
                if !self.music.is_playing() {
                    self.gate.push_ambient(text);
                }
            }
            GateDecision::Respond => {
                tracing::info!(text = %text, "usuario dijo");
                self.gate.mark_responded(&text);
                self.handle_utterance(text).await;
            }
        }
    }

    /// Procesa una frase del usuario y, si `barge_in` está activo, encadena
    /// sin recursión la interrupción que haya quedado confirmada durante la
    /// respuesta (en vez de volver a llamarse a sí misma — Rust no permite
    /// `async fn` recursivas sin bloquear el Future en el heap).
    async fn handle_utterance(&mut self, mut user_text: String) {
        loop {
            let content = match self.gate.take_ambient_context() {
                Some(ambient) => format!("{ambient}\n{user_text}"),
                None => user_text.clone(),
            };
            self.history.push(ChatMessage::user(content));

            self.begin_speaking().await;

            // history[0] es el bloque estático (prompt base + memorias,
            // cacheado) y history[1] el dinámico (fecha/hora de este turno).
            self.history[0] = ChatMessage::system(self.static_system_content().await);
            self.history[1] = ChatMessage::system(format!(
                "Contexto actual: hoy es {} (hora local).",
                system_info::fecha_hora_es()
            ));

            let cancel = self.new_turn_cancellation();
            let (result, next_utterance) = if self.barge_in_supported() {
                self.run_turn_racing_stt(cancel).await
            } else {
                let result = if self.registry.is_empty() {
                    self.plain_speaking_turn(cancel).await
                } else {
                    // Sin carrera de STT en este camino (barge-in no soportado): nunca pausa.
                    let (_pause_tx, pause_rx) = watch::channel(false);
                    let mut ctx = TurnContext {
                        llm: &self.llm,
                        tts: &self.tts,
                        player: &mut self.player,
                        registry: &self.registry,
                        config: &self.config,
                        cancel,
                        echo_gate: self.echo_gate.clone(),
                        pause_rx,
                        ui: self.ui.clone(),
                    };
                    agent::run_agentic_turn(&mut ctx, &mut self.history).await
                };
                (result, None)
            };

            self.conclude_turn(result).await;

            match next_utterance {
                Some(text) => {
                    self.gate.mark_responded(&text);
                    user_text = text;
                }
                None => break,
            }
        }
    }

    /// Corre el turno mientras sigue leyendo eventos de STT en paralelo,
    /// para detectar que el usuario empezó a hablar encima de Jarvis
    /// (barge-in) y cancelar. `run_speaking_turn`/`run_agentic_turn` ya
    /// saben desenrollarse solos al ver `cancel` disparado (Fase 2); acá
    /// solo se decide CUÁNDO dispararlo y con qué texto seguir.
    ///
    /// No usa `restart_stt_or_die` a mitad de carrera si el worker muere:
    /// deja que el turno actual termine solo (no depende de STT para nada)
    /// y recién después intenta reiniciarlo.
    async fn run_turn_racing_stt(
        &mut self,
        cancel: CancellationToken,
    ) -> (crate::errors::Result<AgentTurnResult>, Option<String>) {
        let mut interrupt_text: Option<String> = None;
        let mut stt_died = false;
        let mut paused = false;
        let (pause_tx, pause_rx) = watch::channel(false);

        let turn_result = if self.registry.is_empty() {
            let out_result = {
                let turn_future = pipeline::run_speaking_turn(
                    self.llm.clone(),
                    self.tts.clone(),
                    &mut self.player,
                    &self.history,
                    Arc::new(Vec::new()),
                    &self.config.pipeline,
                    cancel.clone(),
                    self.echo_gate.clone(),
                    pause_rx.clone(),
                );
                tokio::pin!(turn_future);
                loop {
                    if stt_died {
                        break turn_future.await;
                    }
                    tokio::select! {
                        biased;
                        out = &mut turn_future => break out,
                        event = self.stt.next_event() => {
                            match event {
                                Some(SttEvent::WorkerDied) | None => stt_died = true,
                                Some(event) => {
                                    handle_barge_in_event(
                                        event,
                                        &mut self.gate,
                                        &self.echo_gate,
                                        &self.config.barge_in,
                                        &self.config.agent,
                                        &self.llm,
                                        &cancel,
                                        &pause_tx,
                                        &mut paused,
                                        &mut interrupt_text,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
            };
            out_result.map(|out| {
                if out.interrupted {
                    AgentTurnResult::Interrupted {
                        spoken_so_far: out.spoken_text,
                    }
                } else {
                    self.history
                        .push(ChatMessage::assistant(out.spoken_text.clone()));
                    AgentTurnResult::Completed {
                        final_text: out.spoken_text,
                    }
                }
            })
        } else {
            let mut ctx = TurnContext {
                llm: &self.llm,
                tts: &self.tts,
                player: &mut self.player,
                registry: &self.registry,
                config: &self.config,
                cancel: cancel.clone(),
                echo_gate: self.echo_gate.clone(),
                pause_rx: pause_rx.clone(),
                ui: self.ui.clone(),
            };
            let turn_future = agent::run_agentic_turn(&mut ctx, &mut self.history);
            tokio::pin!(turn_future);
            loop {
                if stt_died {
                    break turn_future.await;
                }
                tokio::select! {
                    biased;
                    result = &mut turn_future => break result,
                    event = self.stt.next_event() => {
                        match event {
                            Some(SttEvent::WorkerDied) | None => stt_died = true,
                            Some(event) => {
                                handle_barge_in_event(
                                    event,
                                    &mut self.gate,
                                    &self.echo_gate,
                                    &self.config.barge_in,
                                    &self.config.agent,
                                    &self.llm,
                                    &cancel,
                                    &pause_tx,
                                    &mut paused,
                                    &mut interrupt_text,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        };

        if stt_died {
            if let Err(e) = self.restart_stt_or_die().await {
                return (Err(e), None);
            }
        }

        (turn_result, interrupt_text)
    }

    /// Turno clásico sin herramientas (agent.enabled: false).
    async fn plain_speaking_turn(
        &mut self,
        cancel: CancellationToken,
    ) -> crate::errors::Result<AgentTurnResult> {
        // Sin carrera de STT en este camino (barge-in no soportado): nunca pausa.
        let (_pause_tx, pause_rx) = watch::channel(false);
        let out = pipeline::run_speaking_turn(
            self.llm.clone(),
            self.tts.clone(),
            &mut self.player,
            &self.history,
            Arc::new(Vec::new()),
            &self.config.pipeline,
            cancel,
            self.echo_gate.clone(),
            pause_rx,
        )
        .await?;
        if out.interrupted {
            return Ok(AgentTurnResult::Interrupted {
                spoken_so_far: out.spoken_text,
            });
        }
        self.history
            .push(ChatMessage::assistant(out.spoken_text.clone()));
        Ok(AgentTurnResult::Completed {
            final_text: out.spoken_text,
        })
    }

    /// Cierra el turno según el resultado del loop agéntico: o quedó
    /// completo, o queda esperando una confirmación por voz.
    async fn conclude_turn(&mut self, result: crate::errors::Result<AgentTurnResult>) {
        match result {
            Ok(AgentTurnResult::Completed { final_text }) => {
                tracing::info!(reply = %final_text, "Jarvis respondió");
                self.finish_turn().await;
            }
            Ok(AgentTurnResult::Interrupted { spoken_so_far }) => {
                tracing::info!(spoken = %spoken_so_far, "el turno se interrumpió a mitad de respuesta");
                if !spoken_so_far.trim().is_empty() {
                    self.history.push(ChatMessage::assistant(format!(
                        "{spoken_so_far} [Nota: el usuario te interrumpió justo acá; no llegaste \
                         a terminar de decir esto. Si retomás el tema, hacelo con naturalidad, sin \
                         repetir lo ya dicho.]"
                    )));
                }
                self.finish_turn().await;
            }
            Ok(AgentTurnResult::NeedsConfirmation(pending)) => {
                tracing::info!(
                    tool = %pending.call.name,
                    requires_code = pending.requires_code,
                    "esperando confirmación por voz"
                );
                agent::speak(&self.tts, &mut self.player, &pending.spoken_question).await;
                let deadline =
                    Instant::now() + Duration::from_secs(self.config.agent.confirm_timeout_secs);
                self.state = AgentState::AwaitingConfirmation { pending, deadline };
                self.end_speaking().await;
                self.ui.set(VisualState::AwaitingConfirmation);
            }
            Err(e) => {
                tracing::error!(error = %e, "fallo generando la respuesta");
                self.finish_turn().await;
            }
        }
    }

    async fn handle_confirmation(
        &mut self,
        pending: PendingConfirmation,
        deadline: Instant,
        text: String,
    ) {
        if Instant::now() > deadline {
            tracing::info!("la confirmación expiró; se cancela la acción pendiente");
            self.cancel_pending(
                pending,
                "El usuario no respondió a tiempo; la acción fue cancelada.",
            );
            // La frase que llegó tarde se procesa como una petición normal.
            self.dispatch_by_gate(text).await;
            return;
        }

        if pending.requires_code {
            match confirm::interpret_code(&text, &self.config.agent) {
                CodeDecision::Correct => {
                    tracing::info!("código de aceptación correcto; se ejecuta la acción");
                    self.approve_pending(pending).await;
                }
                CodeDecision::Wrong => {
                    tracing::info!("código de aceptación incorrecto; acción cancelada");
                    self.cancel_pending(
                        pending,
                        "El usuario dio un código de aceptación incorrecto; la acción fue cancelada.",
                    );
                    agent::speak(
                        &self.tts,
                        &mut self.player,
                        "Código incorrecto. Acción cancelada, señor.",
                    )
                    .await;
                    self.finish_turn().await;
                }
                CodeDecision::Cancelled => {
                    self.cancel_and_acknowledge(pending).await;
                }
                CodeDecision::Unrelated => {
                    self.cancel_pending(
                        pending,
                        "El usuario cambió de tema; la acción fue cancelada.",
                    );
                    self.handle_utterance(text).await;
                }
            }
        } else {
            match confirm::interpret(&text, &self.config.agent) {
                ConfirmDecision::Yes => {
                    tracing::info!("acción confirmada por voz");
                    self.approve_pending(pending).await;
                }
                ConfirmDecision::No => {
                    self.cancel_and_acknowledge(pending).await;
                }
                ConfirmDecision::Unrelated => {
                    self.cancel_pending(
                        pending,
                        "El usuario cambió de tema; la acción fue cancelada.",
                    );
                    self.handle_utterance(text).await;
                }
            }
        }
    }

    /// El usuario aprobó: ejecutar la herramienta pendiente y retomar el
    /// loop agéntico donde quedó (las restantes pueden pedir sus propias
    /// confirmaciones).
    async fn approve_pending(&mut self, pending: PendingConfirmation) {
        self.begin_speaking().await;
        let cancel = self.new_turn_cancellation();
        // Fuera de una carrera contra el STT: este receptor nunca cambia,
        // así que nunca pausa (ver doc de `TurnContext::pause_rx`).
        let (_pause_tx, pause_rx) = watch::channel(false);
        let mut ctx = TurnContext {
            llm: &self.llm,
            tts: &self.tts,
            player: &mut self.player,
            registry: &self.registry,
            config: &self.config,
            cancel,
            echo_gate: self.echo_gate.clone(),
            pause_rx,
            ui: self.ui.clone(),
        };
        let result = agent::resume_agentic_turn(&mut ctx, &mut self.history, pending).await;
        self.conclude_turn(result).await;
    }

    /// Registra la cancelación en el historial. Invariante del protocolo:
    /// TODO tool call recibe siempre un tool_result (aunque sea "cancelado")
    /// — OpenAI/Anthropic lo exigen y a Ollama le da coherencia.
    fn cancel_pending(&mut self, pending: PendingConfirmation, reason: &str) {
        self.history.push(ChatMessage::tool_result(
            &pending.call.id,
            &pending.call.name,
            reason,
        ));
        for call in &pending.remaining_calls {
            self.history.push(ChatMessage::tool_result(
                &call.id,
                &call.name,
                "Cancelada junto con la acción anterior.",
            ));
        }
    }

    /// Cancela y deja que el LLM dé el acuse natural ("Como desee, señor"),
    /// en una pasada final sin herramientas.
    async fn cancel_and_acknowledge(&mut self, pending: PendingConfirmation) {
        tracing::info!("acción cancelada por el usuario");
        self.cancel_pending(pending, "El usuario canceló la acción.");
        self.begin_speaking().await;
        let cancel = self.new_turn_cancellation();
        let result = self.plain_speaking_turn(cancel).await;
        self.conclude_turn(result).await;
    }

    /// Cierre común de un turno completado: reabrir ventana de atención y
    /// micrófono, y recortar historial.
    async fn finish_turn(&mut self) {
        self.state = AgentState::Idle;

        // Incluso si el pipeline falló: el usuario querrá reintentar sin
        // repetir el nombre.
        self.gate.open_window();

        self.end_speaking().await;

        self.trim_history();

        // Recordatorios que vencieron mientras el turno estaba en curso.
        let due = std::mem::take(&mut self.pending_reminders);
        for reminder in due {
            self.speak_reminder(&reminder).await;
        }
    }

    /// Parte estática del system prompt: prompt base + memorias persistentes
    /// recientes (para que Jarvis conozca al usuario sin tener que llamar a
    /// `recall` en cada turno). Se cachea y solo se reconstruye cuando la
    /// generación del MemoryStore cambió (alguna tool remember/forget
    /// escribió) — evita una consulta a SQLite por turno y mantiene estable
    /// el prefijo para el prompt caching de los proveedores en nube. La
    /// fecha/hora va en un segundo mensaje system aparte, justamente para no
    /// invalidar este prefijo en cada turno.
    async fn static_system_content(&mut self) -> String {
        let generation = self.memory.generation();
        if let Some((cached_gen, content)) = &self.system_static_cache {
            if *cached_gen == generation {
                return content.clone();
            }
        }

        let mut content = self.config.llm.system_prompt.clone();
        let max = self.config.agent.memory.max_injected;
        match self.memory.all_recent(max).await {
            Ok(memories) if !memories.is_empty() => {
                content.push_str("\n\nCosas que sabes del usuario de sesiones anteriores:");
                for m in &memories {
                    content.push_str(&format!("\n- {}", m.content));
                }
                if memories.len() >= max {
                    content
                        .push_str("\n(Si necesitas algo más antiguo, usa la herramienta recall.)");
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "no se pudieron leer las memorias"),
        }
        self.system_static_cache = Some((generation, content.clone()));
        content
    }

    /// Conserva los dos system prompts (siempre los dos primeros mensajes) +
    /// los últimos `max_history_messages` mensajes de la conversación. El
    /// corte solo puede caer en un mensaje `User`: nunca debe quedar un
    /// `Tool` huérfano ni un `Assistant` con tool_calls sin sus resultados
    /// (rompe los protocolos de OpenAI/Anthropic y confunde a Ollama).
    fn trim_history(&mut self) {
        let max = self.config.llm.max_history_messages;
        if self.history.len() <= max + 2 {
            return;
        }
        let mut tail_start = (self.history.len() - max).max(2);
        while tail_start < self.history.len() && self.history[tail_start].role != Role::User {
            tail_start += 1;
        }
        let mut trimmed = Vec::with_capacity(self.history.len() - tail_start + 2);
        trimmed.push(self.history[0].clone());
        trimmed.push(self.history[1].clone());
        trimmed.extend_from_slice(&self.history[tail_start..]);
        self.history = trimmed;
    }

    pub async fn shutdown(&self) {
        self.stt.shutdown().await;
        self.tts.shutdown().await;
    }
}

/// Qué hacer con un evento de STT llegado MIENTRAS Jarvis está hablando,
/// según `classify_barge_in_event` (síncrona, sin tocar el LLM todavía).
enum BargeInAction {
    /// Nada que hacer (evento de solo telemetría, o ya estaba resuelto).
    Ignore,
    /// Dejar de hablar frases nuevas sin cortar la que suena, mientras se
    /// espera la transcripción para decidir si de verdad hay que cortar.
    Pause,
    /// Era eco propio, o el segmento se descartó, o no hacía falta pausar:
    /// seguir hablando con normalidad.
    Resume,
    /// Interrupción confirmada sin necesidad de chequeo de relevancia
    /// (modo `wake_word` con el nombre presente en la frase).
    Confirmed(String),
    /// Modo `any_voice`, no es eco: hace falta preguntarle al LLM si esto
    /// tiene sentido como algo dirigido a Jarvis antes de decidir.
    NeedsRelevanceCheck { text: String, was_saying: String },
}

/// Clasifica un evento de STT llegado durante un turno (dentro de
/// `Orchestrator::run_turn_racing_stt`). Puro y síncrono a propósito: el
/// chequeo de relevancia (que sí necesita `.await`-ear al LLM) se resuelve
/// aparte, en `handle_barge_in_event`, para no mantener el `MutexGuard` de
/// `echo_gate` atravesado por un `.await` (no es `Send`).
fn classify_barge_in_event(
    gate: &AttentionGate,
    echo_gate: &mut EchoGate,
    barge_in: &BargeInConfig,
    event: SttEvent,
    already_paused: bool,
) -> BargeInAction {
    match event {
        SttEvent::SpeechConfirmed => {
            // Solo any_voice pausa acá, sin esperar la transcripción (llega
            // cientos de ms después). wake_word no toca nada todavía: espera
            // el Transcript con el nombre.
            if barge_in.mode == BargeInMode::AnyVoice {
                tracing::info!("barge-in: voz sostenida detectada, pausando la respuesta");
                BargeInAction::Pause
            } else {
                BargeInAction::Ignore
            }
        }
        SttEvent::Transcript { text, meta, .. } => {
            if let Some(meta) = &meta {
                tracing::debug!(
                    speech_ms = ?meta.speech_ms,
                    rms_dbfs = ?meta.rms_dbfs,
                    "telemetría de transcripción durante barge-in"
                );
            }
            if text.trim().is_empty() {
                return BargeInAction::Ignore;
            }
            if echo_gate.is_echo(&text) {
                tracing::debug!(text = %text, "barge-in: descartado por el echo gate (probable eco propio)");
                return BargeInAction::Resume;
            }
            match barge_in.mode {
                BargeInMode::WakeWord => {
                    if gate.contains_wake_word(&text) {
                        tracing::info!(text = %text, "barge-in: interrupción confirmada (wake word)");
                        BargeInAction::Confirmed(text)
                    } else {
                        tracing::debug!(text = %text, "barge-in: sin wake word, se ignora (modo wake_word)");
                        BargeInAction::Ignore
                    }
                }
                BargeInMode::AnyVoice => {
                    let was_saying = echo_gate.recent_spoken_text();
                    BargeInAction::NeedsRelevanceCheck { text, was_saying }
                }
            }
        }
        SttEvent::VadStart => {
            tracing::debug!("VAD: inicio de voz detectado (posible barge-in)");
            BargeInAction::Ignore
        }
        SttEvent::VadEnd { speech_ms } => {
            tracing::debug!(speech_ms = ?speech_ms, "VAD: fin de voz detectado");
            BargeInAction::Ignore
        }
        SttEvent::Discarded { reason } => {
            tracing::debug!(reason = %reason, "audio descartado por el motor STT durante barge-in");
            // Si se había pausado esperando este segmento y no dio texto,
            // hay que reanudar: si no, Jarvis quedaría mudo para siempre.
            if already_paused {
                BargeInAction::Resume
            } else {
                BargeInAction::Ignore
            }
        }
        // v1: un doble aplauso a mitad de turno se ignora, no interrumpe la
        // respuesta en curso ni dispara la escena de bienvenida encima.
        SttEvent::ClapDetected => BargeInAction::Ignore,
        // Telemetría de nivel, no afecta la decisión de barge-in (el nivel
        // en sí se publica aparte, en el loop principal de `run()`).
        SttEvent::Level { .. } => BargeInAction::Ignore,
        SttEvent::WorkerDied => BargeInAction::Ignore,
    }
}

/// Convierte dBFS a un nivel 0.0-1.0 para la TUI. No depende del piso de
/// energía calibrado por el motor nativo (`energy_floor_dbfs`, específico de
/// cada máquina/micrófono): es una señal puramente visual, no de decisión,
/// así que alcanza con un rango fijo razonable para voz.
fn normalize_mic_level(dbfs: f32) -> f32 {
    const FLOOR_DBFS: f32 = -50.0;
    const RANGE_DB: f32 = 35.0;
    ((dbfs - FLOOR_DBFS) / RANGE_DB).clamp(0.0, 1.0)
}

/// Resuelve un evento de STT llegado durante un turno: clasifica (síncrono,
/// bajo el lock de `echo_gate`) y, si hace falta, corre el chequeo de
/// relevancia contra el LLM (fuera del lock) antes de pausar, reanudar o
/// confirmar la interrupción de verdad.
#[allow(clippy::too_many_arguments)]
async fn handle_barge_in_event(
    event: SttEvent,
    gate: &mut AttentionGate,
    echo_gate: &Arc<Mutex<EchoGate>>,
    barge_in: &BargeInConfig,
    agent_cfg: &AgentConfig,
    llm: &Arc<dyn LlmProvider>,
    cancel: &CancellationToken,
    pause_tx: &watch::Sender<bool>,
    paused: &mut bool,
    interrupt_text: &mut Option<String>,
) {
    let action = {
        let mut eg = echo_gate.lock().unwrap();
        classify_barge_in_event(gate, &mut eg, barge_in, event, *paused)
    };
    match action {
        BargeInAction::Ignore => {}
        BargeInAction::Pause => {
            *paused = true;
            let _ = pause_tx.send(true);
        }
        BargeInAction::Resume => {
            *paused = false;
            let _ = pause_tx.send(false);
        }
        BargeInAction::Confirmed(text) => {
            if !cancel.is_cancelled() {
                cancel.cancel();
            }
            *interrupt_text = Some(text);
        }
        BargeInAction::NeedsRelevanceCheck { text, was_saying } => {
            let timeout = Duration::from_secs(barge_in.relevance_timeout_secs);
            let relevant =
                agent::relevance::sounds_directed_at_jarvis(llm, &was_saying, &text, agent_cfg, timeout)
                    .await;
            if relevant {
                tracing::info!(text = %text, "barge-in: interrupción confirmada tras chequeo de relevancia");
                if !cancel.is_cancelled() {
                    cancel.cancel();
                }
                *interrupt_text = Some(text);
            } else {
                tracing::debug!(text = %text, "barge-in: no parece dirigido a Jarvis, sigue hablando");
                *paused = false;
                let _ = pause_tx.send(false);
                gate.push_ambient(text);
            }
        }
    }
}
