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

pub use state::{UiState, VisualState};

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::symbols::Marker;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::config::UiConfig;
use crate::errors::Result;
use theme::Palette;

/// Corre el loop de renderizado hasta que el usuario pide salir (`q`/Esc) o
/// falla la terminal. Instala/restaura la terminal (raw mode + alternate
/// screen) al entrar/salir; un panic mientras corre queda cubierto por el
/// panic hook que instala `ratatui::try_init` (restaura la terminal antes de
/// propagar al hook de `main.rs`, que a su vez limpia los workers Python).
pub async fn run(
    config: UiConfig,
    mut state_rx: watch::Receiver<VisualState>,
    mut level_rx: watch::Receiver<f32>,
    mut mic_level_rx: watch::Receiver<f32>,
    shutdown: CancellationToken,
) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let palette = Palette::from_config(&config);
    let marker = if config.marker == "block" {
        Marker::Block
    } else {
        Marker::Braille
    };

    let fps = config.fps.max(1);
    let frame_duration = Duration::from_millis(1000 / u64::from(fps));
    let start = Instant::now();

    // Envolvente del nivel real del micrófono (dBFS normalizado por
    // `Orchestrator::normalize_mic_level`), suavizada con ataque rápido y
    // liberación un poco más lenta para que reaccione al volumen de la voz
    // sin verse nerviosa cuadro a cuadro. Fuera de `UserSpeaking` el objetivo
    // es 0.0, así que decae solo al terminar de hablar (no hace falta un
    // "gap de silencio" como con Jarvis: acá el límite lo da el propio
    // cambio de estado, `VadEnd` ya dispara `Listening` de inmediato).
    let mut user_envelope: f32 = 0.0;

    // El nivel de audio de Jarvis solo se actualiza cuando hay un chunk de
    // TTS nuevo (`AudioPlayer::play_chunk`); nada lo vuelve a 0 cuando el
    // audio termina. Si no llegó ningún cambio en un rato se asume silencio
    // — con un margen generoso, porque Jarvis sintetiza frase por frase y
    // entre una y la siguiente hay un hueco real sin audio (mientras se
    // genera la próxima) que no debería leerse como "dejó de hablar".
    let mut last_jarvis_raw: f32 = 0.0;
    let mut last_jarvis_update = Instant::now();
    const JARVIS_SILENCE_GAP: Duration = Duration::from_millis(500);
    // Umbral por debajo del cual se considera que todavía no hay audio real
    // sonando (turno recién empezado, LLM generando): el estado se ve como
    // `Thinking` hasta que este umbral se supera, momento en que se "asciende"
    // a `JarvisSpeaking` sin que el orquestador necesite saber cuándo
    // arrancó el TTS.
    const SPEAKING_THRESHOLD: f32 = 0.015;
    // Envolvente suavizada del nivel de Jarvis: ataque rápido (reacciona ya
    // al primer chunk audible) y liberación lenta (no parpadea entre frases
    // ni cae en seco al final de una).
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

        if level_rx.has_changed().unwrap_or(false) {
            last_jarvis_raw = level_rx.borrow_and_update().clamp(0.0, 1.0);
            last_jarvis_update = Instant::now();
        }
        let jarvis_target = if last_jarvis_update.elapsed() > JARVIS_SILENCE_GAP {
            0.0
        } else {
            last_jarvis_raw
        };
        let jarvis_rate: f32 = if jarvis_target > jarvis_envelope {
            0.5
        } else {
            0.06
        };
        jarvis_envelope += (jarvis_target - jarvis_envelope) * jarvis_rate;
        let jarvis_level = jarvis_envelope.clamp(0.0, 1.0);

        let render_state = if matches!(reported_state, VisualState::Thinking)
            && jarvis_level > SPEAKING_THRESHOLD
        {
            VisualState::JarvisSpeaking
        } else {
            reported_state
        };

        let mic_raw = mic_level_rx.borrow_and_update().clamp(0.0, 1.0);
        let user_target: f32 = if matches!(render_state, VisualState::UserSpeaking) {
            mic_raw
        } else {
            0.0
        };
        let rate: f32 = if user_target > user_envelope {
            0.5
        } else {
            0.15
        };
        user_envelope += (user_target - user_envelope) * rate;

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
            _ = tokio::time::sleep(frame_duration) => {}
            _ = shutdown.cancelled() => break Ok(()),
        }
    };

    ratatui::try_restore()?;
    outcome
}
