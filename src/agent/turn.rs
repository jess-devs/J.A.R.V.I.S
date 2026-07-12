//! El loop agéntico de un turno de conversación.
//!
//! Cada iteración: una pasada de streaming por el LLM (cuyo texto ya se
//! habla en vivo) que puede pedir tool calls. Las herramientas `Safe` se
//! ejecutan directo; las que requieren aprobación interrumpen el loop
//! devolviendo `NeedsConfirmation` — el orquestador pregunta por voz y
//! retoma con `resume_agentic_turn`. El micrófono permanece muteado durante
//! todo el loop; solo el orquestador lo reabre.

use std::sync::{Arc, Mutex};

use rand::seq::SliceRandom;
use tokio_util::sync::CancellationToken;

use crate::audio::AudioPlayer;
use crate::config::Config;
use crate::echo_gate::EchoGate;
use crate::errors::{JarvisError, ToolError};
use crate::llm::{ChatMessage, LlmProvider, ToolCallRequest};
use crate::pipeline::run_speaking_turn;
use crate::tools::{RiskLevel, ToolRegistry};
use crate::tts::TtsProvider;

use super::speak;

/// Referencias que necesita el loop; se reconstruye barato en cada llamada.
pub struct TurnContext<'a> {
    pub llm: &'a Arc<dyn LlmProvider>,
    pub tts: &'a Arc<dyn TtsProvider>,
    pub player: &'a mut AudioPlayer,
    pub registry: &'a ToolRegistry,
    pub config: &'a Config,
    /// Se dispara para cortar el turno a mitad de respuesta (barge-in, o el
    /// hook de prueba `JARVIS_TEST_CANCEL`). Un token nuevo por turno.
    pub cancel: CancellationToken,
    /// Registra las frases que Jarvis efectivamente dice, para descartar
    /// como eco propio transcripciones que lleguen mientras habla.
    pub echo_gate: Arc<Mutex<EchoGate>>,
}

/// Una herramienta esperando aprobación por voz del usuario.
#[derive(Debug)]
pub struct PendingConfirmation {
    pub call: ToolCallRequest,
    /// Pregunta ya redactada para hablar ("Voy a cerrar Chrome. ¿Confirma?").
    pub spoken_question: String,
    /// true = nivel `Code`: exige el código de aceptación, no un simple sí.
    pub requires_code: bool,
    /// Tool calls del mismo turno aún sin ejecutar (pueden requerir sus
    /// propias confirmaciones al retomar).
    pub remaining_calls: Vec<ToolCallRequest>,
    /// Iteraciones ya consumidas, para retomar el loop donde quedó.
    pub iterations_used: usize,
}

#[derive(Debug)]
pub enum AgentTurnResult {
    /// El turno terminó y la respuesta final ya fue hablada y agregada al
    /// historial.
    Completed { final_text: String },
    NeedsConfirmation(PendingConfirmation),
    /// Se canceló a mitad de respuesta (barge-in, o `JARVIS_TEST_CANCEL`).
    /// `spoken_so_far` es solo lo que alcanzó a sonar; el orquestador es
    /// quien decide qué guardar en el historial.
    Interrupted { spoken_so_far: String },
}

pub async fn run_agentic_turn(
    ctx: &mut TurnContext<'_>,
    history: &mut Vec<ChatMessage>,
) -> Result<AgentTurnResult, JarvisError> {
    turn_loop(ctx, history, 0, Vec::new()).await
}

/// Retoma el turno después de que el usuario aprobó la herramienta pendiente:
/// la ejecuta y sigue con las restantes y el resto del loop.
pub async fn resume_agentic_turn(
    ctx: &mut TurnContext<'_>,
    history: &mut Vec<ChatMessage>,
    pending: PendingConfirmation,
) -> Result<AgentTurnResult, JarvisError> {
    execute_and_record(ctx.registry, ctx.config, history, &pending.call).await;
    turn_loop(ctx, history, pending.iterations_used, pending.remaining_calls).await
}

