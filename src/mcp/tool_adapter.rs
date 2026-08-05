//! Envuelve una tool descubierta de un servidor MCP como `Arc<dyn Tool>`,
//! el mismo tipo que usan las tools built-in y las `scripted`.

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::RunningService;
use rmcp::RoleClient;
use serde_json::Value;

use crate::errors::ToolError;
use crate::llm::ImageBlock;
use crate::tools::{RiskLevel, Tool, ToolOutput};

pub struct McpTool {
    server_name: String,
    tool: rmcp::model::Tool,
    client: Arc<RunningService<RoleClient, ()>>,
    /// Decidido una sola vez al descubrir la tool, según
    /// `mcp[].trusted_tools` — nunca `Code`: Jarvis no tiene forma de saber
    /// de antemano qué tan destructiva es una tool de un tercero, así que
    /// el techo de riesgo es `Confirm` (ver `client::discover_tools`).
    risk: RiskLevel,
}

impl McpTool {
    pub fn new(
        server_name: String,
        tool: rmcp::model::Tool,
        client: Arc<RunningService<RoleClient, ()>>,
        risk: RiskLevel,
    ) -> Self {
        Self {
            server_name,
            tool,
            client,
            risk,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.tool.name
    }

    fn description(&self) -> &str {
        self.tool.description.as_deref().unwrap_or("")
    }

    fn parameters_schema(&self) -> Value {
        serde_json::to_value(&*self.tool.input_schema)
            .unwrap_or_else(|_| serde_json::json!({ "type": "object", "properties": {} }))
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        self.risk
    }

    fn describe_action(&self, _args: &Value) -> String {
        format!(
            "ejecutar la tool '{}' del servidor MCP '{}'",
            self.tool.name, self.server_name
        )
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut params = CallToolRequestParams::new(self.tool.name.clone());
        if let Value::Object(map) = args {
            if !map.is_empty() {
                params = params.with_arguments(map);
            }
        }

        let result = self.client.call_tool(params).await.map_err(|e| {
            ToolError::Execution(format!("servidor MCP '{}': {e}", self.server_name))
        })?;

        let mut text_parts = Vec::new();
        let mut images = Vec::new();
        for block in result.content {
            match block {
                ContentBlock::Text(t) => text_parts.push(t.text),
                ContentBlock::Image(img) => images.push(ImageBlock {
                    media_type: img.mime_type,
                    base64_data: img.data,
                }),
                // Audio/Resource/ResourceLink: sin representación en
                // ToolOutput hoy (solo texto + imágenes) — se ignoran en
                // vez de fallar toda la respuesta por un bloque que Jarvis
                // no puede mostrar.
                _ => {}
            }
        }
        let text = text_parts.join("\n");

        if result.is_error == Some(true) {
            return Err(ToolError::Execution(if text.is_empty() {
                format!("la tool '{}' devolvió un error", self.tool.name)
            } else {
                text
            }));
        }

        Ok(ToolOutput::with_images(text, images))
    }
}
