//! Memoria persistente local en SQLite (`rusqlite` bundled: sin DLLs
//! externas ni servicios). Cada memoria es una frase corta ("el cumpleaños
//! del usuario es el 3 de marzo") con categoría opcional.
//!
//! Búsqueda por LIKE sobre términos — suficiente para cientos de memorias.
//! Si algún día hace falta relevancia semántica, el camino es FTS5 (viene
//! compilado en el bundle de SQLite), no un vector store.

use std::path::Path;

use rusqlite::Connection;
use tokio::sync::Mutex;

use crate::errors::ToolError;

#[derive(Debug, Clone)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub category: Option<String>,
    pub created_at: String,
}

pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    pub fn open(path: &Path) -> Result<Self, ToolError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Execution(format!("no se pudo crear {parent:?}: {e}")))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| ToolError::Execution(format!("no se pudo abrir la base: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id         INTEGER PRIMARY KEY,
                content    TEXT NOT NULL,
                category   TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo crear el schema: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub async fn remember(
        &self,
        content: &str,
        category: Option<&str>,
    ) -> Result<i64, ToolError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO memories (content, category) VALUES (?1, ?2)",
            rusqlite::params![content, category],
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo guardar: {e}")))?;
        Ok(conn.last_insert_rowid())
    }

    /// Busca memorias cuyo contenido contenga TODOS los términos del query
    /// (case-insensitive), de la más reciente a la más vieja.
    pub async fn recall(&self, query: &str, limit: usize) -> Result<Vec<Memory>, ToolError> {
        let terms: Vec<String> = query
            .split_whitespace()
            .filter(|t| t.chars().count() >= 3)
            .map(|t| format!("%{}%", t.to_lowercase()))
            .collect();
        let conn = self.conn.lock().await;

        let (sql, params): (String, Vec<&dyn rusqlite::ToSql>) = if terms.is_empty() {
            (
                "SELECT id, content, category, created_at FROM memories \
                 ORDER BY id DESC LIMIT ?1"
                    .to_string(),
                vec![&limit as &dyn rusqlite::ToSql],
            )
        } else {
            let conditions: Vec<String> = (0..terms.len())
                .map(|i| format!("lower(content) LIKE ?{}", i + 1))
                .collect();
            let sql = format!(
                "SELECT id, content, category, created_at FROM memories \
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
                    created_at: row.get(3)?,
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
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY, content TEXT NOT NULL, category TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);",
        )
        .unwrap();
        MemoryStore {
            conn: Mutex::new(conn),
        }
    }

    #[tokio::test]
    async fn guarda_y_recupera() {
        let s = store().await;
        s.remember("el cumpleaños del usuario es el 3 de marzo", Some("personal"))
            .await
            .unwrap();
        let found = s.recall("cumpleaños marzo", 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].content.contains("3 de marzo"));
    }

    #[tokio::test]
    async fn recall_sin_match() {
        let s = store().await;
        s.remember("le gusta el café sin azúcar", None).await.unwrap();
        assert!(s.recall("mascota perro", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn forget_borra_lo_que_matchea() {
        let s = store().await;
        s.remember("clave del wifi de la oficina: no recordarla", None)
            .await
            .unwrap();
        s.remember("le gusta el café", None).await.unwrap();
        let deleted = s.forget("wifi oficina").await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(s.all_recent(10).await.unwrap().len(), 1);
    }
}
