//! Primitivas de framing del protocolo IPC: NDJSON para mensajes de control,
//! con una extensión para leer un bloque de bytes crudos inmediatamente
//! después de un mensaje (usado por el TTS worker para el audio sintetizado).

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Lee una línea NDJSON y la parsea como `T`. `Ok(None)` significa EOF (el
/// worker cerró stdout, típicamente porque el proceso terminó).
pub async fn read_message<T: DeserializeOwned>(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> std::io::Result<Option<T>> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    let value = serde_json::from_str(line.trim_end())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(value))
}

/// Lee exactamente `len` bytes crudos (usado tras un header que declara
/// `"bytes": N`, ej. la respuesta de audio del TTS worker).
pub async fn read_binary(
    reader: &mut (impl AsyncRead + Unpin),
    len: usize,
) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Serializa `msg` como una línea NDJSON y la escribe (con flush).
pub async fn write_message<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    msg: &T,
) -> std::io::Result<()> {
    let mut line = serde_json::to_string(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
