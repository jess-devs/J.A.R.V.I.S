//! Simulación de input de teclado/mouse vía `SendInput`/`SetCursorPos`
//! (WinAPI), compartida entre `media.rs` (teclas de medios) y `screen.rs`
//! (control de mouse).

use crate::errors::ToolError;

#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
}

/// Mueve el cursor a coordenadas absolutas de pantalla (píxeles físicos,
/// mismo sistema que devuelve `xcap` en los screenshots).
pub async fn move_cursor(x: i32, y: i32) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || unsafe {
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        SetCursorPos(x, y)
            .map_err(|e| ToolError::Execution(format!("no se pudo mover el cursor: {e}")))
    })
    .await
    .map_err(|e| ToolError::Execution(e.to_string()))?
}

/// Simula un click (down+up) del botón indicado en la posición actual del
/// cursor.
pub async fn click_mouse(button: MouseButton) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT,
        };

        let (down_flag, up_flag) = match button {
            MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        };

        let mk_input =
            |flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
        let inputs = [mk_input(down_flag), mk_input(up_flag)];
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent as usize != inputs.len() {
            return Err(ToolError::Execution(
                "SendInput no pudo enviar el click".to_string(),
            ));
        }
        Ok(())
    })
    .await
    .map_err(|e| ToolError::Execution(e.to_string()))?
}

/// Envía un keydown+keyup de una tecla virtual (ej. una tecla de medios) al
/// foco actual del sistema.
pub async fn send_key_press(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        };

        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: Default::default(),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let mut up = down;
        up.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;

        let inputs = [down, up];
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent as usize != inputs.len() {
            return Err(ToolError::Execution(
                "SendInput no pudo enviar la tecla".to_string(),
            ));
        }
        Ok(())
    })
    .await
    .map_err(|e| ToolError::Execution(e.to_string()))?
}
