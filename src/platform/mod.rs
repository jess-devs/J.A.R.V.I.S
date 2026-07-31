//! Primitivas de lanzamiento de procesos específicas de plataforma. Cada
//! función tiene una sola implementación activa según `cfg` (Windows vs.
//! Unix), así el resto del código (`tools/apps.rs`, `tools/files.rs`,
//! `tools/shell.rs`) no necesita `#[cfg]` disperso — solo llama a
//! `platform::open_target`/`run_shell_command` sin preguntarse el SO.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
