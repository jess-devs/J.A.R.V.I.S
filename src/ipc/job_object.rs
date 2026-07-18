//! Agrupa el proceso Jarvis (y todo lo que este lance después) en un Windows
//! Job Object con `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: cuando el kernel
//! cierra la última referencia al job — es decir, cuando Jarvis desaparece,
//! sea cual sea la causa (crash, panic, "Finalizar tarea", cierre de
//! consola) — Windows mata automáticamente a todos los procesos del job,
//! incluidos los workers Python. Esto no depende de que Rust ejecute ningún
//! código de limpieza.
//!
//! Los procesos hijos heredan la membresía del job al ser creados (mientras
//! el proceso padre ya sea miembro y no se use `CREATE_BREAKAWAY_FROM_JOB`,
//! que `WorkerHandle::spawn` no usa), así que basta con crear el job y
//! asignarle el proceso actual antes del primer spawn de un worker.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Handle al Job Object. Debe mantenerse vivo durante toda la vida del
/// proceso — y como el proceso actual es miembro del job, NUNCA debe
/// dropearse antes de terminar: cerrar el último handle es exactamente lo
/// que dispara kill-on-close, y mataría a todos los miembros del job,
/// incluido este mismo proceso (en silencio y con código de salida 0). Por
/// eso `main` lo `std::mem::forget`-ea: el kernel cierra el handle al morir
/// el proceso, y recién ahí arrastra a los workers.
pub struct JobObject {
    handle: HANDLE,
}

impl JobObject {
    /// Crea un Job Object anónimo con `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` y
    /// le asigna el proceso actual.
    pub fn create_and_assign_current_process() -> windows::core::Result<Self> {
        let handle = unsafe { CreateJobObjectW(None, None) }?;

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            AssignProcessToJobObject(handle, GetCurrentProcess())?;
        }

        Ok(Self { handle })
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

// Seguro: el handle no se comparte ni se usa concurrentemente desde otros
// hilos; solo se guarda para mantenerlo vivo hasta el final del proceso.
unsafe impl Send for JobObject {}
