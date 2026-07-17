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
    /// Ejecutando una herramienta (ver `ToolCategory`) — reemplaza el
    /// genérico `Thinking` mientras esa herramienta puntual está corriendo.
    ToolRunning(ToolCategory),
    /// Estado transitorio ante un error recuperable (ej. worker reiniciado).
    Error(String),
}

/// Categoría visual de una herramienta, para elegir qué mini-animación
/// mostrar mientras corre (ver `crate::tui::wave`). No es una taxonomía de
/// negocio, solo agrupa por "qué pinta tiene" — varias herramientas sin
/// relación entre sí caen en la misma categoría si visualmente deben verse
/// igual (ej. todo lo que es "trabajo de sistema").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    /// web_search, fetch_page, translate.
    Web,
    /// media_control, stop_music.
    Media,
    /// remember, recall, forget, create_reminder, list_reminders, cancel_reminder.
    Memory,
    /// get_datetime, system_status, list_processes, open_app, open_url, close_app,
    /// find_files, open_file, run_powershell, get_volume, set_volume,
    /// take_screenshot, mouse_move, mouse_click, click_at.
    System,
    /// Cualquier otra (tools dinámicas/scripted, o nombre no reconocido):
    /// misma animación que `Thinking`.
    Other,
}

impl ToolCategory {
    pub fn from_tool_name(name: &str) -> Self {
        match name {
            "web_search" | "fetch_page" | "translate" => Self::Web,
            "media_control" | "stop_music" => Self::Media,
            "remember" | "recall" | "forget" | "create_reminder" | "list_reminders"
            | "cancel_reminder" => Self::Memory,
            "get_datetime" | "system_status" | "list_processes" | "open_app" | "open_url"
            | "close_app" | "find_files" | "open_file" | "run_powershell" | "get_volume"
            | "set_volume" | "take_screenshot" | "mouse_move" | "mouse_click" | "click_at" => {
                Self::System
            }
            _ => Self::Other,
        }
    }
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
