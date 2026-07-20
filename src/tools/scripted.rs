//! Tools creadas dinámicamente en tiempo de ejecución y persistidas en
//! SQLite (`scripted_store.rs`). Como el binario es Rust compilado, no hay
//! compilación dinámica real: una tool "scripted" es una plantilla
//! (PowerShell o HTTP) con placeholders `{param}` sustituidos desde los
//! argumentos que pasa el LLM al invocarla.
//!
//! Tres meta-tools la manejan: `create_tool` (siempre `Code`: define una
//! plantilla que luego ejecuta comandos/HTTP arbitrarios), `list_custom_tools`
//! (`Safe`) y `delete_custom_tool` (`Confirm`). Una tool creada nunca hereda
//! `Safe`: como mínimo `Confirm`, forzado en `CreateTool::execute`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::ScriptedToolsConfig;
use crate::errors::ToolError;

use super::scripted_store::{ScriptedToolDef, ScriptedToolStore};
use super::shell::run_powershell_command;
use super::{required_str, RiskLevel, Tool, ToolOutput};

pub const BUILTIN_NAMES: &[&str] = &[
    "get_datetime",
    "system_status",
    "list_processes",
    "open_app",
    "open_url",
    "close_app",
    "find_files",
    "open_file",
    "run_powershell",
    "get_volume",
    "set_volume",
    "web_search",
    "fetch_page",
    "remember",
    "recall",
    "forget",
    "translate",
    "media_control",
    "create_reminder",
    "list_reminders",
    "cancel_reminder",
    "take_screenshot",
    "mouse_move",
    "mouse_click",
    "click_at",
    "create_tool",
    "list_custom_tools",
    "delete_custom_tool",
    "accept_suggestion",
    "dismiss_suggestion",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolRecipe {
    Powershell {
        command_template: String,
    },
    Http {
        /// GET, POST, PUT, DELETE...
        method: String,
        url_template: String,
        #[serde(default)]
        body_template: Option<String>,
    },
}

/// Sustituye placeholders `{param}` en `template` con los argumentos JSON
/// recibidos. Error si falta algún placeholder referenciado.
fn substitute(template: &str, args: &Value) -> Result<String, ToolError> {
    let re = Regex::new(r"\{(\w+)\}").expect("regex de placeholders válida");
    let mut missing: Option<String> = None;
    let result = re.replace_all(template, |caps: &regex::Captures| {
        let key = &caps[1];
        match args.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => {
                missing = Some(key.to_string());
                String::new()
            }
        }
    });
    if let Some(key) = missing {
        return Err(ToolError::InvalidArgs(format!(
            "falta el parámetro '{key}' que la plantilla necesita"
        )));
    }
    Ok(result.into_owned())
}

/// Una tool creada por `create_tool`, cargada desde SQLite y registrada como
/// cualquier otra `Arc<dyn Tool>`.
pub struct ScriptedTool {
    def: ScriptedToolDef,
    http_client: reqwest::Client,
    allowed_hosts: Vec<String>,
}

impl ScriptedTool {
    pub fn new(def: ScriptedToolDef, cfg: &ScriptedToolsConfig) -> Self {
        Self {
            def,
            http_client: crate::http::client(Duration::from_secs(cfg.http_timeout_secs)),
            allowed_hosts: cfg.allowed_hosts.clone(),
        }
    }

    fn check_host_allowed(&self, url: &str) -> Result<(), ToolError> {
        if self.allowed_hosts.is_empty() {
            return Ok(());
        }
        let parsed = url::Url::parse(url)
            .map_err(|_| ToolError::InvalidArgs(format!("URL inválida: {url}")))?;
        let host = parsed.host_str().unwrap_or_default();
        if self.allowed_hosts.iter().any(|h| h == host) {
            Ok(())
        } else {
            Err(ToolError::Execution(format!(
                "el host '{host}' no está en la lista de hosts permitidos para tools personalizadas"
            )))
        }
    }
}

#[async_trait]
impl Tool for ScriptedTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn parameters_schema(&self) -> Value {
        self.def.parameters_schema.clone()
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // El nivel se fijó en la creación (create_tool) y nunca es Safe.
        self.def.risk_level
    }

    fn describe_action(&self, _args: &Value) -> String {
        format!("ejecutar la tool personalizada '{}'", self.def.name)
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        match &self.def.recipe {
            ToolRecipe::Powershell { command_template } => {
                let command = substitute(command_template, &args)?;
                let result = run_powershell_command(&command).await?;
                Ok(ToolOutput::text(result))
            }
            ToolRecipe::Http {
                method,
                url_template,
                body_template,
            } => {
                let url = substitute(url_template, &args)?;
                self.check_host_allowed(&url)?;
                let method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
                    .map_err(|_| {
                        ToolError::InvalidArgs(format!("método HTTP inválido: {method}"))
                    })?;
                let mut request = self.http_client.request(method, &url);
                if let Some(body_template) = body_template {
                    let body = substitute(body_template, &args)?;
                    request = request.body(body);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|e| ToolError::Execution(format!("error de red: {e}")))?;
                let status = response.status();
                let text = response.text().await.map_err(|e| {
                    ToolError::Execution(format!("error leyendo la respuesta: {e}"))
                })?;
                Ok(ToolOutput::text(format!("HTTP {status}: {text}")))
            }
        }
    }
}

pub struct CreateTool {
    store: Arc<ScriptedToolStore>,
    cfg: ScriptedToolsConfig,
}

impl CreateTool {
    pub fn new(store: Arc<ScriptedToolStore>, cfg: &ScriptedToolsConfig) -> Self {
        Self {
            store,
            cfg: cfg.clone(),
        }
    }
}

