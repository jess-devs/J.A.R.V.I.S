//! Modo texto (`--text-mode`, ver README.md): lee pedidos por
//! stdin en vez de STT. v1 solo saltea el reconocimiento de voz — Jarvis
//! sigue *hablando* las respuestas por TTS y sigue pidiendo confirmación
//! de voz-por-texto para acciones de riesgo. Útil para debugging o
//! ambientes donde hablarle a Jarvis no es práctico.
//!
//! Implementado como un runner liviano y aparte de `Orchestrator`, en vez
//! de threadear `Option<SttWorker>` por todo `orchestrator.rs`/`turn.rs`:
//! reusa los mismos bloques ya existentes y probados
//! (`agent::run_agentic_turn`, `ToolRegistry`, `agent::confirm::interpret`)
//! sin tocar ni arriesgar el camino caliente de voz. Un modo texto v2
//! totalmente headless (sin TTS/audio) queda para más adelante.

use std::io::{BufRead, Write};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::agent::confirm::{self, CodeDecision, ConfirmDecision};
use crate::agent::{self, AgentTurnResult, PendingConfirmation, TurnContext};
use crate::audio::AudioPlayer;
use crate::config::Config;
use crate::echo_gate::EchoGate;
use crate::errors::Result;
use crate::llm::{self, ChatMessage, LlmProvider};
use crate::memory::MemoryStore;
use crate::reminders::{self, ReminderStore};
use crate::tools::scripted_store::ScriptedToolStore;
use crate::tools::{system_info, ToolRegistry};
use crate::tts::{self, TtsProvider};

struct TextSession {
    config: Config,
    llm: Arc<dyn LlmProvider>,
    tts: Arc<dyn TtsProvider>,
    player: AudioPlayer,
    registry: ToolRegistry,
    memory: Arc<MemoryStore>,
    echo_gate: Arc<Mutex<EchoGate>>,
    history: Vec<ChatMessage>,
    system_static_cache: Option<(u64, String)>,
    /// Recordatorios vencidos que anuncia `reminders::run_poller` en una
    /// tarea aparte (mismo `ReminderStore` que usan `create_reminder`/
    /// `list_reminders`/`cancel_reminder` en `registry`).
    reminder_rx: mpsc::Receiver<reminders::DueReminder>,
}

impl TextSession {
    async fn new(config: Config) -> Result<Self> {
        let llm = llm::build_provider(&config)?;
        let tts = tts::build_provider(&config).await?;
        let player = AudioPlayer::new(
            config.audio.output_device.as_deref(),
            config.audio.volume,
            config.audio.drain_timeout_secs,
        )?;
        let memory = Arc::new(MemoryStore::open(&config.agent.memory.db_path)?);
        let reminder_store = Arc::new(ReminderStore::open(&config.agent.reminders.db_path)?);
        let scripted_store = Arc::new(ScriptedToolStore::open(
            &config.agent.scripted_tools.db_path,
        )?);
        let registry = ToolRegistry::build(
            &config.agent,
            memory.clone(),
            reminder_store.clone(),
            scripted_store,
            None, // sin música de bienvenida en modo texto
            Arc::new(AtomicBool::new(false)),
            &config.mcp,
        )
        .await;
        let echo_gate = Arc::new(Mutex::new(EchoGate::new(
            config.barge_in.echo_guard.clone(),
        )));
        let (reminder_tx, reminder_rx) = mpsc::channel(16);
        tokio::spawn(reminders::run_poller(
            reminder_store,
            reminder_tx,
            Duration::from_secs(config.agent.reminders.poll_interval_secs),
        ));

        let history = vec![
            ChatMessage::system(config.llm.system_prompt.clone()),
            ChatMessage::system(String::new()),
        ];

        Ok(Self {
            config,
            llm,
            tts,
            player,
            registry,
            memory,
            echo_gate,
            history,
            system_static_cache: None,
            reminder_rx,
        })
    }

    /// Mismo contenido que `Orchestrator::static_system_content`: prompt
    /// base + memorias recientes, cacheado por generación del `MemoryStore`.
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

    /// Arranca un turno nuevo con `user_text` como pedido. Devuelve la
    /// confirmación pendiente, si el LLM pidió una acción de riesgo.
    async fn run_new_turn(&mut self, user_text: String) -> Option<PendingConfirmation> {
        self.history.push(ChatMessage::user(user_text));

        let static_content = self.static_system_content().await;
        self.history[0] = ChatMessage::system(static_content);
        self.history[1] = ChatMessage::system(format!(
            "Contexto actual: hoy es {} (hora local).",
            system_info::fecha_hora_es()
        ));

        // Construcción directa (no vía método `&mut self`): así el borrow
        // checker ve que `ctx` solo toma prestados llm/tts/player/registry/
        // config, y `&mut self.history` de la línea siguiente sigue libre
        // (un helper `fn turn_context(&mut self)` bloquearía todo `self`).
        let (_pause_tx, pause_rx) = watch::channel(false);
        let mut ctx = TurnContext {
            llm: &self.llm,
            tts: &self.tts,
            player: &mut self.player,
            registry: &self.registry,
            config: &self.config,
            cancel: CancellationToken::new(),
            echo_gate: self.echo_gate.clone(),
            pause_rx,
        };
        match agent::run_agentic_turn(&mut ctx, &mut self.history).await {
            Ok(result) => self.report_result(result),
            Err(e) => {
                eprintln!("[error] {e}");
                None
            }
        }
    }

