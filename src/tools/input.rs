//! Simulación de input de mouse/teclado vía `enigo` (cross-platform:
//! Windows, macOS, Linux/X11), compartida entre `media.rs` (teclas de
//! medios) y `screen.rs` (control de mouse). En Linux/Wayland puede no
//! funcionar sin soporte explícito del compositor — limitación conocida de
//! `enigo`, no de Jarvis (ver MEJORAS.md ítem 1).

use enigo::{Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

use crate::errors::ToolError;

#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
}

fn new_enigo() -> Result<Enigo, ToolError> {
    Enigo::new(&Settings::default())
        .map_err(|e| ToolError::Execution(format!("no se pudo inicializar el control de input: {e}")))
}

/// Mueve el cursor a coordenadas absolutas de pantalla (píxeles físicos,
/// mismo sistema que devuelve `xcap` en los screenshots).
pub async fn move_cursor(x: i32, y: i32) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = new_enigo()?;
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| ToolError::Execution(format!("no se pudo mover el cursor: {e}")))
    })
    .await
    .map_err(|e| ToolError::Execution(e.to_string()))?
}

/// Simula un click (down+up) del botón indicado en la posición actual del
/// cursor.
pub async fn click_mouse(button: MouseButton) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = new_enigo()?;
        let btn = match button {
            MouseButton::Left => enigo::Button::Left,
            MouseButton::Right => enigo::Button::Right,
        };
        enigo
            .button(btn, Direction::Click)
            .map_err(|e| ToolError::Execution(format!("no se pudo hacer click: {e}")))
    })
    .await
    .map_err(|e| ToolError::Execution(e.to_string()))?
}

/// Envía un keydown+keyup de una tecla (ej. una tecla de medios) al foco
/// actual del sistema.
pub async fn send_key(key: Key) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = new_enigo()?;
        enigo
            .key(key, Direction::Click)
            .map_err(|e| ToolError::Execution(format!("no se pudo enviar la tecla: {e}")))
    })
    .await
    .map_err(|e| ToolError::Execution(e.to_string()))?
}
