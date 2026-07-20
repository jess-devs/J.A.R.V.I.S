//! Observación local de patrones de uso: cuenta qué herramientas ejecuta el
//! usuario, con qué argumentos y en qué franja horaria, y cuando una acción
//! se repite lo bastante propone (nunca aplica) una adaptación. Mismo
//! patrón que `reminders`: `rusqlite` bundled, sin framework de migraciones,
//! y una tarea en segundo plano (`run_scanner`) que avisa por un canal.
//!
//! Todo el análisis es conteo/agregación local en SQLite — sin
//! entrenamiento de modelos ni servicios externos. `run_scanner` NUNCA habla
//! directo: solo empuja `HabitSuggestion` por un `mpsc::channel` que el
//! orquestador consume, igual que con los recordatorios.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Local, Timelike};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};

use crate::config::HabitsConfig;
use crate::errors::ToolError;

const DATE_FMT: &str = "%Y-%m-%dT%H:%M:%S";

#[derive(Debug, Clone)]
pub struct HabitSuggestion {
    pub id: i64,
    /// Frase ya redactada del patrón detectado, lista para incluir en el
    /// evento sintético que se le manda al LLM.
    pub description: String,
}

pub struct HabitStore {
    conn: Mutex<Connection>,
}

fn init_schema(conn: &Connection) -> Result<(), ToolError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_events (
            id             INTEGER PRIMARY KEY,
            tool_name      TEXT NOT NULL,
            args_signature TEXT,
            weekday        INTEGER NOT NULL,
            hour           INTEGER NOT NULL,
            created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS suggestions (
            id           INTEGER PRIMARY KEY,
            pattern_key  TEXT NOT NULL UNIQUE,
            description  TEXT NOT NULL,
            status       TEXT NOT NULL DEFAULT 'proposed',
            created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            offered_at   TEXT,
            resolved_at  TEXT,
            snooze_until TEXT
        );",
    )
    .map_err(|e| ToolError::Execution(format!("no se pudo crear el schema: {e}")))?;
    Ok(())
}

