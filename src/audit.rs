//! Log de auditoría estructurado (JSON Lines) de acciones de riesgo
//! `Confirm`/`Code` y de confirmaciones denegadas/expiradas, independiente
//! de `log_level` general — para que quede un rastro de qué se ejecutó, con
//! qué confirmación, aunque los logs de `debug` estén apagados.
//!
//! Singleton estilo `tracing`: `init()` se llama una sola vez al arrancar
//! (`main.rs`) si `agent.audit.enabled`; `record_tool`/
//! `record_confirmation_denied` son no-op si nunca se inicializó (auditoría
//! desactivada) o si `init` falló (ej. no se pudo crear `data/`).

use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};

struct AuditLog {
    writer: Mutex<NonBlocking>,
}

static AUDIT: OnceLock<AuditLog> = OnceLock::new();
// El `WorkerGuard` flushea el buffer al dropearse; debe vivir tanto como el
// proceso, así que se guarda en un static aparte en vez de descartarse.
static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Inicializa el log de auditoría. Llamar una sola vez al arrancar. Si
/// falla (ej. no se pudo crear el directorio), loguea un warning por
/// `tracing` y deja la auditoría desactivada en vez de abortar el arranque
/// — no es una garantía de seguridad tan crítica como para tumbar Jarvis.
pub fn init(path: &Path) {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "no se pudo crear el directorio del log de auditoría; auditoría desactivada");
        return;
    }
    let Some(file_name) = path.file_name() else {
        tracing::warn!(path = %path.display(), "agent.audit.path no tiene nombre de archivo; auditoría desactivada");
        return;
    };
    let appender = tracing_appender::rolling::never(dir, file_name);
    let (writer, guard) = tracing_appender::non_blocking(appender);
    // Si ya se inicializó (no debería pasar, init() se llama una vez desde
    // main), no pisar el existente.
    let _ = GUARD.set(guard);
    let _ = AUDIT.set(AuditLog {
        writer: Mutex::new(writer),
    });
}

fn write_entry(mut entry: Value) {
    let Some(log) = AUDIT.get() else { return };
    if let Value::Object(map) = &mut entry {
        map.insert(
            "timestamp".to_string(),
            json!(chrono::Utc::now().to_rfc3339()),
        );
    }
    let Ok(mut line) = serde_json::to_string(&entry) else {
        return;
    };
    line.push('\n');
    if let Ok(mut w) = log.writer.lock() {
        let _ = w.write_all(line.as_bytes());
    }
}

/// Registra la ejecución de una tool de riesgo `Confirm`/`Code` (ya sea
/// tras una confirmación de voz o en `confirm_mode: free`).
pub fn record_tool(tool: &str, risk_level: &str, confirm_mode_free: bool, success: bool) {
    write_entry(json!({
        "event": "tool_executed",
        "tool": tool,
        "risk_level": risk_level,
        "confirm_mode_free": confirm_mode_free,
        "success": success,
    }));
}

/// Registra que una confirmación pendiente se resolvió SIN ejecutar la
/// tool: el usuario dijo que no, el código fue incorrecto, o se agotó
/// `confirm_timeout_secs`.
pub fn record_confirmation_denied(tool: &str, requires_code: bool, reason: &str) {
    write_entry(json!({
        "event": "confirmation_denied",
        "tool": tool,
        "requires_code": requires_code,
        "reason": reason,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Único test de este módulo: `AUDIT`/`GUARD` son singletons globales
    /// (`OnceLock`), así que un segundo test llamando `init()` con otra ruta
    /// no tendría efecto (el primer `set()` gana). Se prueban `init` +
    /// ambos `record_*` juntos para no necesitar un segundo `init`.
    #[test]
    fn init_and_record_write_valid_jsonlines() {
        let dir = std::env::temp_dir().join(format!("jarvis_audit_test_{}", std::process::id()));
        let path = dir.join("audit.log");
        init(&path);

        record_tool("close_app", "confirm", false, true);
        record_confirmation_denied("run_powershell", true, "código incorrecto");

        // El writer de tracing-appender es non-blocking (vuelca en un hilo
        // aparte); darle tiempo antes de leer el archivo.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut contents = String::new();
        std::fs::File::open(&path)
            .expect("el archivo de auditoría debería existir")
            .read_to_string(&mut contents)
            .unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "contenido inesperado: {contents:?}");

        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["event"], "tool_executed");
        assert_eq!(first["tool"], "close_app");
        assert_eq!(first["risk_level"], "confirm");
        assert_eq!(first["confirm_mode_free"], false);
        assert_eq!(first["success"], true);
        assert!(first["timestamp"].is_string());

        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["event"], "confirmation_denied");
        assert_eq!(second["tool"], "run_powershell");
        assert_eq!(second["requires_code"], true);
        assert_eq!(second["reason"], "código incorrecto");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
