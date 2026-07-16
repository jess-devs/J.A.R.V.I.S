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
mod wake;

use std::path::PathBuf;

use clap::Parser;

use config::Config;
use orchestrator::Orchestrator;

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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(log_level))
        .init();

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
        .map_err(
            |e| tracing::warn!(error = %e, "no se pudo instalar el handler de cierre de consola"),
        )
        .ok();

    llm::model_select::resolve(&mut config).await;

    //verifica si el archivo welcome.mp3 se encuentra dentro de assets\musci
    // En caso de no haber desahbilita el booleano responsable de la funcion
    // Arreglando el bug que no dejaba iniciar el programa
    if config.welcome.enabled && !config.welcome.music_path.exists() {
        tracing::warn!(
            path = %config.welcome.music_path.display(),
            "El archivo de música no existe; deshabilitando Welcome"
        );

        config.welcome.enabled = false;
    }

    startup_checks::run(&config).await?;

    let mut assistant = Orchestrator::new(config).await?;

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
    };

    assistant.shutdown().await;
    result
}
