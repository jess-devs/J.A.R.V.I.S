//! Interfaz Ratatui "viva": un holograma central que respira en reposo y
//! reacciona con animaciones distintas al usuario y a Jarvis hablando. Se
//! activa con `config.ui.enabled` (ver `src/config.rs`); mientras está
//! apagada, todo el resto del programa se comporta exactamente igual que
//! antes (logs de `tracing` por consola).
//!
//! El orquestador solo publica el estado discreto (`VisualState`) a través
//! de `UiState`; toda la animación (envolventes, fases, colores) vive acá,
//! para no mezclar lógica de turnos con lógica de renderizado.

mod hud;
mod state;
mod theme;
mod wave;

pub use state::{ToolCategory, UiState, VisualState};

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::symbols::Marker;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::audio::PlaybackMeter;
use crate::config::{MarkerKind, UiConfig};
use crate::errors::Result;
use theme::Palette;

/// Suaviza `envelope` hacia `target` con una tasa de ataque (subiendo) o
/// liberación (bajando) distinta, para que la animación reaccione rápido
/// pero no caiga en seco.
fn smooth(envelope: f32, target: f32, attack: f32, release: f32) -> f32 {
    let rate = if target > envelope { attack } else { release };
    envelope + (target - envelope) * rate
}

/// El orquestador nunca reporta `JarvisSpeaking` directamente (ver
/// `VisualState`): se deriva acá cruzando el estado discreto reportado con
/// el nivel real de reproducción, con un margen (`grace`) para no parpadear
/// en los micro-cortes entre frases mientras Jarvis sintetiza la siguiente.
fn effective_state(reported: VisualState, jarvis_active: bool) -> VisualState {
    if matches!(reported, VisualState::Thinking) && jarvis_active {
        VisualState::JarvisSpeaking
    } else {
        reported
    }
}

/// Convierte dBFS crudo (`Orchestrator::mic_level_rx`) a un nivel 0.0-1.0
/// para la TUI. No depende del piso de energía calibrado por el motor
/// nativo (`energy_floor_dbfs`, específico de cada máquina/micrófono): es
/// una señal puramente visual, no de decisión, así que alcanza con un rango
/// fijo razonable para voz.
pub fn normalize_mic_level(dbfs: f32) -> f32 {
    const FLOOR_DBFS: f32 = -50.0;
    const RANGE_DB: f32 = 35.0;
    ((dbfs - FLOOR_DBFS) / RANGE_DB).clamp(0.0, 1.0)
}

/// Corre el loop de renderizado hasta que el usuario pide salir (`q`/Esc) o
/// falla la terminal. Instala/restaura la terminal (raw mode + alternate
/// screen) al entrar/salir; un panic mientras corre queda cubierto por el
/// panic hook que instala `ratatui::try_init` (restaura la terminal antes de
/// propagar al hook de `main.rs`, que a su vez limpia los workers Python).
pub async fn run(
    config: UiConfig,
    mut state_rx: watch::Receiver<VisualState>,
    jarvis_meter: PlaybackMeter,
    mut mic_level_rx: watch::Receiver<f32>,
    shutdown: CancellationToken,
) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let palette = Palette::from_config(&config);
    let marker = match config.marker {
        MarkerKind::Block => Marker::Block,
        MarkerKind::Braille => Marker::Braille,
    };

    let fps = config.fps.max(1);
    let frame_duration = Duration::from_millis(1000 / u64::from(fps));
    let mut ticker = tokio::time::interval(frame_duration);
    let start = Instant::now();

    // Envolvente del nivel real del micrófono (dBFS normalizado por
    // `normalize_mic_level`), suavizada con ataque rápido y
    // liberación un poco más lenta para que reaccione al volumen de la voz
    // sin verse nerviosa cuadro a cuadro. Fuera de `UserSpeaking` el objetivo
    // es 0.0, así que decae solo al terminar de hablar (no hace falta un
    // "gap de silencio" como con Jarvis: acá el límite lo da el propio
    // cambio de estado, `VadEnd` ya dispara `Listening` de inmediato).
    let mut user_envelope: f32 = 0.0;

    // `jarvis_meter.is_speaking()` viene del callback de audio en tiempo
    // real (ver `AudioPlayer::new`): es exactamente lo que se le está
    // mandando a la tarjeta de sonido en este instante, no un valor que se
    // encoló hace rato. Igual se le da un margen chico antes de "apagar" la
    // animación — no para compensar un desfasaje grande (ya no lo hay), sino
    // para no parpadear en los micro-cortes naturales entre frases mientras
    // Jarvis sintetiza la siguiente.
    let mut last_jarvis_active_at = Instant::now();
    const JARVIS_DEMOTE_GRACE: Duration = Duration::from_millis(150);
    // Envolvente suavizada del nivel de Jarvis: ataque rápido (reacciona ya
    // al primer chunk audible) y liberación lenta (no cae en seco al final
    // de una frase).
    let mut jarvis_envelope: f32 = 0.0;

    let outcome = loop {
        if shutdown.is_cancelled() {
            break Ok(());
        }
        if event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                // El raw mode desactiva el manejo normal de señales de la
                // terminal: Ctrl+C ya no llega como SIGINT (`tokio::signal
                // ::ctrl_c()` nunca dispararía), sino como un evento de
                // tecla más — hay que interceptarlo acá a mano.
                let is_ctrl_c =
                    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                if key.kind == KeyEventKind::Press
                    && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) || is_ctrl_c)
                {
                    break Ok(());
                }
            }
        }

        let reported_state = state_rx.borrow_and_update().clone();

        if jarvis_meter.is_speaking() {
            last_jarvis_active_at = Instant::now();
        }
        let jarvis_active = last_jarvis_active_at.elapsed() < JARVIS_DEMOTE_GRACE;
        let jarvis_target = if jarvis_active {
            jarvis_meter.level().clamp(0.0, 1.0)
        } else {
            0.0
        };
        jarvis_envelope = smooth(jarvis_envelope, jarvis_target, 0.5, 0.06);
        let jarvis_level = jarvis_envelope.clamp(0.0, 1.0);

        let render_state = effective_state(reported_state, jarvis_active);

        let mic_raw = normalize_mic_level(*mic_level_rx.borrow_and_update());
        let user_target: f32 = if matches!(render_state, VisualState::UserSpeaking) {
            mic_raw
        } else {
            0.0
        };
        user_envelope = smooth(user_envelope, user_target, 0.5, 0.15);

        let level = match render_state {
            VisualState::JarvisSpeaking => jarvis_level,
            VisualState::UserSpeaking => user_envelope,
            _ => 0.0,
        };

        let elapsed = start.elapsed().as_secs_f64();

        terminal.draw(|frame| {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(10)])
                .split(frame.area());

            hud::render_label(frame, rows[0], &render_state, &palette);
            wave::render(
                frame,
                rows[1],
                &render_state,
                level,
                elapsed,
                &palette,
                marker,
            );
        })?;

        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown.cancelled() => break Ok(()),
        }
    };

    ratatui::try_restore()?;
    outcome
}
