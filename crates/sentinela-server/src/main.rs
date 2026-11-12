mod api;
mod web;

use std::collections::HashMap;
use std::sync::Arc;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use base64::Engine as _;
use rand::Rng;
use sentinela_core::paths::Paths;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

/// Estado compartilhado por todas as rotas do painel.
#[derive(Clone)]
pub(crate) struct AppState {
    pub paths: Paths,
    pub token: Arc<String>,
    pub active_runs: Arc<Mutex<HashMap<String, ActiveRun>>>,
}

/// Execução iniciada pelo painel e ainda não concluída (ou que falhou antes de
/// gerar um registro no histórico).
#[derive(Clone, Debug)]
pub(crate) struct ActiveRun {
    pub environment: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub state: ActiveRunState,
}

#[derive(Clone, Debug)]
pub(crate) enum ActiveRunState {
    Running,
    Failed(String),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let port = parse_port();

    let paths = Paths::discover()?;
    paths.ensure()?;

    let token = new_token();
    let state = AppState {
        paths,
        token: Arc::new(token),
        active_runs: Arc::new(Mutex::new(HashMap::new())),
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
        .route("/api/runs/{id}/report.json", get(api::report_json))
        .route("/api/runs/{id}/report.html", get(api::report_html))
        .route("/api/resources", get(api::list_resources))
        .route("/api/cleanup", post(api::run_cleanup))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api::require_token,
        ));

    let app = Router::new()
        .route("/", get(web::index))
        .route("/styles.css", get(web::styles))
        .route("/app.js", get(web::script))
        .route("/assets/sentinela-logo.png", get(web::logo))
        .route("/assets/inter-latin.woff2", get(web::font))
        .merge(api)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("Sentinela — painel local em http://127.0.0.1:{port}/");
    println!("O servidor escuta apenas em 127.0.0.1; nenhum dado sai da máquina.");
    axum::serve(listener, app).await?;
    Ok(())
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

fn parse_port() -> u16 {
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--port" | "-p" => {
                if let Some(value) = args.next() {
                    if let Ok(port) = value.parse::<u16>() {
                        return port;
                    }
                }
            }
            "--help" | "-h" => {
                println!("sentinela-server [--port <porta>]");
                println!("Serve o painel local em 127.0.0.1 (padrão: 8787).");
                std::process::exit(0);
            }
            _ => {}
        }
    }
    8787
}
