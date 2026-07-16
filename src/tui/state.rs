//! Estado visual compartido entre `Orchestrator` y el loop de renderizado de
//! la TUI. El orquestador solo informa transiciones discretas (qué está
//! pasando); toda la lógica de animación (envolventes, tiempos de
//! ataque/decaimiento) vive en `crate::tui`, no acá.

use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq)]
pub enum VisualState {
    /// Nadie habló todavía / turno terminado, esperando en reposo.
    Idle,
    /// Micrófono abierto, atento, sin voz detectada.
    Listening,
    /// VAD detectó voz del usuario (`SttEvent::VadStart`..`VadEnd`).
    UserSpeaking,
    /// Turno en curso, esperando la primera respuesta (LLM/TTS todavía sin
    /// audio que reproducir).
    Thinking,
    /// Jarvis está reproduciendo audio (entre `begin_speaking`/`end_speaking`).
    JarvisSpeaking,
    /// Esperando confirmación por voz de una herramienta riesgosa.
    AwaitingConfirmation,
    /// Estado transitorio ante un error recuperable (ej. worker reiniciado).
    Error(String),
}

/// Handle clonable para publicar cambios de estado desde `Orchestrator`.
/// Se crea siempre, aunque `ui.enabled` sea `false`: escribir en un
/// `watch::Sender` sin receptores activos es una operación local barata, así
/// que el orquestador puede llamar `set()` sin ramificar por configuración.
#[derive(Clone)]
pub struct UiState {
    tx: watch::Sender<VisualState>,
}

impl UiState {
    pub fn new() -> (Self, watch::Receiver<VisualState>) {
        let (tx, rx) = watch::channel(VisualState::Idle);
        (Self { tx }, rx)
    }

    pub fn set(&self, state: VisualState) {
        self.tx.send_replace(state);
    }
}
