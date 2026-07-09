//! Loop principal: espera una transcripción, silencia el micrófono mientras
//! Jarvis responde, corre el pipeline de streaming y reactiva la escucha.

use std::sync::Arc;

use crate::audio::AudioPlayer;
use crate::config::Config;
use crate::errors::{Result, WorkerError};
use crate::llm::{self, ChatMessage, LlmProvider, Role};
use crate::pipeline;
use crate::stt::{SttEvent, SttWorker};
use crate::tts::{self, TtsProvider};
use crate::wake::{AttentionGate, GateDecision};

pub struct Orchestrator {
    config: Config,
    stt: SttWorker,
    llm: Arc<dyn LlmProvider>,
    tts: Arc<dyn TtsProvider>,
    player: AudioPlayer,
    history: Vec<ChatMessage>,
    gate: AttentionGate,
}

impl Orchestrator {
    pub async fn new(config: Config) -> Result<Self> {
        let stt = SttWorker::spawn(&config.workers, &config.stt).await?;
        let llm_provider = llm::build_provider(&config)?;
        let tts_provider = tts::build_provider(&config).await?;
        let player = AudioPlayer::new(config.audio.output_device.as_deref(), config.audio.volume)?;

        let history = vec![ChatMessage {
            role: Role::System,
            content: config.llm.system_prompt.clone(),
        }];
        let gate = AttentionGate::new(config.wake.clone());

        Ok(Self {
            config,
            stt,
            llm: llm_provider,
            tts: tts_provider,
            player,
            history,
            gate,
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
                    match self.gate.decide(&text) {
                        GateDecision::Ignore => {
                            tracing::info!(text = %text, "ignorado: sin wake word y fuera de ventana");
                            self.gate.push_ambient(text);
                        }
                        GateDecision::Respond => {
                            tracing::info!(text = %text, "usuario dijo");
                            self.handle_utterance(text).await;
                        }
                    }
                }
                SttEvent::WorkerDied => {
                    return Err(WorkerError::Crashed(None).into());
                }
            }
        }
        Ok(())
    }

    async fn handle_utterance(&mut self, user_text: String) {
        let content = match self.gate.take_ambient_context() {
            Some(ambient) => format!("{ambient}\n{user_text}"),
            None => user_text,
        };
        self.history.push(ChatMessage {
            role: Role::User,
            content,
        });

        if let Err(e) = self.stt.mute().await {
            tracing::warn!(error = %e, "no se pudo silenciar el micrófono, sigo igual");
        }

        let result = pipeline::run_streaming_response(
            self.llm.clone(),
            self.tts.clone(),
            &mut self.player,
            &self.history,
            &self.config.pipeline,
        )
        .await;

        match result {
            Ok(reply) => {
                tracing::info!(reply = %reply, "Jarvis respondió");
                self.history.push(ChatMessage {
                    role: Role::Assistant,
                    content: reply,
                });
            }
            Err(e) => tracing::error!(error = %e, "fallo generando la respuesta"),
        }

        // Incluso si el pipeline falló: el usuario querrá reintentar sin
        // repetir el nombre.
        self.gate.open_window();

        if let Err(e) = self.stt.unmute().await {
            tracing::warn!(error = %e, "no se pudo reactivar el micrófono");
        }

        self.trim_history();
    }

    /// Conserva el system prompt (siempre el primer mensaje) + los últimos
    /// `max_history_messages` mensajes de la conversación.
    fn trim_history(&mut self) {
        let max = self.config.llm.max_history_messages;
        if self.history.len() > max + 1 {
            let system = self.history[0].clone();
            let tail_start = self.history.len() - max;
            let mut trimmed = Vec::with_capacity(max + 1);
            trimmed.push(system);
            trimmed.extend_from_slice(&self.history[tail_start..]);
            self.history = trimmed;
        }
    }

    pub async fn shutdown(&self) {
        self.stt.shutdown().await;
        self.tts.shutdown().await;
    }
}
