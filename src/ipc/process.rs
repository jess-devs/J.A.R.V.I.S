//! `WorkerHandle`: envoltorio genérico sobre un proceso hijo Python hablando
//! el protocolo NDJSON (+ bytes crudos opcionales) definido en `framing`.
//!
//! Cada worker spawneado corre con 3 tareas tokio dedicadas: un lector de
//! stdout (produce `WorkerFrame`s a un channel), un reenviador de stderr
//! (hacia `tracing`, para que los logs de Python aparezcan junto a los de
//! Rust) y un vigía de salida que detecta muerte inesperada del proceso.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};

use crate::errors::WorkerError;
use crate::ipc::framing::{read_binary, read_message, write_message};

/// Un mensaje recibido por stdout del worker, con o sin un bloque de bytes
/// crudos adjunto (declarado por el propio worker vía el campo `"bytes"`).
#[derive(Debug, Clone)]
pub enum WorkerFrame {
    Message(serde_json::Value),
    MessageWithBytes(serde_json::Value, Vec<u8>),
}

pub struct WorkerHandle {
    name: &'static str,
    stdin: Mutex<tokio::process::ChildStdin>,
    died: Arc<AtomicBool>,
    kill_tx: mpsc::Sender<()>,
}

impl WorkerHandle {
    /// Spawnea `python_executable script`, devolviendo el handle (para
    /// enviar mensajes) y el receiver de frames leídos de stdout.
    ///
    /// `extra_env` se aplica encima de `JARVIS_RUNTIME_DIR`, sin que este
    /// módulo necesite saber para qué la usa cada caller (ej. `PATH`/
    /// `LD_LIBRARY_PATH` con las DLLs de CUDA para el worker STT, ver
    /// `src/stt/mod.rs`) — mantiene `process.rs` genérico entre workers.
    pub async fn spawn(
        name: &'static str,
        python_executable: &Path,
        script: &Path,
        runtime_dir: &Path,
        extra_env: &[(&str, OsString)],
    ) -> Result<(Self, mpsc::Receiver<WorkerFrame>), WorkerError> {
        // `JARVIS_RUNTIME_DIR` es la única fuente de verdad que ven los
        // workers Python para dónde cae todo lo que escriben (cache, logs).
        // Se pasa como env en vez de vía cwd: los workers deciden ellos
        // mismos cuándo (si acaso) hacer `os.chdir()`, después de que el
        // intérprete ya resolvió su propio `__file__` — ver
        // docs/ARCHITECTURE.md sobre por qué no usar
        // `Command::current_dir` acá (rompería la resolución de
        // `python_executable`/`script` cuando son relativos).
        //
        // Se exporta absoluto (best-effort: si falla, se manda tal cual)
        // para que siga siendo válido incluso después de que un worker haga
        // `os.chdir()` con él — relativo, una segunda lectura de la env
        // después del chdir resolvería contra el cwd nuevo, no el original.
        let runtime_dir_abs =
            std::path::absolute(runtime_dir).unwrap_or_else(|_| runtime_dir.to_path_buf());
        let mut command = Command::new(python_executable);
        command
            .arg(script)
            .env("JARVIS_RUNTIME_DIR", runtime_dir_abs);
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| WorkerError::Spawn {
                executable: python_executable.to_path_buf(),
                source,
            })?;

        if let Some(pid) = child.id() {
            crate::ipc::watchdog::register_worker_pid(pid);
        }

        let stdin = child
            .stdin
            .take()
            .expect("stdin fue pedido con Stdio::piped()");
        let stdout = child
            .stdout
            .take()
            .expect("stdout fue pedido con Stdio::piped()");
        let stderr = child
            .stderr
            .take()
            .expect("stderr fue pedido con Stdio::piped()");

        let (frame_tx, frame_rx) = mpsc::channel::<WorkerFrame>(64);
        let died = Arc::new(AtomicBool::new(false));
        let (kill_tx, mut kill_rx) = mpsc::channel::<()>(1);

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message::<serde_json::Value>(&mut reader).await {
                    Ok(Some(value)) => {
                        let extra_bytes = value.get("bytes").and_then(|b| b.as_u64());
                        let frame = if let Some(n) = extra_bytes {
                            match read_binary(&mut reader, n as usize).await {
                                Ok(bytes) => WorkerFrame::MessageWithBytes(value, bytes),
                                Err(error) => {
                                    tracing::error!(worker = name, %error, "error leyendo payload binario del worker");
                                    break;
                                }
                            }
                        } else {
                            WorkerFrame::Message(value)
                        };
                        if frame_tx.send(frame).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::error!(worker = name, %error, "error de protocolo leyendo stdout del worker");
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(worker = name, "{line}");
            }
        });

        tokio::spawn({
            let died = died.clone();
            async move {
                let mut child = child;
                tokio::select! {
                    status = child.wait() => {
                        died.store(true, Ordering::SeqCst);
                        match status {
                            Ok(s) => tracing::warn!(worker = name, code = ?s.code(), "el worker terminó"),
                            Err(error) => tracing::error!(worker = name, %error, "error esperando la salida del worker"),
                        }
                    }
                    _ = kill_rx.recv() => {
                        died.store(true, Ordering::SeqCst);
                        let _ = child.kill().await;
                    }
                }
            }
        });

        Ok((
            Self {
                name,
                stdin: Mutex::new(stdin),
                died,
                kill_tx,
            },
            frame_rx,
        ))
    }

    /// Serializa `msg` como NDJSON y lo escribe al stdin del worker.
    pub async fn send<T: Serialize>(&self, msg: &T) -> Result<(), WorkerError> {
        let mut stdin = self.stdin.lock().await;
        write_message(&mut *stdin, msg)
            .await
            .map_err(|e| WorkerError::Protocol(e.to_string()))
    }

    pub fn is_alive(&self) -> bool {
        !self.died.load(Ordering::SeqCst)
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Fuerza la terminación del proceso (usado cuando `shutdown` por
    /// protocolo no obtuvo respuesta a tiempo).
    pub async fn kill(&self) {
        let _ = self.kill_tx.send(()).await;
    }
}
