//! Loop principal: espera una transcripción, silencia el micrófono mientras
//! Jarvis responde, corre el turno agéntico (LLM ↔ herramientas) y reactiva
//! la escucha. Si una herramienta requiere aprobación, el orquestador queda
//! en `AwaitingConfirmation`: pregunta por voz, reabre el micrófono y la
//! siguiente transcripción se interpreta como sí/no (o como el código de
//! aceptación de riesgos, verificado acá en Rust — nunca por el LLM).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::agent::{
    self,
    confirm::{self, CodeDecision, ConfirmDecision},
    AgentTurnResult, PendingConfirmation, TurnContext,
};
use crate::audio::AudioPlayer;
use crate::config::Config;
use crate::errors::{Result, WorkerError};
use crate::llm::{self, ChatMessage, LlmProvider, Role};
use crate::memory::MemoryStore;
use crate::pipeline;
use crate::stt::{SttEvent, SttWorker};
use crate::tools::{system_info, ToolRegistry};
use crate::tts::{self, TtsProvider};
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
    /// Bloque estático del system prompt (prompt base + memorias) cacheado
    /// como `(generación del MemoryStore, contenido)`. Ver
    /// [`Self::static_system_content`].
    system_static_cache: Option<(u64, String)>,
    state: AgentState,
    /// Cuántas veces se reinició el worker de STT en esta corrida, contra
    /// `config.workers.max_restarts`.
    stt_restarts: u32,
}

impl Orchestrator {
    pub async fn new(config: Config) -> Result<Self> {
        let stt = SttWorker::spawn(&config.workers, &config.stt).await?;
        let llm_provider = llm::build_provider(&config)?;
        let tts_provider = tts::build_provider(&config).await?;
        let player = AudioPlayer::new(
            config.audio.output_device.as_deref(),
            config.audio.volume,
            config.audio.drain_timeout_secs,
        )?;

        // Dos mensajes system: [0] el bloque estático (prompt base +
        // memorias, estable entre turnos para el prompt caching de los
        // proveedores) y [1] el dinámico (fecha/hora del turno).
        let history = vec![
            ChatMessage::system(config.llm.system_prompt.clone()),
            ChatMessage::system(String::new()),
        ];
        let gate = AttentionGate::new(config.wake.clone());
        let memory = Arc::new(MemoryStore::open(&config.agent.memory.db_path)?);
        let registry = ToolRegistry::build(&config.agent, memory.clone());

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
            system_static_cache: None,
            state: AgentState::Idle,
            stt_restarts: 0,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("Jarvis listo. Escuchando...");
        while let Some(event) = self.stt.next_transcript().await {
            match event {
                SttEvent::Transcript { text } => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    self.on_transcript(text).await;
                }
                SttEvent::WorkerDied => {
                    self.restart_stt_or_die().await?;
                }
            }
        }
        Ok(())
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
        self.stt = SttWorker::spawn(&self.config.workers, &self.config.stt).await?;
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
                self.gate.push_ambient(text);
            }
            GateDecision::Respond => {
                tracing::info!(text = %text, "usuario dijo");
                self.gate.mark_responded(&text);
                self.handle_utterance(text).await;
            }
        }
    }

    async fn handle_utterance(&mut self, user_text: String) {
        let content = match self.gate.take_ambient_context() {
            Some(ambient) => format!("{ambient}\n{user_text}"),
            None => user_text,
        };
        self.history.push(ChatMessage::user(content));

        if let Err(e) = self.stt.mute().await {
            tracing::warn!(error = %e, "no se pudo silenciar el micrófono, sigo igual");
        }

        // history[0] es el bloque estático (prompt base + memorias, cacheado)
        // y history[1] el dinámico (fecha/hora de este turno).
        self.history[0] = ChatMessage::system(self.static_system_content().await);
        self.history[1] = ChatMessage::system(format!(
            "Contexto actual: hoy es {} (hora local).",
            system_info::fecha_hora_es()
        ));

        let result = if self.registry.is_empty() {
            self.plain_speaking_turn().await
        } else {
            let mut ctx = TurnContext {
                llm: &self.llm,
                tts: &self.tts,
                player: &mut self.player,
                registry: &self.registry,
                config: &self.config,
            };
            agent::run_agentic_turn(&mut ctx, &mut self.history).await
        };

        self.conclude_turn(result).await;
    }

    /// Turno clásico sin herramientas (agent.enabled: false).
    async fn plain_speaking_turn(&mut self) -> crate::errors::Result<AgentTurnResult> {
        let out = pipeline::run_speaking_turn(
            self.llm.clone(),
            self.tts.clone(),
            &mut self.player,
            &self.history,
            Arc::new(Vec::new()),
            &self.config.pipeline,
        )
        .await?;
        self.history.push(ChatMessage::assistant(out.spoken_text.clone()));
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
                if let Err(e) = self.stt.unmute().await {
                    tracing::warn!(error = %e, "no se pudo reactivar el micrófono");
                }
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
            self.cancel_pending(pending, "El usuario no respondió a tiempo; la acción fue cancelada.");
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
                    self.cancel_pending(pending, "El usuario cambió de tema; la acción fue cancelada.");
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
                    self.cancel_pending(pending, "El usuario cambió de tema; la acción fue cancelada.");
                    self.handle_utterance(text).await;
                }
            }
        }
    }

    /// El usuario aprobó: ejecutar la herramienta pendiente y retomar el
    /// loop agéntico donde quedó (las restantes pueden pedir sus propias
    /// confirmaciones).
    async fn approve_pending(&mut self, pending: PendingConfirmation) {
        if let Err(e) = self.stt.mute().await {
            tracing::warn!(error = %e, "no se pudo silenciar el micrófono, sigo igual");
        }
        let mut ctx = TurnContext {
            llm: &self.llm,
            tts: &self.tts,
            player: &mut self.player,
            registry: &self.registry,
            config: &self.config,
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
        if let Err(e) = self.stt.mute().await {
            tracing::warn!(error = %e, "no se pudo silenciar el micrófono, sigo igual");
        }
        let result = self.plain_speaking_turn().await;
        self.conclude_turn(result).await;
    }

    /// Cierre común de un turno completado: reabrir ventana de atención y
    /// micrófono, y recortar historial.
    async fn finish_turn(&mut self) {
        self.state = AgentState::Idle;

        // Incluso si el pipeline falló: el usuario querrá reintentar sin
        // repetir el nombre.
        self.gate.open_window();

        if let Err(e) = self.stt.unmute().await {
            tracing::warn!(error = %e, "no se pudo reactivar el micrófono");
        }

        self.trim_history();
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
                    content.push_str(
                        "\n(Si necesitas algo más antiguo, usa la herramienta recall.)",
                    );
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
