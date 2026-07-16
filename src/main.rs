mod agent;
mod audio;
mod config;
mod echo_gate;
mod errors;
mod http;
mod ipc;
mod llm;
mod memory;
mod orchestrator;
mod pipeline;
mod reminders;
mod startup_checks;
mod stt;
mod text;
mod tools;
mod tts;
mod tui;
mod wake;

use std::path::PathBuf;

use clap::Parser;

use config::Config;
use orchestrator::Orchestrator;
use tui::UiState;

#[derive(Parser)]
#[command(
    name = "jarvis",
    about = "Asistente de voz conversacional en tiempo real"
)]
struct Cli {
    /// Ruta al archivo de configuración.
    #[arg(long, default_value = "config.yaml")]
    config: PathBuf,

    /// Sobreescribe el nivel de log de config.yaml (ej. "debug", "jarvis=trace").
    #[arg(long)]
    log_level: Option<String>,
}

#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(|info| {
        tracing::error!(panic = %info, "panic no controlado en Jarvis");
        // Un panic en una tarea `tokio::spawn` no termina el proceso (no hay
        // `panic = "abort"` en el perfil): en ese caso Jarvis sigue vivo y
        // los workers deben seguir corriendo. Solo si el panic ocurrió en el
        // hilo principal el proceso va a morir, y ahí sí vale la pena
        // adelantar la limpieza (el Job Object es la red de seguridad final
        // de cualquier forma).
        if std::thread::current().name() == Some("main") {
            ipc::watchdog::kill_known_workers_sync();
        }
    }));

    let cli = Cli::parse();
    dotenvy::dotenv().ok();

    let config = match Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error de configuración: {e}");
            std::process::exit(1);
        }
    };

    let log_level = cli
        .log_level
        .clone()
        .unwrap_or_else(|| config.log_level.clone());

    // Con la TUI activa, la pantalla alterna le pertenece al holograma: los
    // logs de tracing van a un archivo en vez de la consola. `_log_guard`
    // debe vivir hasta el final de `main` (el writer no bloqueante de
    // tracing-appender depende de él para no perder líneas al salir).
    let _log_guard = if config.ui.enabled {
        let file_appender = tracing_appender::rolling::never("logs", "jarvis.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(log_level))
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();
        Some(guard)
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(log_level))
            .init();
        None
    };

    if let Err(e) = run(config).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run(mut config: Config) -> errors::Result<()> {
    // El Job Object y el handler de consola deben instalarse ANTES de crear
    // el Orchestrator: este spawnea los workers Python en su constructor, y
    // solo heredan la membresía del job si el proceso Jarvis ya es miembro
    // en el momento del spawn.
    let _job_object = ipc::job_object::JobObject::create_and_assign_current_process()
        .map_err(|e| {
            tracing::error!(
                error = %e,
                "no se pudo crear el Job Object de aislamiento; si Jarvis muere de forma anómala los workers Python podrían quedar huérfanos"
            );
        })
        .ok();

    let mut console_shutdown_rx = ipc::console_handler::install()
        .map_err(|e| tracing::warn!(error = %e, "no se pudo instalar el handler de cierre de consola"))
        .ok();

    llm::model_select::resolve(&mut config).await;
    startup_checks::run(&config).await?;

    let ui_config = config.ui.clone();
    let (ui_state, ui_state_rx) = UiState::new();

    let mut assistant = Orchestrator::new(config, ui_state).await?;

    // Cancela el loop de la TUI (si está activa) para que se llegue a
    // restaurar la terminal (raw mode/alternate screen) sin importar por qué
    // rama del `select!` de abajo se terminó cerrando Jarvis.
    let ui_shutdown = tokio_util::sync::CancellationToken::new();
    let mut ui_handle = if ui_config.enabled {
        let level_rx = assistant.audio_level_rx();
        let shutdown = ui_shutdown.clone();
        Some(tokio::spawn(tui::run(
            ui_config,
            ui_state_rx,
            level_rx,
            shutdown,
        )))
    } else {
        None
    };

    let result = tokio::select! {
        r = assistant.run() => r,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Señal de interrupción recibida, cerrando...");
            Ok(())
        }
        _ = async {
            match console_shutdown_rx.as_mut() {
                Some(rx) => { rx.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => {
            tracing::info!("Cierre de consola/logoff/apagado detectado, cerrando...");
            Ok(())
        }
        ui_result = join_optional(&mut ui_handle) => {
            tracing::info!("Interfaz cerrada, cerrando Jarvis...");
            match ui_result {
                Ok(Err(e)) => Err(e),
                _ => Ok(()),
            }
        }
    };

    // Si `ui_result` fue la rama que resolvió el `select!` de arriba, el
    // handle ya está completo (`join_optional` lo pollea vía `&mut`, no lo
    // consume) — repollearlo acá rompería (`JoinHandle` no admite poll tras
    // `Ready`). Para el resto de las ramas, esto es lo que le da tiempo a la
    // TUI de restaurar la terminal antes de seguir.
    ui_shutdown.cancel();
    if let Some(handle) = ui_handle {
        if !handle.is_finished() {
            let _ = handle.await;
        }
    }

    assistant.shutdown().await;
    result
}

/// Espera un `JoinHandle` opcional sin resolver nunca si es `None` — así se
/// puede sumar como una rama más de un `tokio::select!` sin ramificar la
/// macro por configuración (ver uso arriba, TUI activada o no).
async fn join_optional<T>(
    handle: &mut Option<tokio::task::JoinHandle<T>>,
) -> Result<T, tokio::task::JoinError> {
    match handle {
        Some(h) => h.await,
        None => std::future::pending().await,
    }
}
