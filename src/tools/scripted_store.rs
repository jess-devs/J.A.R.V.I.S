//! Persistencia de tools "scripted" creadas en tiempo de ejecución por
//! `create_tool` (`scripted.rs`). Mismo patrón que `memory::MemoryStore`:
//! `rusqlite` bundled, un `Connection` detrás de un `Mutex`, sin framework de
//! migraciones. Vive en su propio archivo (no en `memory.db`) para mantener
//! separables los datos de mayor riesgo (plantillas que ejecutan comandos).

use std::path::Path;

use rusqlite::Connection;
use tokio::sync::Mutex;

use crate::errors::ToolError;

use super::scripted::ToolRecipe;
use super::RiskLevel;

#[derive(Debug, Clone)]
pub struct ScriptedToolDef {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
    pub risk_level: RiskLevel,
    pub recipe: ToolRecipe,
}

fn risk_to_str(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Safe => "safe",
        RiskLevel::Confirm => "confirm",
        RiskLevel::Code => "code",
    }
}

fn risk_from_str(s: &str) -> RiskLevel {
    match s {
        "code" => RiskLevel::Code,
        _ => RiskLevel::Confirm,
    }
}

pub struct ScriptedToolStore {
    conn: Mutex<Connection>,
}

impl ScriptedToolStore {
    pub fn open(path: &Path) -> Result<Self, ToolError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Execution(format!("no se pudo crear {parent:?}: {e}")))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| ToolError::Execution(format!("no se pudo abrir la base: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS custom_tools (
                id              INTEGER PRIMARY KEY,
                name            TEXT NOT NULL UNIQUE,
                description     TEXT NOT NULL,
                parameters_json TEXT NOT NULL,
                risk_level      TEXT NOT NULL,
                recipe_json     TEXT NOT NULL,
                created_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo crear el schema: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub async fn create(&self, def: &ScriptedToolDef, max_tools: usize) -> Result<(), ToolError> {
        let conn = self.conn.lock().await;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM custom_tools", [], |row| row.get(0))
            .map_err(|e| ToolError::Execution(format!("no se pudo contar tools: {e}")))?;
        if count as usize >= max_tools {
            return Err(ToolError::InvalidArgs(format!(
                "ya hay {max_tools} tools personalizadas, el límite configurado"
            )));
        }
        let parameters_json = def.parameters_schema.to_string();
        let recipe_json = serde_json::to_string(&def.recipe)
            .map_err(|e| ToolError::Execution(format!("no se pudo serializar la receta: {e}")))?;
        conn.execute(
            "INSERT INTO custom_tools (name, description, parameters_json, risk_level, recipe_json) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                def.name,
                def.description,
                parameters_json,
                risk_to_str(def.risk_level),
                recipe_json,
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                ToolError::InvalidArgs(format!("ya existe una tool llamada '{}'", def.name))
            } else {
                ToolError::Execution(format!("no se pudo guardar la tool: {e}"))
            }
        })?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<ScriptedToolDef>, ToolError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT name, description, parameters_json, risk_level, recipe_json FROM custom_tools ORDER BY id ASC")
            .map_err(|e| ToolError::Execution(format!("consulta inválida: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let name: String = row.get(0)?;
                let description: String = row.get(1)?;
                let parameters_json: String = row.get(2)?;
                let risk_level: String = row.get(3)?;
                let recipe_json: String = row.get(4)?;
                Ok((name, description, parameters_json, risk_level, recipe_json))
            })
            .map_err(|e| ToolError::Execution(format!("no se pudo consultar: {e}")))?;

        let mut defs = Vec::new();
        for row in rows {
            let (name, description, parameters_json, risk_level, recipe_json) =
                row.map_err(|e| ToolError::Execution(format!("no se pudo leer: {e}")))?;
            let parameters_schema = match serde_json::from_str(&parameters_json) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(tool = %name, error = %e, "parameters_json inválido, se ignora la tool");
                    continue;
                }
            };
            let recipe: ToolRecipe = match serde_json::from_str(&recipe_json) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(tool = %name, error = %e, "recipe_json inválido, se ignora la tool");
                    continue;
                }
            };
            defs.push(ScriptedToolDef {
                name,
                description,
                parameters_schema,
                risk_level: risk_from_str(&risk_level),
                recipe,
            });
        }
        Ok(defs)
    }

    /// Borra las tools cuyo nombre coincida exactamente o contenga `query`
    /// (case-insensitive). Devuelve cuántas borró.
    pub async fn delete_matching(&self, query: &str) -> Result<usize, ToolError> {
        let conn = self.conn.lock().await;
        let pattern = format!("%{}%", query.to_lowercase());
        let deleted = conn
            .execute(
                "DELETE FROM custom_tools WHERE lower(name) LIKE ?1",
                rusqlite::params![pattern],
            )
            .map_err(|e| ToolError::Execution(format!("no se pudo borrar: {e}")))?;
        Ok(deleted)
    }
}
