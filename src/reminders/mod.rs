//! Recordatorios persistentes en SQLite (mismo patrón que `memory::MemoryStore`:
//! `rusqlite` bundled, un solo `Connection` detrás de un `Mutex`, sin
//! framework de migraciones) más un poller en segundo plano que avisa por un
//! canal cuándo un recordatorio venció.
//!
//! El poller NUNCA habla directo (no tiene acceso al `AudioPlayer`, que es
//! `&mut self`-only y vive en el `Orchestrator`): solo empuja `DueReminder`
//! por un `mpsc::channel` que el loop principal del orquestador consume.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use rusqlite::Connection;
use tokio::sync::{mpsc, Mutex};

use crate::errors::ToolError;

#[derive(Debug, Clone)]
pub struct Reminder {
    pub text: String,
    pub trigger_at: String,
}

#[derive(Debug, Clone)]
pub struct DueReminder {
    pub id: i64,
    pub text: String,
}

pub struct ReminderStore {
    conn: Mutex<Connection>,
}

impl ReminderStore {
    pub fn open(path: &Path) -> Result<Self, ToolError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Execution(format!("no se pudo crear {parent:?}: {e}")))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| ToolError::Execution(format!("no se pudo abrir la base: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reminders (
                id           INTEGER PRIMARY KEY,
                text         TEXT NOT NULL,
                trigger_at   TEXT NOT NULL,
                created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                fired_at     TEXT,
                cancelled_at TEXT
            );",
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo crear el schema: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub async fn create(
        &self,
        text: &str,
        trigger_at: &str,
        max_active: usize,
    ) -> Result<i64, ToolError> {
        let conn = self.conn.lock().await;
        let active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM reminders WHERE fired_at IS NULL AND cancelled_at IS NULL",
                [],
                |row| row.get(0),
            )
            .map_err(|e| ToolError::Execution(format!("no se pudo contar recordatorios: {e}")))?;
        if active as usize >= max_active {
            return Err(ToolError::InvalidArgs(format!(
                "ya hay {max_active} recordatorios activos, el límite configurado"
            )));
        }
        conn.execute(
            "INSERT INTO reminders (text, trigger_at) VALUES (?1, ?2)",
            rusqlite::params![text, trigger_at],
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo guardar el recordatorio: {e}")))?;
        Ok(conn.last_insert_rowid())
    }

    pub async fn list_active(&self) -> Result<Vec<Reminder>, ToolError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT text, trigger_at FROM reminders \
                 WHERE fired_at IS NULL AND cancelled_at IS NULL ORDER BY trigger_at ASC",
            )
            .map_err(|e| ToolError::Execution(format!("consulta inválida: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Reminder {
                    text: row.get(0)?,
                    trigger_at: row.get(1)?,
                })
            })
            .map_err(|e| ToolError::Execution(format!("no se pudo consultar: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| ToolError::Execution(format!("no se pudo leer: {e}")))
    }

    /// Cancela los recordatorios activos cuyo texto contenga `query`
    /// (case-insensitive). Devuelve cuántos canceló.
    pub async fn cancel_matching(&self, query: &str) -> Result<usize, ToolError> {
        let conn = self.conn.lock().await;
        let pattern = format!("%{}%", query.to_lowercase());
        let updated = conn
            .execute(
                "UPDATE reminders SET cancelled_at = CURRENT_TIMESTAMP \
                 WHERE fired_at IS NULL AND cancelled_at IS NULL AND lower(text) LIKE ?1",
                rusqlite::params![pattern],
            )
            .map_err(|e| ToolError::Execution(format!("no se pudo cancelar: {e}")))?;
        Ok(updated)
    }

    /// Recordatorios vencidos (trigger_at <= ahora) que aún no se marcaron
    /// como disparados.
    async fn due(&self, now: &str) -> Result<Vec<DueReminder>, ToolError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, text FROM reminders \
                 WHERE fired_at IS NULL AND cancelled_at IS NULL AND trigger_at <= ?1",
            )
            .map_err(|e| ToolError::Execution(format!("consulta inválida: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![now], |row| {
                Ok(DueReminder {
                    id: row.get(0)?,
                    text: row.get(1)?,
                })
            })
            .map_err(|e| ToolError::Execution(format!("no se pudo consultar: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| ToolError::Execution(format!("no se pudo leer: {e}")))
    }

    async fn mark_fired(&self, id: i64) -> Result<(), ToolError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE reminders SET fired_at = CURRENT_TIMESTAMP WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo marcar como disparado: {e}")))?;
        Ok(())
    }
}

/// Tarea en segundo plano: cada `poll_interval` revisa recordatorios
/// vencidos, los marca como disparados y los envía por `tx`. Si el receptor
/// se cerró (el orquestador terminó), la tarea simplemente termina.
pub async fn run_poller(
    store: Arc<ReminderStore>,
    tx: mpsc::Sender<DueReminder>,
    poll_interval: Duration,
) {
    let mut interval = tokio::time::interval(poll_interval);
    loop {
        interval.tick().await;
        let now = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        let due = match store.due(&now).await {
            Ok(due) => due,
            Err(e) => {
                tracing::warn!(error = %e, "no se pudo consultar recordatorios vencidos");
                continue;
            }
        };
        for reminder in due {
            if let Err(e) = store.mark_fired(reminder.id).await {
                tracing::warn!(error = %e, id = reminder.id, "no se pudo marcar el recordatorio como disparado");
                continue;
            }
            if tx.send(reminder).await.is_err() {
                tracing::debug!("canal de recordatorios cerrado, terminando el poller");
                return;
            }
        }
    }
}