    /// Retoma un turno tras resolver una confirmación afirmativa.
    async fn approve_pending(
        &mut self,
        pending: PendingConfirmation,
    ) -> Option<PendingConfirmation> {
        let (_pause_tx, pause_rx) = watch::channel(false);
        let mut ctx = TurnContext {
            llm: &self.llm,
            tts: &self.tts,
            player: &mut self.player,
            registry: &self.registry,
            config: &self.config,
            cancel: CancellationToken::new(),
            echo_gate: self.echo_gate.clone(),
            pause_rx,
        };
        match agent::resume_agentic_turn(&mut ctx, &mut self.history, pending).await {
            Ok(result) => self.report_result(result),
            Err(e) => {
                eprintln!("[error] {e}");
                None
            }
        }
    }

    /// Cancela una confirmación pendiente: dos tool_result "cancelado" (uno
    /// para la call pendiente, uno por cada call encolada detrás), igual
    /// que `Orchestrator::cancel_pending` — ver ese comentario para el
    /// porqué (todo tool call necesita SIEMPRE su tool_result).
    fn cancel_pending(&mut self, pending: PendingConfirmation, reason: &str) {
        crate::audit::record_confirmation_denied(&pending.call.name, pending.requires_code, reason);
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

    fn report_result(&self, result: AgentTurnResult) -> Option<PendingConfirmation> {
        match result {
            AgentTurnResult::Completed { .. } => None,
            AgentTurnResult::Interrupted { spoken_so_far } => {
                if !spoken_so_far.trim().is_empty() {
                    println!("(interrumpido) {spoken_so_far}");
                }
                None
            }
            AgentTurnResult::NeedsConfirmation(pending) => {
                println!("[jarvis] {}", pending.spoken_question);
                Some(pending)
            }
        }
    }
}

/// Loop principal del modo texto: lee líneas de stdin (en un hilo aparte,
/// bloqueante) y las alimenta al mismo camino de manejo por-turno que usa
/// la voz. `speak()` ya imprime nada por sí solo — acá se imprime aparte
/// para poder *leer* la respuesta además de escucharla.
pub async fn run(config: Config) -> Result<()> {
    println!("Jarvis (modo texto). Escribí un pedido y Enter. Ctrl+C para salir.");

    let mut session = TextSession::new(config).await?;
    let mut pending: Option<PendingConfirmation> = None;

    let (line_tx, mut line_rx) = mpsc::channel::<String>(8);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(text) => {
                    if line_tx.blocking_send(text).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    print_prompt();
    loop {
        tokio::select! {
            line = line_rx.recv() => {
                let Some(raw) = line else { break };
                let text = raw.trim().to_string();
                if text.is_empty() {
                    print_prompt();
                    continue;
                }

                pending = match pending.take() {
                    Some(p) => handle_confirmation_reply(&mut session, p, text).await,
                    None => session.run_new_turn(text).await,
                };
                print_prompt();
            }
            due = session.reminder_rx.recv() => {
                let Some(due) = due else { continue };
                let text = format!("Recordatorio: {}", due.text);
                println!("\n[{text}]");
                agent::speak(&session.tts, &mut session.player, &session.echo_gate, &text).await;
                print_prompt();
            }
        }
    }

    session.tts.shutdown().await;
    Ok(())
}

fn print_prompt() {
    print!("> ");
    let _ = std::io::stdout().flush();
}

/// Interpreta `text` como respuesta a `pending` (sí/no o código de
/// aceptación, según `pending.requires_code`) — misma lógica de texto
/// puro que ya usa la voz (`agent::confirm`), sin cambios.
async fn handle_confirmation_reply(
    session: &mut TextSession,
    pending: PendingConfirmation,
    text: String,
) -> Option<PendingConfirmation> {
    if pending.requires_code {
        match confirm::interpret_code(&text, &session.config.agent) {
            CodeDecision::Correct => session.approve_pending(pending).await,
            CodeDecision::Wrong => {
                session.cancel_pending(
                    pending,
                    "El usuario dio un código de aceptación incorrecto; la acción fue cancelada.",
                );
                println!("[jarvis] Código incorrecto. Acción cancelada.");
                None
            }
            CodeDecision::Cancelled => {
                session.cancel_pending(pending, "El usuario canceló la acción.");
                println!("[jarvis] Como desees.");
                None
            }
            // Acá no hay STT que falle: lo que se tipeó es exactamente lo que
            // se quiso decir. Así que un código equivocado sigue cancelando al
            // primer intento (`code_max_attempts` solo compensa errores de
            // transcripción, que en texto no existen), pero una respuesta que
            // no contiene ningún número es un tipeo a medias y se repregunta.
            CodeDecision::Unintelligible => {
                println!(
                    "[jarvis] No entendí. Escribe el código de aceptación, o 'no' para cancelar."
                );
                Some(pending)
            }
            CodeDecision::Unrelated => {
                session.cancel_pending(
                    pending,
                    "El usuario cambió de tema; la acción fue cancelada.",
                );
                session.run_new_turn(text).await
            }
        }
    } else {
        match confirm::interpret(&text, &session.config.agent) {
            ConfirmDecision::Yes => session.approve_pending(pending).await,
            ConfirmDecision::No => {
                session.cancel_pending(pending, "El usuario canceló la acción.");
                println!("[jarvis] Como desees.");
                None
            }
            ConfirmDecision::Unintelligible => {
                println!("[jarvis] No entendí. Responde sí o no.");
                Some(pending)
            }
            ConfirmDecision::Unrelated => {
                session.cancel_pending(
                    pending,
                    "El usuario cambió de tema; la acción fue cancelada.",
                );
                session.run_new_turn(text).await
            }
        }
    }
}
