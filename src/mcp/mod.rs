//! Cliente MCP (Model Context Protocol): Jarvis se conecta a servidores
//! externos configurados (`mcp:` en config.yaml) y sus tools se pliegan al
//! mismo `ToolRegistry` que las built-in y las `scripted` de `create_tool`.
//!
//! Complementa `create_tool`, no lo reemplaza: `create_tool` es para
//! automatización personal instantánea (una receta HTTP/PowerShell que el
//! usuario arma por voz en el momento); un servidor MCP es una integración
//! curada y mantenida por terceros (auth, paginación, estado) — ver README.
//!
//! Solo transporte stdio (proceso hijo local) en esta versión; HTTP/SSE
//! queda para una etapa posterior. La conexión es best-effort: un servidor
//! que falla al conectar o listar tools solo genera un warning, nunca
//! bloquea el arranque de Jarvis (a diferencia de los workers Python STT/
//! TTS, que sí son críticos).

pub mod client;
mod tool_adapter;

pub use client::discover_tools;
