//! Onda central estilo Siri/Alexa: una franja horizontal con una curva
//! animada que fluye. Reemplaza el diseño anterior de anillos concéntricos
//! (rechazado — se veía como ruido). Cada estado tiene su propia forma para
//! que se distingan de un vistazo:
//!
//! - `Idle`/`Listening`: onda calma, respira en amplitud.
//! - `UserSpeaking`: barras irregulares con picos marcados (como un
//!   waveform de mensaje de voz), reactivas a la envolvente del usuario.
//! - `JarvisSpeaking`: curva suave con "glow" (una copia más ancha y tenue
//!   detrás), reactiva al nivel real de audio.
//! - `Thinking`: un pulso que barre de un lado a otro sobre una base plana
//!   (efecto "escáner", distinto de hablar/escuchar).
//! - `AwaitingConfirmation`: línea plana con una respiración lenta.
//! - `Error`: línea plana que parpadea.

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Context, Line};
use ratatui::Frame;

use super::state::VisualState;
use super::theme::{self, Palette};

const COLUMNS: f64 = 140.0;
const X_BOUNDS: [f64; 2] = [0.0, COLUMNS];
const Y_BOUNDS: [f64; 2] = [-50.0, 50.0];

/// Hash determinístico en [0.0, 1.0) — sirve para que cada barra tenga su
/// propia variación sin depender de un RNG externo (mismo truco que el
/// clásico "hash" de shaders GLSL).
fn hash01(seed: f64) -> f64 {
    let s = (seed * 12.9898).sin() * 43_758.547;
    s - s.floor()
}

fn polyline(ctx: &mut Context, points: &[(f64, f64)], color: Color) {
    for pair in points.windows(2) {
        let (x1, y1) = pair[0];
        let (x2, y2) = pair[1];
        ctx.draw(&Line {
            x1,
            y1,
            x2,
            y2,
            color,
        });
    }
}

/// Onda calma que fluye de derecha a izquierda, con la amplitud
/// "respirando" lentamente encima. Usada en `Idle`/`Listening`.
fn flowing_breath(ctx: &mut Context, elapsed: f64, base_amp: f64, breathing_speed: f64, color: Color) {
    let amp = base_amp * (0.55 + 0.45 * (elapsed * breathing_speed).sin());
    let steps = 90;
    let points: Vec<(f64, f64)> = (0..=steps)
        .map(|i| {
            let x = COLUMNS * i as f64 / steps as f64;
            let y = amp * (0.09 * x - elapsed * 1.1).sin();
            (x, y)
        })
        .collect();
    polyline(ctx, &points, color);
}

/// Barras irregulares reactivas a `envelope` (0.0-1.0) — como un waveform de
/// mensaje de voz. Usada en `UserSpeaking`.
fn voice_bars(ctx: &mut Context, elapsed: f64, envelope: f64, color: Color) {
    let bar_count = 32;
    let spacing = COLUMNS / bar_count as f64;
    // Cambia de "semilla" ~10 veces por segundo: shimmer vivo sin que sea
    // puro parpadeo aleatorio cuadro a cuadro.
    let tick = (elapsed * 10.0).floor();
    for i in 0..bar_count {
        let x = spacing * i as f64 + spacing / 2.0;
        let n = hash01(i as f64 * 3.7 + tick * 91.3);
        let height = envelope * 46.0 * (0.25 + 0.75 * n);
        if height < 0.5 {
            continue;
        }
        ctx.draw(&Line {
            x1: x,
            y1: -height,
            x2: x,
            y2: height,
            color,
        });
    }
}

