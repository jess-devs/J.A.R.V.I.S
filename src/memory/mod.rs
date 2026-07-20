//! Memoria persistente local en SQLite (`rusqlite` bundled: sin DLLs
//! externas ni servicios). Cada memoria es una frase corta ("el cumpleaños
//! del usuario es el 3 de marzo") con categoría opcional.
//!
//! Búsqueda por LIKE sobre términos — suficiente para cientos de memorias.
//! Si algún día hace falta relevancia semántica, el camino es FTS5 (viene
//! compilado en el bundle de SQLite), no un vector store.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use tokio::sync::Mutex;

use crate::errors::ToolError;

#[derive(Debug, Clone)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub category: Option<String>,
    /// Qué dijo o hizo el usuario que motivó guardar esto, capturado en el
    /// momento de crear la memoria. `None` en memorias guardadas antes de
    /// esta columna, o si la tool no lo recibió.
    pub reason: Option<String>,
    pub created_at: String,
}

pub struct MemoryStore {
    conn: Mutex<Connection>,
    /// Se incrementa en cada escritura (remember/forget) para que el
    /// orquestador sepa cuándo invalidar el bloque de memorias cacheado en
    /// el system prompt.
    generation: AtomicU64,
}

/// Crea el schema si no existe y lo migra in-place si viene de una versión
/// anterior. Sin framework de migraciones: cada paso es idempotente.
fn init_schema(conn: &Connection) -> Result<(), ToolError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (
            id         INTEGER PRIMARY KEY,
            content    TEXT NOT NULL,
            category   TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .map_err(|e| ToolError::Execution(format!("no se pudo crear el schema: {e}")))?;

    // Bases anteriores a la memoria explicable no tienen `reason`. Las filas
    // viejas quedan con reason = NULL. "duplicate column" = ya migrada.
    if let Err(e) = conn.execute("ALTER TABLE memories ADD COLUMN reason TEXT", []) {
        let msg = e.to_string();
        if !msg.contains("duplicate column") {
            return Err(ToolError::Execution(format!(
                "no se pudo migrar el schema: {e}"
            )));
        }
    }
    Ok(())
}