#[async_trait]
impl Tool for CreateTool {
    fn name(&self) -> &str {
        "create_tool"
    }

    fn description(&self) -> &str {
        "Crea una tool nueva y la persiste para el futuro: define un nombre, \
         descripción, parámetros y una receta de ejecución (un comando de \
         PowerShell con placeholders {param}, o una petición HTTP con \
         placeholders en la URL/body). Úsala solo cuando el usuario pida \
         explícitamente crear una nueva capacidad. La tool queda disponible \
         desde el próximo turno."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Nombre único de la tool (snake_case, sin espacios)"
                },
                "description": {
                    "type": "string",
                    "description": "Qué hace la tool, para que el LLM sepa cuándo usarla"
                },
                "parameters_schema": {
                    "type": "object",
                    "description": "JSON Schema del objeto de parámetros que recibirá la tool"
                },
                "risk_level": {
                    "type": "string",
                    "enum": ["confirm", "code"],
                    "description": "Nivel de riesgo: 'confirm' (pide sí/no) o 'code' (pide el código de riesgo). Nunca 'safe'."
                },
                "recipe_kind": {
                    "type": "string",
                    "enum": ["powershell", "http"]
                },
                "command_template": {
                    "type": "string",
                    "description": "Para recipe_kind=powershell: comando con placeholders {param}"
                },
                "http_method": {
                    "type": "string",
                    "description": "Para recipe_kind=http: GET, POST, etc."
                },
                "url_template": {
                    "type": "string",
                    "description": "Para recipe_kind=http: URL con placeholders {param}"
                },
                "body_template": {
                    "type": "string",
                    "description": "Para recipe_kind=http: body opcional con placeholders {param}"
                }
            },
            "required": ["name", "description", "parameters_schema", "risk_level", "recipe_kind"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // Define una plantilla que luego ejecutará comandos/HTTP arbitrarios:
        // la acción de mayor alcance de todo el sistema de tools.
        RiskLevel::Code
    }

    fn describe_action(&self, args: &Value) -> String {
        let name = args.get("name").and_then(Value::as_str).unwrap_or("?");
        format!("crear una nueva tool personalizada llamada '{name}'")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let name = required_str(&args, "name")?.to_string();
        if BUILTIN_NAMES.contains(&name.as_str()) {
            return Err(ToolError::InvalidArgs(format!(
                "'{name}' ya es el nombre de una tool incorporada, elegí otro"
            )));
        }
        let description = required_str(&args, "description")?.to_string();
        let parameters_schema = args
            .get("parameters_schema")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
        let risk_level = match required_str(&args, "risk_level")? {
            "code" => RiskLevel::Code,
            _ => RiskLevel::Confirm, // nunca Safe, aunque el LLM lo pida
        };
        let recipe_kind = required_str(&args, "recipe_kind")?;
        let recipe = match recipe_kind {
            "powershell" => ToolRecipe::Powershell {
                command_template: required_str(&args, "command_template")?.to_string(),
            },
            "http" => ToolRecipe::Http {
                method: args
                    .get("http_method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET")
                    .to_string(),
                url_template: required_str(&args, "url_template")?.to_string(),
                body_template: args
                    .get("body_template")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            other => {
                return Err(ToolError::InvalidArgs(format!(
                    "recipe_kind desconocido: '{other}' (usa 'powershell' o 'http')"
                )))
            }
        };

        let def = ScriptedToolDef {
            name: name.clone(),
            description,
            parameters_schema,
            risk_level,
            recipe,
        };
        self.store.create(&def, self.cfg.max_tools).await?;
        Ok(ToolOutput::text(format!(
            "Tool '{name}' creada. Estará disponible desde el próximo turno."
        )))
    }
}

pub struct ListCustomTools {
    store: Arc<ScriptedToolStore>,
}

impl ListCustomTools {
    pub fn new(store: Arc<ScriptedToolStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ListCustomTools {
    fn name(&self) -> &str {
        "list_custom_tools"
    }

    fn description(&self) -> &str {
        "Lista las tools personalizadas creadas con create_tool."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Safe
    }

    fn describe_action(&self, _args: &Value) -> String {
        "listar las tools personalizadas".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let defs = self.store.list().await?;
        if defs.is_empty() {
            return Ok(ToolOutput::text("No hay tools personalizadas creadas."));
        }
        let mut out = String::from("Tools personalizadas:\n");
        for d in defs {
            out.push_str(&format!("- {}: {}\n", d.name, d.description));
        }
        Ok(ToolOutput::text(out))
    }
}

pub struct DeleteCustomTool {
    store: Arc<ScriptedToolStore>,
}

impl DeleteCustomTool {
    pub fn new(store: Arc<ScriptedToolStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for DeleteCustomTool {
    fn name(&self) -> &str {
        "delete_custom_tool"
    }

    fn description(&self) -> &str {
        "Borra una tool personalizada por nombre (o coincidencia parcial). \
         El sistema pedirá confirmación al usuario."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Nombre (o parte del nombre) de la tool a borrar"
                }
            },
            "required": ["name"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Confirm
    }

    fn describe_action(&self, args: &Value) -> String {
        let name = args.get("name").and_then(Value::as_str).unwrap_or("?");
        format!("borrar la tool personalizada '{name}'")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let query = required_str(&args, "name")?;
        let deleted = self.store.delete_matching(query).await?;
        Ok(ToolOutput::text(if deleted == 0 {
            format!("No había tools personalizadas que coincidieran con '{query}'.")
        } else {
            format!("Borradas {deleted} tools personalizadas relacionadas con '{query}'. Efectivo desde el próximo turno.")
        }))
    }
}