async fn turn_loop(
    ctx: &mut TurnContext<'_>,
    history: &mut Vec<ChatMessage>,
    start_iteration: usize,
    initial_queue: Vec<ToolCallRequest>,
) -> Result<AgentTurnResult, JarvisError> {
    let specs = ctx.registry.specs();
    let max_iterations = ctx.config.agent.max_iterations.max(1);
    let mut iterations = start_iteration;
    let mut queue = initial_queue;

    loop {
        if let Some(pending) = process_queue(ctx, history, &mut queue, iterations).await? {
            return Ok(AgentTurnResult::NeedsConfirmation(pending));
        }
        if iterations >= max_iterations {
            break;
        }

        let out = run_speaking_turn(
            ctx.llm.clone(),
            ctx.tts.clone(),
            ctx.player,
            history,
            specs.clone(),
            &ctx.config.pipeline,
            ctx.cancel.clone(),
            ctx.echo_gate.clone(),
        )
        .await?;
        iterations += 1;

        if out.interrupted {
            // No se empuja nada al historial por esta pasada: si el modelo
            // ya había pedido herramientas en una pasada ANTERIOR de este
            // mismo turno, esas ya quedaron completas (assistant_with_tools
            // + sus tool_result) antes de llegar acá. Esta pasada, la que
            // se cortó, no llegó a pedir nada todavía.
            return Ok(AgentTurnResult::Interrupted {
                spoken_so_far: out.spoken_text,
            });
        }

        if out.tool_calls.is_empty() {
            history.push(ChatMessage::assistant(out.spoken_text.clone()));
            return Ok(AgentTurnResult::Completed {
                final_text: out.spoken_text,
            });
        }

        tracing::info!(
            calls = ?out.tool_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            iteration = iterations,
            "el modelo pidió herramientas"
        );
        history.push(ChatMessage::assistant_with_tools(
            out.spoken_text.clone(),
            out.tool_calls.clone(),
        ));

        // Si el modelo pidió herramientas sin decir nada en su primera
        // pasada, Jarvis avisa con una frase enlatada para no dejar un
        // silencio muerto mientras ejecuta.
        if iterations == 1 && out.spoken_text.trim().is_empty() {
            let filler = ctx
                .config
                .agent
                .filler_phrases
                .choose(&mut rand::thread_rng())
                .cloned();
            if let Some(filler) = filler {
                speak(ctx.tts, ctx.player, &filler).await;
            }
        }

        queue = out.tool_calls;
    }

    // Límite de iteraciones agotado: una última pasada SIN herramientas para
    // forzar una respuesta hablada con lo que se tenga.
    history.push(ChatMessage::user(
        "(Sistema: alcanzaste el límite de herramientas de este turno. \
         Responde ahora al usuario con la información que ya tienes.)",
    ));
    let out = run_speaking_turn(
        ctx.llm.clone(),
        ctx.tts.clone(),
        ctx.player,
        history,
        Arc::new(Vec::new()),
        &ctx.config.pipeline,
        ctx.cancel.clone(),
        ctx.echo_gate.clone(),
    )
    .await?;
    if out.interrupted {
        return Ok(AgentTurnResult::Interrupted {
            spoken_so_far: out.spoken_text,
        });
    }
    history.push(ChatMessage::assistant(out.spoken_text.clone()));
    Ok(AgentTurnResult::Completed {
        final_text: out.spoken_text,
    })
}

/// Ejecuta en orden los tool calls encolados. Devuelve `Some` si encontró
/// uno que requiere aprobación por voz (con el resto en `remaining_calls`).
async fn process_queue(
    ctx: &mut TurnContext<'_>,
    history: &mut Vec<ChatMessage>,
    queue: &mut Vec<ToolCallRequest>,
    iterations_used: usize,
) -> Result<Option<PendingConfirmation>, JarvisError> {
    while !queue.is_empty() {
        let call = queue.remove(0);
        let Some(tool) = ctx.registry.get(&call.name) else {
            tracing::warn!(name = %call.name, "el modelo pidió una herramienta inexistente");
            history.push(ChatMessage::tool_result(
                &call.id,
                &call.name,
                format!("Error: no existe ninguna herramienta llamada '{}'.", call.name),
            ));
            continue;
        };

        match tool.assess_risk(&call.arguments) {
            RiskLevel::Safe => {
                execute_and_record(ctx.registry, ctx.config, history, &call).await;
            }
            risk => {
                let action = tool.describe_action(&call.arguments);
                let requires_code = risk == RiskLevel::Code;
                let spoken_question = if requires_code {
                    format!(
                        "Señor, la acción de {action} es de alto riesgo. Diga el código de \
                         aceptación de riesgos para proceder, o diga no para cancelar."
                    )
                } else {
                    format!("Voy a {action}. ¿Confirma, señor?")
                };
                return Ok(Some(PendingConfirmation {
                    call,
                    spoken_question,
                    requires_code,
                    remaining_calls: std::mem::take(queue),
                    iterations_used,
                }));
            }
        }
    }
    Ok(None)
}

/// Ejecuta una herramienta con timeout y agrega su resultado (o el error,
/// como texto para que el LLM se disculpe o reintente) al historial.
pub async fn execute_and_record(
    registry: &ToolRegistry,
    config: &Config,
    history: &mut Vec<ChatMessage>,
    call: &ToolCallRequest,
) {
    let timeout_secs = config.agent.tool_timeout_secs;
    let result = match registry.get(&call.name) {
        None => format!("Error: no existe ninguna herramienta llamada '{}'.", call.name),
        Some(tool) => {
            let fut = tool.execute(call.arguments.clone());
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
                Err(_) => format!("Error: {}", ToolError::Timeout(timeout_secs)),
                Ok(Err(e)) => format!("Error: {e}"),
                Ok(Ok(output)) => registry.truncate_result(output),
            }
        }
    };
    tracing::info!(tool = %call.name, args = %call.arguments, result = %result, "herramienta ejecutada");
    history.push(ChatMessage::tool_result(&call.id, &call.name, result));
}
