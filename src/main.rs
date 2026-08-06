mod agent;
mod audio;
mod audit;
mod config;
mod config_ui;
mod echo_gate;
mod errors;
mod http;
mod ipc;
mod llm;
mod mcp;
mod memory;
mod net_guard;
mod orchestrator;
mod pipeline;
mod platform;
mod reminders;
mod startup_checks;
mod stt;
mod text;
mod text_mode;
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

    /// Modo texto: lee pedidos por stdin en vez de por voz (STT). Jarvis
    /// sigue respondiendo por TTS y pidiendo confirmación de voz-por-texto
    /// para acciones de riesgo. Útil para debugging o ambientes ruidosos.
    #[arg(long)]
    text_mode: bool,

    /// Solo levanta la página de configuración local (ver `web_ui` en
    /// config.yaml) y no arranca el resto de Jarvis: sin STT/TTS/LLM, sin
    /// preflight de micrófono/Python/Ollama. Pensado para arreglar un
    /// config.yaml roto (o simplemente configurar Jarvis) sin que el resto
    /// del programa tenga que poder arrancar primero. Ignora `web_ui.enabled`
    /// (pedirlo por acá ya es la intención explícita).
    #[arg(long)]
    config_ui: bool,
}

/// Agrupa el proceso Jarvis (y todo lo que este lance después, incluidos
/// los workers Python) en un Windows Job Object con
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: cuando Jarvis desaparece, sea cual
/// sea la causa (crash, panic, "Finalizar tarea", cierre de consola),
/// Windows mata automáticamente a todos sus procesos hijos sin que Rust
/// tenga que ejecutar ningún código de limpieza.
///
/// Debe llamarse ANTES del primer spawn de cualquier worker — los hijos
/// solo heredan la membresía del job si el proceso Jarvis ya es miembro en
/// el momento del spawn. Por eso se llama en los dos puntos de entrada que
/// pueden spawnear un worker: acá arriba, antes de la rama `--config-ui`
/// (que puede spawnear un `CalibrationWorker` bajo demanda sin que el resto
/// de Jarvis llegue a correr nunca), y de nuevo al principio de `run()`
/// para el modo normal. Antes, esto solo vivía dentro de `run()`, así que
/// un Jarvis corriendo como `--config-ui` puro que moría de golpe dejaba
/// cualquier worker de calibración activo huérfano.
///
/// El handle se "olvida" a propósito: con KILL_ON_JOB_CLOSE, cerrar el
/// último handle del job (el `Drop` normal) mata a TODOS sus miembros —
/// incluido este mismo proceso, en silencio y con código 0. Al olvidarlo,
/// el kernel lo cierra recién cuando el proceso termina, que es el único
/// momento en el que debe arrastrar a los workers.
///
/// Sin equivalente instalado todavía en Unix (el equivalente real sería un
/// grupo de procesos vía `setsid`/`killpg`, una mejora futura del roadmap)
/// — ahí el arranque sigue andando sin esta garantía extra.
#[cfg(windows)]
fn install_job_object_guard() {
    match ipc::job_object::JobObject::create_and_assign_current_process() {
        Ok(job) => std::mem::forget(job),
        Err(e) => {
            tracing::error!(
                error = %e,
                "no se pudo crear el Job Object de aislamiento; si Jarvis muere de forma anómala los workers Python podrían quedar huérfanos"
            );
        }
    }
}
#[cfg(unix)]
fn install_job_object_guard() {}

/// Abre `url` en el navegador default del sistema, best-effort (un fallo acá
/// no debe tumbar el servidor de configuración — el usuario siempre puede
/// abrir la URL a mano, que además ya se imprime en consola). No usa la
/// crate `webbrowser`: en Windows, `cmd /C start` alcanza en dos líneas.
fn open_url_in_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(|info| {
        // `eprintln!` además de `tracing::error!` a propósito: con
        // `eprintln!` garantiza que un panic sea visible aunque el subscriber
        // de tracing todavía no esté inicializado.
        eprintln!("Panic no controlado en Jarvis: {info}");
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

    if cli.config_ui {
        // Modo standalone: no arranca el pipeline de voz y los logs van a
        // consola.
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(log_level))
            .init();

        install_job_object_guard();

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], config.web_ui.port));
        println!("Página de configuración local en http://{addr}");
        println!("Ctrl+C para salir. El resto de Jarvis no está corriendo.");

        // Acá sí tiene sentido abrir el navegador solo: `--config-ui` ya es
        // una acción explícita del usuario ("quiero configurar Jarvis"),
        // a diferencia del modo integrado (ver `run()`), donde Jarvis puede
        // estar corriendo en segundo plano sin que nadie esté mirando.
        let url = format!("http://{addr}");
        let open_browser: Box<dyn FnOnce() + Send> = Box::new(move || open_url_in_browser(&url));

        tokio::select! {
            _ = config_ui::serve(cli.config, addr, PathBuf::from("web/config-ui/dist"), Some(open_browser)) => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Señal de interrupción recibida, cerrando...");
            }
        }
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(log_level))
        .init();

    if let Err(e) = run(config, cli.config, cli.text_mode).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run(mut config: Config, config_path: PathBuf, text_mode: bool) -> errors::Result<()> {
    // Ver `install_job_object_guard` (arriba): debe instalarse ANTES de
    // crear el Orchestrator, que spawnea los workers Python en su
    // constructor.
    install_job_object_guard();

    #[cfg(windows)]
    let mut console_shutdown_rx = ipc::console_handler::install()
        .map_err(
            |e| tracing::warn!(error = %e, "no se pudo instalar el handler de cierre de consola"),
        )
        .ok();
    #[cfg(unix)]
    let mut console_shutdown_rx: Option<tokio::sync::mpsc::Receiver<()>> = None;

    if config.agent.audit.enabled {
        audit::init(&config.agent.audit.path);
    }

    llm::model_select::resolve(&mut config).await;

    // El mp3 del modo bienvenida es del usuario y nunca se versiona (ver
    // `assets/music/.gitkeep`), así que faltar es el caso normal en una
    // instalación nueva. Degradar la función con un warning en vez de abortar
    // el arranque: no poder escuchar música no es razón para no poder hablarle
    // a Jarvis. Este es el único chequeo del mp3 — corre antes del preflight,
    // que por eso ya no lo repite.
    if config.welcome.enabled && !config.welcome.music_path.exists() {
        tracing::warn!(
            path = %config.welcome.music_path.display(),
            "El archivo de música no existe; deshabilitando Welcome"
        );

        config.welcome.enabled = false;
    }

    if config.web_ui.enabled {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], config.web_ui.port));
        let static_dir = PathBuf::from("web/config-ui/dist");
        let server_config_path = config_path.clone();
        tokio::spawn(config_ui::serve(server_config_path, addr, static_dir, None));

        if !config.onboarding.completed {
            tracing::info!(
                url = %format!("http://{addr}"),
                "primer arranque: abrí la página de configuración para elegir tu micrófono (opcional)"
            );
        }
    }

    if text_mode {
        startup_checks::run_text_mode(&config).await?;
        return tokio::select! {
            r = text_mode::run(config) => r,
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Señal de interrupción recibida, cerrando...");
                Ok(())
            }
        };
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

    // Si `ui_result` fue la rama que resolvió el `select!` de arriba, el
    // handle ya está completo (`join_optional` lo pollea vía `&mut`, no lo
    // consume) — repollearlo acá rompería (`JoinHandle` no admite poll tras
    assistant.shutdown().await;
    result
}
