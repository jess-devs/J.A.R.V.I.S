//! Ruta WebSocket de calibración de micrófono (`/api/onboarding/calibration/ws`):
//! túnel NDJSON-sobre-WS entre el navegador y un `CalibrationWorker` propio,
//! spawneado bajo demanda para cada conexión. Ver
//! `docs/planning/onboarding-mic-calibracion.md` para el contexto completo.

use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use serde::Deserialize;

use crate::config::Config;
use crate::stt::{CalibrationEvent, CalibrationWorker};

use super::AppState;

/// Si no llega ningún mensaje del cliente en este tiempo, se asume una
/// desconexión silenciosa (laptop suspendida, wifi caído sin frame de
/// cierre WS) y se cierra el socket + el worker — acota cuánto tiempo puede
/// quedar un stream de audio abierto sin que nadie lo esté mirando. No es
/// la única red de seguridad: si este loop termina por cualquier otro
/// motivo, `CalibrationWorker::shutdown` igual se llama al salir, y aunque
/// no se llamara, el reaper interno de `WorkerHandle` limpia el proceso al
/// dropearse.
const CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Duración de la grabación de enrollment si el cliente no especifica
/// `seconds` — mismo default que `_cli_enroll_voice` en `stt_worker.py`.
const DEFAULT_ENROLL_SECONDS: f32 = 5.0;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    ListDevices,
    StartCalibration { device_index: Option<u32> },
    StopCalibration,
    StartEnroll { device_index: Option<u32>, seconds: Option<f32> },
    CancelEnroll,
}

pub(super) async fn calibration_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn send_error(socket: &mut WebSocket, code: &str, message: impl std::fmt::Display) {
    let payload = serde_json::json!({"type": "error", "code": code, "message": message.to_string()});
    let _ = socket.send(WsMessage::Text(payload.to_string().into())).await;
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let config = match Config::load(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            send_error(&mut socket, "config_load_failed", e).await;
            return;
        }
    };

    let mut worker = match CalibrationWorker::spawn(&config.workers, config.runtime_dir()).await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "no se pudo arrancar el worker de calibración");
            send_error(&mut socket, "spawn_failed", e).await;
            return;
        }
    };

    loop {
        tokio::select! {
            incoming = tokio::time::timeout(CLIENT_IDLE_TIMEOUT, socket.recv()) => {
                let Ok(incoming) = incoming else {
                    tracing::debug!("calibración: cliente inactivo, cerrando");
                    break;
                };
                match incoming {
                    Some(Ok(WsMessage::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::ListDevices) => {
                                if worker.list_devices().await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::StartCalibration { device_index }) => {
                                if worker.start(device_index).await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::StopCalibration) => {
                                if worker.stop().await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::StartEnroll { device_index, seconds }) => {
                                let seconds = seconds.unwrap_or(DEFAULT_ENROLL_SECONDS);
                                if worker.start_enroll(device_index, seconds).await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::CancelEnroll) => {
                                if worker.cancel_enroll().await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, mensaje = %text, "mensaje de calibración no reconocido desde el navegador");
                            }
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        tracing::debug!(error = %e, "error leyendo el WebSocket de calibración");
                        break;
                    }
                }
            }
            event = worker.next_event() => {
                match event {
                    None | Some(CalibrationEvent::Died) => break,
                    Some(event) => {
                        let Ok(json) = serde_json::to_string(&event) else { continue };
                        if socket.send(WsMessage::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }

    worker.shutdown().await;
}
