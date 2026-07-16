//! Rótulo de estado en una esquina, estilo HUD (sin panel de logs: solo la
//! etiqueta del estado actual entre corchetes finos).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::state::VisualState;
use super::theme::Palette;

fn label(state: &VisualState) -> String {
    match state {
        VisualState::Idle => "⟨ EN REPOSO ⟩".to_string(),
        VisualState::Listening => "⟨ ESCUCHANDO ⟩".to_string(),
        VisualState::UserSpeaking => "⟨ TE ESCUCHO ⟩".to_string(),
        VisualState::Thinking => "⟨ PENSANDO… ⟩".to_string(),
        VisualState::JarvisSpeaking => "⟨ HABLANDO ⟩".to_string(),
        VisualState::AwaitingConfirmation => "⟨ ESPERANDO CONFIRMACIÓN ⟩".to_string(),
        VisualState::Error(msg) => format!("⟨ ERROR: {msg} ⟩"),
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
