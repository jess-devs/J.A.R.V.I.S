//! Captura de pantalla (para darle contexto visual al LLM) y control de
//! mouse, vía `xcap` (captura) y `SendInput`/`SetCursorPos` (input,
//! `input.rs`). Coordenadas en píxeles físicos, el mismo sistema que usa
//! `xcap` para reportar screenshots — así el LLM puede clickear directo
//! sobre lo que ve en la imagen sin convertir por DPI.

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use crate::errors::ToolError;
use crate::llm::ImageBlock;

use super::input::{click_mouse, move_cursor, MouseButton};
use super::{RiskLevel, Tool, ToolOutput};

pub struct TakeScreenshot;

#[async_trait]
impl Tool for TakeScreenshot {
    fn name(&self) -> &'static str {
        "take_screenshot"
    }

    fn description(&self) -> &'static str {
        "Captura una imagen de la pantalla actual (monitor primario) para \
         que puedas ver lo que el usuario tiene abierto. Úsala antes de \
         mouse_click/click_at para saber dónde clickear."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // Puede exponer contenido sensible en pantalla (contraseñas,
        // mensajes privados): confirmación por voz, no ejecución directa.
        RiskLevel::Confirm
    }

    fn describe_action(&self, _args: &Value) -> String {
        "tomar una captura de la pantalla".to_string()
    }

    async fn execute(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        let (width, height, png_bytes) = tokio::task::spawn_blocking(|| -> Result<_, ToolError> {
            let monitors = xcap::Monitor::all()
                .map_err(|e| ToolError::Execution(format!("no se pudo listar monitores: {e}")))?;
            let monitor = monitors
                .into_iter()
                .find(|m| m.is_primary().unwrap_or(false))
                .or_else(|| xcap::Monitor::all().ok().and_then(|m| m.into_iter().next()))
                .ok_or_else(|| ToolError::Execution("no se encontró ningún monitor".to_string()))?;
            let image = monitor.capture_image().map_err(|e| {
                ToolError::Execution(format!("no se pudo capturar la pantalla: {e}"))
            })?;
            let (width, height) = (image.width(), image.height());
            let mut png_bytes: Vec<u8> = Vec::new();
            image::DynamicImage::ImageRgba8(image)
                .write_to(
                    &mut std::io::Cursor::new(&mut png_bytes),
                    image::ImageFormat::Png,
                )
                .map_err(|e| {
                    ToolError::Execution(format!("no se pudo codificar la imagen: {e}"))
                })?;
            Ok((width, height, png_bytes))
        })
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))??;

        let base64_data = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let text = format!("Captura de pantalla tomada ({width}x{height} px).");
        Ok(ToolOutput::with_images(
            text,
            vec![ImageBlock {
                media_type: "image/png".to_string(),
                base64_data,
            }],
        ))
    }
}

pub struct MouseMove;

#[async_trait]
impl Tool for MouseMove {
    fn name(&self) -> &'static str {
        "mouse_move"
    }

    fn description(&self) -> &'static str {
        "Mueve el cursor del mouse a coordenadas absolutas de pantalla \
         (píxeles físicos, como en take_screenshot), sin hacer click."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "x": { "type": "integer", "description": "Coordenada X en píxeles" },
                "y": { "type": "integer", "description": "Coordenada Y en píxeles" }
            },
            "required": ["x", "y"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // Solo mover el cursor no tiene efecto por sí solo.
        RiskLevel::Safe
    }

    fn describe_action(&self, args: &Value) -> String {
        let x = args.get("x").and_then(Value::as_i64).unwrap_or(0);
        let y = args.get("y").and_then(Value::as_i64).unwrap_or(0);
        format!("mover el cursor a ({x}, {y})")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let (x, y) = xy_from_args(&args)?;
        move_cursor(x, y).await?;
        Ok(ToolOutput::text(format!("Cursor movido a ({x}, {y}).")))
    }
}

pub struct MouseClick;

#[async_trait]
impl Tool for MouseClick {
    fn name(&self) -> &'static str {
        "mouse_click"
    }

    fn description(&self) -> &'static str {
        "Hace click con el mouse en la posición actual del cursor. Usa \
         click_at si necesitas mover el cursor y clickear en un solo paso."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "button": {
                    "type": "string",
                    "enum": ["left", "right"],
                    "description": "Botón a usar (default 'left')"
                }
            }
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        // Un click puede disparar cualquier acción en pantalla: confirmación
        // por voz (no el código de riesgo completo, para no ser tedioso en
        // uso seguido).
        RiskLevel::Confirm
    }

    fn describe_action(&self, args: &Value) -> String {
        let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
        format!("hacer click ({button}) en la posición actual del cursor")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let button = button_from_args(&args)?;
        click_mouse(button).await?;
        Ok(ToolOutput::text("Click realizado."))
    }
}

pub struct ClickAt;

#[async_trait]
impl Tool for ClickAt {
    fn name(&self) -> &'static str {
        "click_at"
    }

    fn description(&self) -> &'static str {
        "Mueve el cursor a coordenadas absolutas de pantalla (píxeles \
         físicos, como en take_screenshot) y hace click ahí."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "x": { "type": "integer", "description": "Coordenada X en píxeles" },
                "y": { "type": "integer", "description": "Coordenada Y en píxeles" },
                "button": {
                    "type": "string",
                    "enum": ["left", "right"],
                    "description": "Botón a usar (default 'left')"
                }
            },
            "required": ["x", "y"]
        })
    }

    fn assess_risk(&self, _args: &Value) -> RiskLevel {
        RiskLevel::Confirm
    }

    fn describe_action(&self, args: &Value) -> String {
        let x = args.get("x").and_then(Value::as_i64).unwrap_or(0);
        let y = args.get("y").and_then(Value::as_i64).unwrap_or(0);
        let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
        format!("hacer click ({button}) en ({x}, {y})")
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let (x, y) = xy_from_args(&args)?;
        let button = button_from_args(&args)?;
        move_cursor(x, y).await?;
        click_mouse(button).await?;
        Ok(ToolOutput::text(format!(
            "Click ({button:?}) en ({x}, {y}) realizado."
        )))
    }
}

fn xy_from_args(args: &Value) -> Result<(i32, i32), ToolError> {
    let x = args
        .get("x")
        .and_then(Value::as_i64)
        .ok_or_else(|| ToolError::InvalidArgs("falta el parámetro 'x'".to_string()))?;
    let y = args
        .get("y")
        .and_then(Value::as_i64)
        .ok_or_else(|| ToolError::InvalidArgs("falta el parámetro 'y'".to_string()))?;
    Ok((x as i32, y as i32))
}

fn button_from_args(args: &Value) -> Result<MouseButton, ToolError> {
    match args.get("button").and_then(Value::as_str).unwrap_or("left") {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        other => Err(ToolError::InvalidArgs(format!(
            "botón de mouse desconocido: '{other}'"
        ))),
    }
}
