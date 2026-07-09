mod audio;
mod config;
mod errors;
mod ipc;
mod llm;
mod orchestrator;
mod pipeline;
mod startup_checks;
mod stt;
mod text;
mod tts;
mod wake;

use std::path::PathBuf;

use clap::Parser;

use config::Config;
use orchestrator::Orchestrator;

#[derive(Parser)]
#[command(name = "jarvis", about = "Asistente de voz conversacional en tiempo real")]
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

async fn run(config: Config) -> errors::Result<()> {
    startup_checks::run(&config).await?;

    let mut assistant = Orchestrator::new(config).await?;

    let result = tokio::select! {
        r = assistant.run() => r,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Señal de interrupción recibida, cerrando...");
            Ok(())
        }
    };

    assistant.shutdown().await;
    result
}
