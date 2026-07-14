//! Detecta el cierre de la ventana de consola, logoff o apagado del sistema
//! (`CTRL_CLOSE_EVENT` / `CTRL_LOGOFF_EVENT` / `CTRL_SHUTDOWN_EVENT`), que
//! hoy no disparan ningún shutdown ordenado (solo Ctrl+C lo hace). El
//! handler de Windows corre en un hilo aparte y no puede hacer `.await`, así
//! que la señal cruza a través de un canal `mpsc` no bloqueante hacia el
//! `tokio::select!` de `main.rs`.

use std::sync::OnceLock;

use tokio::sync::mpsc;
use windows::Win32::Foundation::BOOL;
use windows::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
};

static SHUTDOWN_TX: OnceLock<mpsc::Sender<()>> = OnceLock::new();

unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
    match ctrl_type {
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            if let Some(tx) = SHUTDOWN_TX.get() {
                let _ = tx.try_send(());
            }
            BOOL(1)
        }
        _ => BOOL(0),
    }
}

/// Instala el handler de consola y devuelve el receiver que se despierta
/// cuando se detecta cierre de ventana, logoff o apagado.
pub fn install() -> windows::core::Result<mpsc::Receiver<()>> {
    let (tx, rx) = mpsc::channel(1);
    SHUTDOWN_TX
        .set(tx)
        .expect("console_handler::install() no debe llamarse más de una vez");
    unsafe { SetConsoleCtrlHandler(Some(handler), true) }?;
    Ok(rx)
}