impl MemoryStore {
    pub fn open(path: &Path) -> Result<Self, ToolError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Execution(format!("no se pudo crear {parent:?}: {e}")))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| ToolError::Execution(format!("no se pudo abrir la base: {e}")))?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            generation: AtomicU64::new(0),
        })
    }

    /// Contador de escrituras: cambia cada vez que remember/forget modifican
    /// la base. Sirve como clave de invalidación de caches.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub async fn remember(
        &self,
        content: &str,
        category: Option<&str>,
        reason: Option<&str>,
    ) -> Result<i64, ToolError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO memories (content, category, reason) VALUES (?1, ?2, ?3)",
            rusqlite::params![content, category, reason],
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo guardar: {e}")))?;
        self.generation.fetch_add(1, Ordering::Relaxed);
        Ok(conn.last_insert_rowid())
    }

    /// Busca memorias cuyo contenido contenga TODOS los términos del query
    /// (case-insensitive), de la más reciente a la más vieja. El motivo
    /// (`reason`) no participa de la búsqueda, solo se devuelve.
    pub async fn recall(&self, query: &str, limit: usize) -> Result<Vec<Memory>, ToolError> {
        let terms: Vec<String> = query
            .split_whitespace()
            .filter(|t| t.chars().count() >= 3)
            .map(|t| format!("%{}%", t.to_lowercase()))
            .collect();
        let conn = self.conn.lock().await;

        let (sql, params): (String, Vec<&dyn rusqlite::ToSql>) = if terms.is_empty() {
            (
                "SELECT id, content, category, reason, created_at FROM memories \
                 ORDER BY id DESC LIMIT ?1"
                    .to_string(),
                vec![&limit as &dyn rusqlite::ToSql],
            )
        } else {
            let conditions: Vec<String> = (0..terms.len())
                .map(|i| format!("lower(content) LIKE ?{}", i + 1))
                .collect();
            let sql = format!(
                "SELECT id, content, category, reason, created_at FROM memories \
                 WHERE {} ORDER BY id DESC LIMIT ?{}",
                conditions.join(" AND "),
                terms.len() + 1
            );
            let mut params: Vec<&dyn rusqlite::ToSql> =
                terms.iter().map(|t| t as &dyn rusqlite::ToSql).collect();
            params.push(&limit);
            (sql, params)
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| ToolError::Execution(format!("consulta inválida: {e}")))?;
        let rows = stmt
            .query_map(params.as_slice(), |row| {
                Ok(Memory {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    reason: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| ToolError::Execution(format!("no se pudo consultar: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| ToolError::Execution(format!("no se pudo leer: {e}")))
    }

    /// Borra las memorias que matchean el query (mismos términos que
    /// `recall`). Devuelve cuántas borró.
    pub async fn forget(&self, query: &str) -> Result<usize, ToolError> {
        let matches = self.recall(query, 50).await?;
        if matches.is_empty() {
            return Ok(0);
        }
        let ids: Vec<String> = matches.iter().map(|m| m.id.to_string()).collect();
        let conn = self.conn.lock().await;
        let deleted = conn
            .execute(
                &format!("DELETE FROM memories WHERE id IN ({})", ids.join(",")),
                [],
            )
            .map_err(|e| ToolError::Execution(format!("no se pudo borrar: {e}")))?;
        if deleted > 0 {
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
        Ok(deleted)
    }

    /// Las más recientes, para inyectar en el system prompt.
    pub async fn all_recent(&self, limit: usize) -> Result<Vec<Memory>, ToolError> {
        self.recall("", limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> MemoryStore {
        // Base en memoria: mismo código, sin archivo.
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        MemoryStore {
            conn: Mutex::new(conn),
            generation: AtomicU64::new(0),
        }
    }

    #[tokio::test]
    async fn guarda_y_recupera() {
        let s = store().await;
        s.remember(
            "el cumpleaños del usuario es el 3 de marzo",
            Some("personal"),
            Some("el usuario lo mencionó al pedir un recordatorio"),
        )
        .await
        .unwrap();
        let found = s.recall("cumpleaños marzo", 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].content.contains("3 de marzo"));
    }

    #[tokio::test]
    async fn recall_sin_match() {
        let s = store().await;
        s.remember("le gusta el café sin azúcar", None, None)
            .await
            .unwrap();
        assert!(s.recall("mascota perro", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn forget_borra_lo_que_matchea() {
        let s = store().await;
        s.remember("clave del wifi de la oficina: no recordarla", None, None)
            .await
            .unwrap();
        s.remember("le gusta el café", None, None).await.unwrap();
        let deleted = s.forget("wifi oficina").await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(s.all_recent(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn guarda_y_recupera_motivo() {
        let s = store().await;
        s.remember(
            "al usuario le gusta el café sin azúcar",
            Some("preferencias"),
            Some("el usuario lo dijo al pedir que le prepare un café"),
        )
        .await
        .unwrap();
        let found = s.recall("café azúcar", 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].reason.as_deref(),
            Some("el usuario lo dijo al pedir que le prepare un café")
        );
    }

    #[tokio::test]
    async fn migra_base_vieja() {
        // Simula una base creada antes de la columna `reason`.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY, content TEXT NOT NULL, category TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memories (content, category) VALUES (?1, ?2)",
            rusqlite::params!["memoria de antes de la migración", Option::<&str>::None],
        )
        .unwrap();

        // init_schema debe migrar sin romper la fila existente.
        init_schema(&conn).unwrap();
        let s = MemoryStore {
            conn: Mutex::new(conn),
            generation: AtomicU64::new(0),
        };

        let all = s.all_recent(10).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].reason, None);

        s.remember("memoria nueva", None, Some("motivo nuevo"))
            .await
            .unwrap();
        let nueva = s.recall("nueva", 10).await.unwrap();
        assert_eq!(nueva[0].reason.as_deref(), Some("motivo nuevo"));
    }
}
