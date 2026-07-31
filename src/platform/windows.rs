use tokio::process::Command;

use crate::errors::ToolError;

/// Lanza `target` (ruta, URL o nombre) con `cmd /C start`, que no bloquea y
/// resuelve igual que lo haría el usuario en Win+R o con doble click (PATH +
/// App Paths del registro + asociaciones de archivo/protocolo).
pub async fn open_target(target: &str) -> Result<(), ToolError> {
    let status = Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(target)
        .status()
        .await
        .map_err(|e| ToolError::Execution(format!("no se pudo lanzar '{target}': {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::Execution(format!(
            "Windows no pudo lanzar '{target}'."
        )))
    }
}
