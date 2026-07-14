//! Registro de PIDs de los workers Python, usado por el panic hook de
//! `main.rs` para limpiarlos de forma síncrona (sin `.await`) cuando un
//! panic termina el proceso. El Job Object (`job_object.rs`) es la garantía
//! real; esto solo acelera la limpieza y dan mejor rastro en los logs.

use std::sync::{Mutex, OnceLock};

static WORKER_PIDS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<u32>> {
    WORKER_PIDS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn register_worker_pid(pid: u32) {
    registry().lock().expect("registry lock envenenado").push(pid);
}

/// Mata (síncronamente) todos los PIDs registrados. Pensado para llamarse
/// desde un panic hook, donde no hay runtime de tokio disponible para
/// `.await` ni motivo para usar `spawn_blocking`.
pub fn kill_known_workers_sync() {
    use sysinfo::System;
    let pids = registry().lock().expect("registry lock envenenado").clone();
    if pids.is_empty() {
        return;
    }
    let sys = System::new_all();
    for pid in pids {
        if let Some(process) = sys.process(sysinfo::Pid::from_u32(pid)) {
            process.kill();
        }
    }
}
