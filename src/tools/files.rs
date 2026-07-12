//! Buscar y abrir archivos. `find_files` usa Everything CLI (es.exe) si está
//! configurado (instantáneo, indexa todo el disco); si no, recorre con
//! walkdir las carpetas de `agent.files.search_roots` con presupuesto de
//! tiempo. `open_file` abre con la aplicación asociada (equivale a doble
//! click).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::FilesToolConfig;
use crate::errors::ToolError;

use super::{required_str, RiskLevel, Tool, ToolOutput};

/// Carpetas que no vale la pena recorrer: enormes y sin archivos del usuario.
const SKIP_DIRS: [&str; 6] = [
    "node_modules",
    ".git",
    "target",
    ".venv",
    "__pycache__",
    "AppData",
];

const WALK_BUDGET: Duration = Duration::from_secs(5);

pub struct FindFiles {
    cfg: FilesToolConfig,
}

impl FindFiles {
    pub fn new(cfg: &FilesToolConfig) -> Self {
        Self { cfg: cfg.clone() }
    }

    async fn search_with_everything(&self, es: &PathBuf, query: &str) -> Result<String, ToolError> {
        let output = tokio::process::Command::new(es)
            .args(["-n", &self.cfg.max_results.to_string(), query])
            .output()
            .await
            .map_err(|e| ToolError::Execution(format!("no se pudo ejecutar es.exe: {e}")))?;
        if !output.status.success() {
            return Err(ToolError::Execution(
                "es.exe devolvió un error (¿está corriendo Everything?)".to_string(),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let hits: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
        Ok(format_hits(query, &hits))
    }

    fn search_with_walkdir(&self, query: String) -> String {
        let needle = query.to_lowercase();
        let start = Instant::now();
        let mut hits: Vec<String> = Vec::new();

        'roots: for root in &self.cfg.search_roots {
            let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir()
                    && (name.starts_with('.') || SKIP_DIRS.iter().any(|d| name == *d)))
            });
            for entry in walker.flatten() {
                if start.elapsed() > WALK_BUDGET || hits.len() >= self.cfg.max_results {
                    break 'roots;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                if entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&needle)
                {
                    hits.push(entry.path().display().to_string());
                }
            }
        }
        let refs: Vec<&str> = hits.iter().map(String::as_str).collect();
        format_hits(&query, &refs)
    }
}

fn format_hits(query: &str, hits: &[&str]) -> String {
    if hits.is_empty() {
        return format!("No encontré archivos que coincidan con '{query}'.");
    }
    let mut out = format!("Archivos que coinciden con '{query}':\n");
    for hit in hits {
        out.push_str(&format!("- {hit}\n"));
    }
    out
}

#[async_trait]
impl Tool for FindFiles {
    fn name(&self) -> &'static str {
        "find_files"
    }

    fn description(&self) -> &'static str {
        "Busca archivos por nombre (coincidencia parcial) en las carpetas del \
         usuario y devuelve sus rutas completas."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Texto a buscar dentro del nombre de archivo"
                }
            },
            "required": ["query"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("?");
        format!("buscar archivos con '{query}'")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let query = required_str(&args, "query")?.to_string();
        if let Some(es) = self.cfg.everything_cli.as_ref().filter(|p| p.exists()) {
            return self
                .search_with_everything(es, &query)
                .await
                .map(ToolOutput::text);
        }
        let this = Self {
            cfg: self.cfg.clone(),
        };
        tokio::task::spawn_blocking(move || this.search_with_walkdir(query))
            .await
            .map(|text| Ok(ToolOutput::text(text)))
            .unwrap_or_else(|e| Err(ToolError::Execution(e.to_string())))
    }
}

pub struct OpenFile;

#[async_trait]
impl Tool for OpenFile {
    fn name(&self) -> &'static str {
        "open_file"
    }

    fn description(&self) -> &'static str {
        "Abre un archivo o carpeta con su aplicación por defecto (como doble \
         click). Requiere la ruta completa — usa find_files primero si no la \
         conoces."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Ruta completa del archivo o carpeta"
                }
            },
            "required": ["path"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("?");
        format!("abrir el archivo {path}")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let path = PathBuf::from(required_str(&args, "path")?);
        if !path.exists() {
            return Err(ToolError::InvalidArgs(format!(
                "la ruta no existe: {}",
                path.display()
            )));
        }
        let status = tokio::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(&path)
            .status()
            .await
            .map_err(|e| ToolError::Execution(format!("no se pudo abrir: {e}")))?;
        if status.success() {
            Ok(ToolOutput::text(format!("Abierto: {}.", path.display())))
        } else {
            Err(ToolError::Execution(format!(
                "Windows no pudo abrir {}.",
                path.display()
            )))
        }
    }
}