impl HabitStore {
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
        })
    }

    /// Registra un tool call exitoso. `args_signature` en `None` significa
    /// que la tool no es de las que se consideran automatizables: el evento
    /// igual queda para estadística, pero `detect_patterns` lo ignora.
    pub async fn record_event(
        &self,
        tool_name: &str,
        args_signature: Option<&str>,
    ) -> Result<(), ToolError> {
        let now = Local::now();
        let weekday = now.weekday().num_days_from_monday() as i64;
        let hour = now.hour() as i64;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO tool_events (tool_name, args_signature, weekday, hour) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tool_name, args_signature, weekday, hour],
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo registrar el evento: {e}")))?;
        Ok(())
    }

    /// Agrupa los eventos con firma de los últimos `window_days` por
    /// (tool, firma, franja horaria) y da de alta como 'proposed' los
    /// patrones que superan `min_occurrences`. Un patrón ya `proposed`,
    /// `offered` o `accepted` no se toca (evita resetear su snooze o
    /// reabrir algo ya resuelto). Un patrón `rejected` solo se reactiva si
    /// pasó `rejected_cooldown_days` desde que se rechazó.
    pub async fn detect_patterns(&self, cfg: &HabitsConfig) -> Result<(), ToolError> {
        let cutoff = (Local::now() - chrono::Duration::days(cfg.window_days as i64))
            .format(DATE_FMT)
            .to_string();
        let cooldown_cutoff = (Local::now()
            - chrono::Duration::days(cfg.rejected_cooldown_days as i64))
        .format(DATE_FMT)
        .to_string();

        let conn = self.conn.lock().await;
        let rows: Vec<(String, String, String, i64)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT tool_name, args_signature,
                        CASE
                            WHEN hour BETWEEN 0 AND 5 THEN 'madrugada'
                            WHEN hour BETWEEN 6 AND 11 THEN 'mañana'
                            WHEN hour BETWEEN 12 AND 17 THEN 'tarde'
                            ELSE 'noche'
                        END AS franja,
                        COUNT(*) AS occurrences
                     FROM tool_events
                     WHERE args_signature IS NOT NULL AND created_at >= ?1
                     GROUP BY tool_name, args_signature, franja
                     HAVING occurrences >= ?2",
                )
                .map_err(|e| ToolError::Execution(format!("consulta inválida: {e}")))?;
            let mapped = stmt
                .query_map(
                    rusqlite::params![cutoff, cfg.min_occurrences as i64],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|e| ToolError::Execution(format!("no se pudo consultar: {e}")))?;
            mapped
                .collect::<Result<_, _>>()
                .map_err(|e| ToolError::Execution(format!("no se pudo leer: {e}")))?
        };

        for (tool_name, signature, franja, occurrences) in rows {
            let pattern_key = format!("{tool_name}|{signature}|{franja}");
            let description = format!(
                "{} {} (ocurrió {} veces en los últimos {} días)",
                describe_pattern(&tool_name, &signature),
                franja_frase(&franja),
                occurrences,
                cfg.window_days
            );
            conn.execute(
                "INSERT INTO suggestions (pattern_key, description, status) \
                 VALUES (?1, ?2, 'proposed') \
                 ON CONFLICT(pattern_key) DO UPDATE SET \
                    description = excluded.description, \
                    status = 'proposed', \
                    resolved_at = NULL, \
                    snooze_until = NULL \
                 WHERE status = 'rejected' AND resolved_at <= ?3",
                rusqlite::params![pattern_key, description, cooldown_cutoff],
            )
            .map_err(|e| ToolError::Execution(format!("no se pudo registrar el patrón: {e}")))?;
        }
        Ok(())
    }

    /// La sugerencia elegible más vieja: `proposed`, o `offered` cuyo
    /// snooze ya venció. La marca `offered` (con nuevo snooze) ANTES de
    /// devolverla, como `ReminderStore::mark_fired` — evita ofrecerla dos
    /// veces si el scanner corre de nuevo antes de que se resuelva.
    /// Respeta `max_per_day`: si ya se ofrecieron demasiadas hoy, no da
    /// ninguna aunque haya candidatas.
    pub async fn take_next_proposed(
        &self,
        cfg: &HabitsConfig,
    ) -> Result<Option<HabitSuggestion>, ToolError> {
        let conn = self.conn.lock().await;
        let now = Local::now();
        let now_str = now.format(DATE_FMT).to_string();
        let today_start = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 es una hora válida")
            .format(DATE_FMT)
            .to_string();

        let offered_today: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM suggestions WHERE offered_at >= ?1",
                rusqlite::params![today_start],
                |row| row.get(0),
            )
            .map_err(|e| ToolError::Execution(format!("no se pudo contar sugerencias: {e}")))?;
        if offered_today as usize >= cfg.max_per_day {
            return Ok(None);
        }

        let candidate: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, description FROM suggestions \
                 WHERE status = 'proposed' OR (status = 'offered' AND snooze_until <= ?1) \
                 ORDER BY created_at ASC LIMIT 1",
                rusqlite::params![now_str],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| ToolError::Execution(format!("no se pudo consultar: {e}")))?;

        let Some((id, description)) = candidate else {
            return Ok(None);
        };

        let snooze_until = (now + chrono::Duration::days(cfg.snooze_days as i64))
            .format(DATE_FMT)
            .to_string();
        conn.execute(
            "UPDATE suggestions SET status = 'offered', offered_at = ?1, snooze_until = ?2 \
             WHERE id = ?3",
            rusqlite::params![now_str, snooze_until, id],
        )
        .map_err(|e| ToolError::Execution(format!("no se pudo marcar como ofrecida: {e}")))?;

        Ok(Some(HabitSuggestion { id, description }))
    }

    /// Resuelve una sugerencia tras la respuesta del usuario. `false` si el
    /// id no existe (la tool que lo llama lo reporta como error hablable).
    pub async fn resolve(&self, id: i64, accepted: bool) -> Result<bool, ToolError> {
        let conn = self.conn.lock().await;
        let status = if accepted { "accepted" } else { "rejected" };
        let updated = conn
            .execute(
                "UPDATE suggestions SET status = ?1, resolved_at = CURRENT_TIMESTAMP WHERE id = ?2",
                rusqlite::params![status, id],
            )
            .map_err(|e| ToolError::Execution(format!("no se pudo resolver la sugerencia: {e}")))?;
        Ok(updated > 0)
    }
}

fn franja_frase(franja: &str) -> &'static str {
    match franja {
        "madrugada" => "de madrugada",
        "mañana" => "por la mañana",
        "tarde" => "por la tarde",
        _ => "por la noche",
    }
}

fn describe_pattern(tool_name: &str, signature: &str) -> String {
    match tool_name {
        "open_app" => format!("abrir {signature}"),
        "open_url" => format!("abrir {signature}"),
        "media_control" => format!("la acción de {signature} en la reproducción"),
        _ => format!("usar {tool_name} ({signature})"),
    }
}

