//! Conexión a los servidores MCP configurados y descubrimiento de sus
//! tools. Ver el doc del módulo (`mod.rs`) para el porqué del diseño
//! best-effort.

use std::collections::HashSet;
use std::sync::Arc;

use rmcp::service::RunningService;
use rmcp::transport::{which_command, ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};

use crate::config::McpServerConfig;
use crate::tools::{RiskLevel, Tool};

use super::tool_adapter::McpTool;

/// Conecta a cada servidor MCP configurado, lista sus tools y las envuelve
/// como `Arc<dyn Tool>` listas para sumarse a `ToolRegistry::static_tools`.
///
/// `reserved_names` son los nombres ya usados por tools built-in/scripted:
/// una tool MCP que colisione (con una reservada o con otra tool MCP ya
/// descubierta) se descarta con un warning en vez de pisar algo existente.
pub async fn discover_tools(
    servers: &[McpServerConfig],
    reserved_names: &HashSet<String>,
) -> Vec<Arc<dyn Tool>> {
    let mut discovered: Vec<Arc<dyn Tool>> = Vec::new();
    let mut seen: HashSet<String> = reserved_names.clone();

    for server in servers {
        if server.command.trim().is_empty() {
            tracing::warn!(name = %server.name, "servidor MCP sin `command`, se ignora");
            continue;
        }

        let client = match connect(server).await {
            Ok(c) => Arc::new(c),
            Err(e) => {
                tracing::warn!(server = %server.name, error = %e, "no se pudo conectar al servidor MCP, se ignora");
                continue;
            }
        };

        let tools = match client.list_all_tools().await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(server = %server.name, error = %e, "no se pudieron listar las tools del servidor MCP, se ignora");
                continue;
            }
        };

        let mut added = 0usize;
        for tool in tools {
            let name = tool.name.to_string();
            if !seen.insert(name.clone()) {
                tracing::warn!(server = %server.name, tool = %name, "nombre de tool MCP ya usado por otra tool, se ignora");
                continue;
            }
            let risk = if server.trusted_tools.iter().any(|t| t == &name) {
                RiskLevel::Safe
            } else {
                RiskLevel::Confirm
            };
            discovered.push(Arc::new(McpTool::new(
                server.name.clone(),
                tool,
                client.clone(),
                risk,
            )));
            added += 1;
        }
        tracing::info!(server = %server.name, tools = added, "servidor MCP conectado");
    }

    discovered
}

async fn connect(
    server: &McpServerConfig,
) -> Result<RunningService<RoleClient, ()>, Box<dyn std::error::Error + Send + Sync>> {
    let args = server.args.clone();
    let env = server.env.clone();
    // `which_command` resuelve el ejecutable contra el PATH del sistema
    // antes de lanzarlo: en Windows, `Command::new("npx")` no encuentra de
    // forma confiable los shims `.cmd`/`.bat` (npx, uvx, etc. son scripts
    // así) sin la ruta completa.
    let command = which_command(&server.command)?.configure(|cmd| {
        cmd.args(&args).envs(&env);
    });
    let transport = TokioChildProcess::new(command)?;
    let client = ().serve(transport).await?;
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prueba de integración real contra el servidor MCP de referencia
    /// (`@modelcontextprotocol/server-everything`, vía `npx`), no una
    /// simulación: verifica que `discover_tools` conecta de verdad por
    /// stdio, lista tools y respeta `trusted_tools`/colisión de nombres.
    /// Ignorado por defecto (necesita red + Node.js/npx) — correr con
    /// `cargo test --ignored mcp::client::tests -- --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn discover_tools_conecta_a_un_servidor_mcp_real() {
        let server = McpServerConfig {
            name: "everything".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-everything".to_string(),
            ],
            env: Default::default(),
            trusted_tools: vec!["echo".to_string()],
        };

        let tools = discover_tools(&[server], &HashSet::new()).await;
        assert!(
            !tools.is_empty(),
            "el servidor de referencia expone varias tools (echo, add, ...)"
        );

        let echo = tools
            .iter()
            .find(|t| t.name() == "echo")
            .expect("el servidor de referencia expone una tool 'echo'");
        assert_eq!(
            echo.assess_risk(&serde_json::json!({})),
            RiskLevel::Safe,
            "'echo' está en trusted_tools, debería ser Safe"
        );

        let others_are_confirm = tools
            .iter()
            .filter(|t| t.name() != "echo")
            .all(|t| t.assess_risk(&serde_json::json!({})) == RiskLevel::Confirm);
        assert!(
            others_are_confirm,
            "las tools MCP no confiadas deberían ser Confirm por default"
        );

        let output = echo
            .execute(serde_json::json!({ "message": "hola jarvis" }))
            .await
            .expect("echo no debería fallar");
        assert!(
            output.text.contains("hola jarvis"),
            "esperaba que el eco devolviera el mensaje, resultado: {}",
            output.text
        );
    }
}
