//! Paleta de colores de la onda, con variante truecolor (RGB de 24 bits) y
//! variante ANSI de 16 colores para terminales/consolas sin soporte
//! truecolor (ver `config.ui.truecolor`).

use ratatui::style::Color;

use crate::config::UiConfig;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub idle: Color,
    pub listening: Color,
    pub user_speaking: Color,
    pub thinking: Color,
    pub jarvis_speaking: Color,
    pub awaiting_confirmation: Color,
    pub error: Color,
    /// Línea base/tenue de fondo (ej. la base plana detrás del scanner de
    /// `Thinking`).
    pub baseline: Color,
}

impl Palette {
    pub fn from_config(config: &UiConfig) -> Self {
        if config.truecolor {
            Self {
                idle: Color::Rgb(50, 110, 130),
                listening: Color::Rgb(70, 160, 190),
                user_speaking: Color::Rgb(110, 235, 245),
                thinking: Color::Rgb(160, 110, 235),
                jarvis_speaking: Color::Rgb(255, 180, 80),
                awaiting_confirmation: Color::Rgb(235, 165, 45),
                error: Color::Rgb(225, 70, 70),
                baseline: Color::Rgb(35, 55, 65),
            }
        } else {
            Self {
                idle: Color::Blue,
                listening: Color::Cyan,
                user_speaking: Color::LightCyan,
                thinking: Color::Magenta,
                jarvis_speaking: Color::Yellow,
                awaiting_confirmation: Color::LightYellow,
                error: Color::Red,
                baseline: Color::DarkGray,
            }
        }
    }
}

/// Atenúa un color (para el efecto de "glow": una copia más ancha y más
/// tenue de la misma curva, detrás de la curva principal). Sin variante RGB
/// (paleta ANSI de 16 colores) no hay forma fiable de atenuar, así que se
/// devuelve tal cual.
pub fn dim(color: Color, factor: f32) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * factor) as u8,
            (g as f32 * factor) as u8,
            (b as f32 * factor) as u8,
        ),
        other => other,
    }
}
