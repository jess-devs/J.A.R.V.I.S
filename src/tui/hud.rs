//! Rótulo de estado en una esquina, estilo HUD (sin panel de logs: solo la
//! etiqueta del estado actual entre corchetes finos).

use std::borrow::Cow;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::state::{ToolCategory, VisualState};
use super::theme::Palette;

fn label(state: &VisualState) -> Cow<'static, str> {
    match state {
        VisualState::Idle => Cow::Borrowed("⟨ EN REPOSO ⟩"),
        VisualState::Listening => Cow::Borrowed("⟨ ESCUCHANDO ⟩"),
        VisualState::UserSpeaking => Cow::Borrowed("⟨ TE ESCUCHO ⟩"),
        VisualState::Thinking => Cow::Borrowed("⟨ PENSANDO… ⟩"),
        VisualState::JarvisSpeaking => Cow::Borrowed("⟨ HABLANDO ⟩"),
        VisualState::AwaitingConfirmation => Cow::Borrowed("⟨ ESPERANDO CONFIRMACIÓN ⟩"),
        VisualState::ToolRunning(category) => Cow::Borrowed(tool_label(*category)),
        VisualState::Error(msg) => Cow::Owned(format!("⟨ ERROR: {msg} ⟩")),
    }
}

fn tool_label(category: ToolCategory) -> &'static str {
    match category {
        ToolCategory::Web => "⟨ BUSCANDO EN LA WEB ⟩",
        ToolCategory::Media => "⟨ REPRODUCIENDO ⟩",
        ToolCategory::Memory => "⟨ REVISANDO MEMORIA ⟩",
        ToolCategory::System => "⟨ TRABAJANDO ⟩",
        ToolCategory::Other => "⟨ PENSANDO… ⟩",
    }
}

fn color(state: &VisualState, palette: &Palette) -> ratatui::style::Color {
    match state {
        VisualState::Idle => palette.idle,
        VisualState::Listening => palette.listening,
        VisualState::UserSpeaking => palette.user_speaking,
        VisualState::Thinking => palette.thinking,
        VisualState::JarvisSpeaking => palette.jarvis_speaking,
        VisualState::AwaitingConfirmation => palette.awaiting_confirmation,
        VisualState::ToolRunning(ToolCategory::Other) => palette.thinking,
        VisualState::ToolRunning(_) => palette.tool,
        VisualState::Error(_) => palette.error,
    }
}

/// `area` es la franja exacta donde va el rótulo (ver el layout de dos
/// filas en `mod.rs`: HUD arriba, onda abajo ocupando el resto).
pub fn render_label(frame: &mut Frame, area: Rect, state: &VisualState, palette: &Palette) {
    let text = label(state);
    let style = Style::default()
        .fg(color(state, palette))
        .add_modifier(Modifier::BOLD);
    let paragraph = Paragraph::new(Line::from(text).style(style));
    frame.render_widget(paragraph, area);
}
