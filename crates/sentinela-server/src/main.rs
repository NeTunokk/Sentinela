mod api;
mod pwa;
mod web;

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use base64::Engine as _;
use rand::Rng;
use sentinela_core::paths::Paths;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::timeout;
use tracing_subscriber::EnvFilter;

/// Estado compartilhado por todas as rotas do painel.
#[derive(Clone)]
pub(crate) struct AppState {
    /// Lógica da interface, compartilhada com o app desktop.
    pub ui: sentinela_ui::UiState,
    pub token: Arc<String>,
    /// Acionado pelo botão "Encerrar servidor" do painel.
    pub shutdown: Arc<Notify>,
}

/// Opções lidas da linha de comando.
struct Options {
    port: u16,
    open_browser: bool,
    /// Prefere abrir o painel no aplicativo instalado (PWA) antes do navegador.
    prefer_app: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let options = parse_options();

    // Duplo clique duas vezes: em vez de falhar com "porta em uso", abre o
    // painel que já está no ar e sai.
    if panel_is_running(options.port).await {
        let url = panel_url(options.port);
        println!("Sentinela já está em execução em {url}");
        if options.open_browser {
            open_panel(&url, options.port, options.prefer_app);
        }
        return Ok(());
    }

    let paths = Paths::discover()?;
    paths.ensure()?;

    let token = new_token();
    let shutdown = Arc::new(Notify::new());
    let state = AppState {
        ui: sentinela_ui::UiState::new(paths),
        token: Arc::new(token),
        shutdown: shutdown.clone(),
    };

    let api = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/environments", get(api::list_environments))
        .route(
            "/api/environments/{name}",
            get(api::get_environment)
                .put(api::put_environment)
                .delete(api::delete_environment),
        )
        .route("/api/environments/{name}/backups", get(api::list_backups))
        .route("/api/runs", get(api::list_runs).post(api::start_run))
        .route("/api/runs/{id}", get(api::get_run))
        .route("/api/runs/{id}/cancel", post(api::cancel_run))
        .route("/api/runs/{id}/report.json", get(api::report_json))
        .route("/api/runs/{id}/report.html", get(api::report_html))
        .route("/api/resources", get(api::list_resources))
        .route("/api/cleanup", post(api::run_cleanup))
        .route("/api/shutdown", post(api::shutdown))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api::require_token,
        ));

    let app = Router::new()
        .route("/", get(web::index))
        .route("/styles.css", get(web::styles))
        .route("/app.js", get(web::script))
        .route("/assets/sentinela-logo.png", get(web::logo))
        .route("/assets/icon-192.png", get(web::icon_192))
        .route("/assets/icon-512.png", get(web::icon_512))
        .route("/manifest.webmanifest", get(web::manifest))
        .route("/sw.js", get(web::service_worker))
        .route("/assets/inter-latin.woff2", get(web::font))
        .merge(api)
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", options.port)).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            return Err(format!(
                "a porta {} já está em uso por outro programa; feche-o ou use --port <outra>",
                options.port
            )
            .into());
        }
        Err(error) => return Err(error.into()),
    };

    let url = panel_url(options.port);
    println!("Sentinela — painel local em {url}");
    println!("O servidor escuta apenas em 127.0.0.1; nenhum dado sai da máquina.");
    println!("Use o botão \"Encerrar servidor\" no painel para desligar, ou Ctrl+C.");

    if options.open_browser {
        open_panel(&url, options.port, options.prefer_app);
    }

    let shutdown_signal = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal.notified().await;
            println!("Sentinela — encerrando o servidor…");
        })
        .await?;

    println!("Sentinela — servidor encerrado.");
    Ok(())
}

fn panel_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// Verifica se já existe um painel do Sentinela escutando nesta porta.
///
/// A resposta só conta como "já rodando" quando é o nosso HTML: qualquer outro
/// serviço na porta é tratado como porta ocupada, não como instância nossa.
async fn panel_is_running(port: u16) -> bool {
    let connect = timeout(
        Duration::from_millis(700),
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await;
    let Ok(Ok(mut stream)) = connect else {
        return false;
    };

    let request = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }

    let mut response = Vec::new();
    let _ = timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await;

    String::from_utf8_lossy(&response).contains("<title>Sentinela</title>")
}

/// Abre o painel no aplicativo instalado (PWA) quando existir; caso contrário,
/// abre no navegador padrão.
fn open_panel(url: &str, port: u16, prefer_app: bool) {
    if prefer_app {
        if pwa::open_installed(port) {
            println!("Sentinela — painel aberto no aplicativo instalado.");
            return;
        }
        if pwa::open_app_window(url) {
            println!("Sentinela — painel aberto em janela de aplicativo.");
            return;
        }
    }
    open_browser(url);
}

/// Abre o painel no navegador padrão sem depender de nenhuma biblioteca extra.
fn open_browser(url: &str) {
    let result = if cfg!(target_os = "windows") {
        // O primeiro argumento vazio é o título da janela exigido por `start`.
        Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if pwa::running_in_wsl() {
        // No WSL, quem tem navegador é o Windows. `explorer.exe <url>` abre no
        // navegador padrão e não depende do argumento vazio de `start`.
        let mut command = Command::new("explorer.exe");
        command.arg(url);
        if std::path::Path::new("/mnt/c/Windows").is_dir() {
            command.current_dir("/mnt/c/Windows");
        }
        command.spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };

    if let Err(error) = result {
        eprintln!("Não foi possível abrir o navegador automaticamente ({error}).");
        eprintln!("Abra manualmente: {url}");
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,sentinela_server=info,sentinela_cli=info,sentinela_runner=info")
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn new_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn parse_options() -> Options {
    let mut port = 8787;
    let mut open_browser = true;
    let mut prefer_app = true;
    let mut args = std::env::args().skip(1);

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--port" | "-p" => {
                if let Some(value) = args.next() {
                    if let Ok(value) = value.parse::<u16>() {
                        port = value;
                    }
                }
            }
            "--no-open" => open_browser = false,
            "--browser" => prefer_app = false,
            "--help" | "-h" => {
                println!("sentinela-server [--port <porta>] [--no-open] [--browser]");
                println!("Serve o painel local em 127.0.0.1 (padrão: 8787).");
                println!(
                    "Abre o painel no aplicativo instalado (PWA) ou em uma janela de aplicativo."
                );
                println!("--browser força o navegador padrão; --no-open não abre nada.");
                std::process::exit(0);
            }
            _ => {}
        }
    }

    Options {
        port,
        open_browser,
        prefer_app,
    }
}