/// Firma ligera y determinista de los argumentos de un tool call, sin datos
/// sensibles. `None` = la tool no está en la whitelist de automatizables y
/// no debe generar sugerencias (aunque el evento igual se registre).
pub fn args_signature(tool_name: &str, args: &Value) -> Option<String> {
    match tool_name {
        "open_app" => args
            .get("name")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty()),
        "open_url" => args
            .get("url")
            .and_then(Value::as_str)
            .and_then(url_host)
            .filter(|s| !s.is_empty()),
        "media_control" => args
            .get("action")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty()),
        _ => None,
    }
}

/// Extrae el host de una URL sin depender de un crate de parsing (basta con
/// cortar el esquema y quedarse con lo anterior a la primera barra).
fn url_host(url: &str) -> Option<String> {
    let without_scheme = match url.find("://") {
        Some(idx) => &url[idx + 3..],
        None => url,
    };
    let host = without_scheme.split('/').next().unwrap_or(without_scheme);
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// Tarea en segundo plano: cada `poll_interval_secs` analiza patrones y, si
/// hay una sugerencia elegible, la envía por `tx`. Igual que
/// `reminders::run_poller`, nunca habla directo y termina si el receptor se
/// cerró.
pub async fn run_scanner(
    store: Arc<HabitStore>,
    cfg: HabitsConfig,
    tx: mpsc::Sender<HabitSuggestion>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(cfg.poll_interval_secs));
    loop {
        interval.tick().await;
        if let Err(e) = store.detect_patterns(&cfg).await {
            tracing::warn!(error = %e, "no se pudo analizar patrones de uso");
            continue;
        }
        match store.take_next_proposed(&cfg).await {
            Ok(Some(suggestion)) => {
                if tx.send(suggestion).await.is_err() {
                    tracing::debug!("canal de sugerencias cerrado, terminando el scanner");
                    return;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "no se pudo obtener la próxima sugerencia"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg() -> HabitsConfig {
        HabitsConfig {
            enabled: true,
            db_path: Default::default(),
            min_occurrences: 3,
            window_days: 14,
            poll_interval_secs: 600,
            rejected_cooldown_days: 30,
            snooze_days: 3,
            max_per_day: 1,
        }
    }

    async fn store() -> HabitStore {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        HabitStore {
            conn: Mutex::new(conn),
        }
    }

    #[tokio::test]
    async fn detecta_patron_que_supera_el_umbral() {
        let s = store().await;
        let cfg = test_cfg();
        for _ in 0..3 {
            s.record_event("open_app", Some("spotify")).await.unwrap();
        }
        s.detect_patterns(&cfg).await.unwrap();
        let suggestion = s.take_next_proposed(&cfg).await.unwrap();
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().description.contains("spotify"));
    }

    #[tokio::test]
    async fn no_propone_por_debajo_del_umbral() {
        let s = store().await;
        let cfg = test_cfg();
        s.record_event("open_app", Some("spotify")).await.unwrap();
        s.detect_patterns(&cfg).await.unwrap();
        assert!(s.take_next_proposed(&cfg).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn eventos_sin_firma_no_generan_sugerencias() {
        let s = store().await;
        let cfg = test_cfg();
        for _ in 0..5 {
            s.record_event("web_search", None).await.unwrap();
        }
        s.detect_patterns(&cfg).await.unwrap();
        assert!(s.take_next_proposed(&cfg).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rechazada_no_se_reofrece_dentro_del_cooldown() {
        let s = store().await;
        let cfg = test_cfg();
        for _ in 0..3 {
            s.record_event("open_app", Some("spotify")).await.unwrap();
        }
        s.detect_patterns(&cfg).await.unwrap();
        let id = s.take_next_proposed(&cfg).await.unwrap().unwrap().id;
        s.resolve(id, false).await.unwrap();

        // Vuelve a ocurrir el mismo patrón; el cooldown (30 días) sigue vigente.
        s.record_event("open_app", Some("spotify")).await.unwrap();
        s.detect_patterns(&cfg).await.unwrap();
        assert!(s.take_next_proposed(&cfg).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn take_next_proposed_marca_offered_y_respeta_max_per_day() {
        let s = store().await;
        let mut cfg = test_cfg();
        cfg.max_per_day = 1;
        for _ in 0..3 {
            s.record_event("open_app", Some("spotify")).await.unwrap();
        }
        for _ in 0..3 {
            s.record_event("open_url", Some("youtube.com"))
                .await
                .unwrap();
        }
        s.detect_patterns(&cfg).await.unwrap();

        let first = s.take_next_proposed(&cfg).await.unwrap();
        assert!(first.is_some());
        // El tope diario ya se alcanzó: no debe entregar una segunda, aunque
        // haya otro patrón "proposed" esperando.
        let second = s.take_next_proposed(&cfg).await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn resolve_id_inexistente_devuelve_false() {
        let s = store().await;
        assert!(!s.resolve(999, true).await.unwrap());
    }
}
