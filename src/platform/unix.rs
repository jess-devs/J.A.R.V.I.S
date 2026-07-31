use tokio::process::Command;

use crate::errors::ToolError;

/// Lanza `target` (ruta, URL o nombre de comando) con el manejador por
/// defecto del SO: `open` en macOS, `xdg-open` en el resto de Unix. Si eso
/// falla (por ejemplo porque `target` es el nombre de una app sin
/// asociación de MIME/URL, como "spotify" dicho por voz en vez de una ruta
/// o una URL), cae a intentar lanzarlo directo como comando de PATH — cubre
/// el caso común de `open_app` sin escaneo de accesos directos (ese
/// escaneo es Windows-only por ahora, ver `tools/apps.rs`).
pub async fn open_target(target: &str) -> Result<(), ToolError> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Ok(status) = Command::new(opener).arg(target).status().await {
        if status.success() {
            return Ok(());
        }
    }
    match Command::new(target).spawn() {
        Ok(_) => Ok(()),
        Err(e) => Err(ToolError::Execution(format!(
            "no se pudo lanzar '{target}': {e}"
        ))),
    }
}

/// Ejecuta un comando de shell (`sh -c`) y devuelve su salida formateada
/// (stdout + stderr, o un mensaje si no hubo salida). Equivalente Unix de
/// `run_powershell_command` (ver `tools/shell.rs`).
pub async fn run_shell_command(command: &str) -> Result<String, ToolError> {
    let output = Command::new("sh")
        .args(["-c", command])
        .output()
        .await
        .map_err(|e| ToolError::Execution(format!("no se pudo lanzar el shell: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut result = String::new();
    if !stdout.trim().is_empty() {
        result.push_str(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("(stderr) {}", stderr.trim()));
    }
    if result.is_empty() {
        result = if output.status.success() {
            "Comando ejecutado sin salida.".to_string()
        } else {
            format!(
                "El comando falló (código {:?}) sin salida.",
                output.status.code()
            )
        };
    }
    Ok(result)
}