/// Curva suave con un "glow" detrás (copia más ancha y tenue de la misma
/// forma). Reactiva a `level` (nivel real de audio del TTS, 0.0-1.0). Usada
/// en `JarvisSpeaking`.
fn glowing_curve(ctx: &mut Context, elapsed: f64, level: f64, color: Color) {
    let amp = 6.0 + level * 40.0;
    let steps = 110;
    let shape = |mul: f64, phase: f64| -> Vec<(f64, f64)> {
        (0..=steps)
            .map(|i| {
                let x = COLUMNS * i as f64 / steps as f64;
                let y = amp * mul * (0.12 * x - elapsed * 2.6 + phase).sin();
                (x, y)
            })
            .collect()
    };
    // Glow: mismo trazo, un poco más grande y bien tenue, detrás del
    // principal — dos pasadas alcanzan para leerse como un brillo suave.
    polyline(ctx, &shape(1.35, 0.0), theme::dim(color, 0.35));
    polyline(ctx, &shape(1.0, 0.0), color);
}

/// Un pulso que recorre la franja de un lado a otro sobre una base plana:
/// efecto "escáner", para distinguir claramente "procesando" de "hablando".
fn scanning_pulse(ctx: &mut Context, elapsed: f64, color: Color, baseline: Color) {
    ctx.draw(&Line {
        x1: 0.0,
        y1: 0.0,
        x2: COLUMNS,
        y2: 0.0,
        color: baseline,
    });
    // Onda triangular en [0,1] para un barrido ida y vuelta (no un diente de
    // sierra que salte).
    let period = 1.6;
    let phase = (elapsed / period).fract();
    let triangle = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
    let x = COLUMNS * triangle;
    let width = 10.0;
    let steps = 16;
    let points: Vec<(f64, f64)> = (0..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64 - 0.5;
            let px = x + t * width;
            let py = 20.0 * (1.0 - (2.0 * t).abs()) * (elapsed * 6.0 + t).cos();
            (px.clamp(0.0, COLUMNS), py)
        })
        .collect();
    polyline(ctx, &points, color);
}

/// Línea plana con una respiración lenta en amplitud — `AwaitingConfirmation`.
fn slow_pulse_flat(ctx: &mut Context, elapsed: f64, color: Color) {
    let amp = 3.0 + 9.0 * (0.5 + 0.5 * (elapsed * 0.8).sin());
    let points: Vec<(f64, f64)> = (0..=60)
        .map(|i| {
            let x = COLUMNS * i as f64 / 60.0;
            (x, amp * (0.06 * x).sin())
        })
        .collect();
    polyline(ctx, &points, color);
}

/// Línea plana que parpadea rápido — `Error`.
fn flashing_flat(ctx: &mut Context, elapsed: f64, color: Color) {
    if (elapsed * 6.0).fract() < 0.5 {
        ctx.draw(&Line {
            x1: 0.0,
            y1: 0.0,
            x2: COLUMNS,
            y2: 0.0,
            color,
        });
    }
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &VisualState,
    level: f32,
    elapsed: f64,
    palette: &Palette,
    marker: Marker,
) {
    let level = level.clamp(0.0, 1.0) as f64;
    let state = state.clone();
    let palette = *palette;

    let canvas = Canvas::default()
        .marker(marker)
        .x_bounds(X_BOUNDS)
        .y_bounds(Y_BOUNDS)
        .paint(move |ctx| match &state {
            VisualState::Idle => flowing_breath(ctx, elapsed, 8.0, 1.55, palette.idle),
            VisualState::Listening => flowing_breath(ctx, elapsed, 10.0, 2.1, palette.listening),
            VisualState::UserSpeaking => voice_bars(ctx, elapsed, level, palette.user_speaking),
            VisualState::JarvisSpeaking => glowing_curve(ctx, elapsed, level, palette.jarvis_speaking),
            VisualState::Thinking => {
                scanning_pulse(ctx, elapsed, palette.thinking, palette.baseline)
            }
            VisualState::AwaitingConfirmation => {
                slow_pulse_flat(ctx, elapsed, palette.awaiting_confirmation)
            }
            VisualState::Error(_) => flashing_flat(ctx, elapsed, palette.error),
        });

    frame.render_widget(canvas, area);
}
