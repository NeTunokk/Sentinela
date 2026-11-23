#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Comandos Tauri: camada fina sobre `sentinela-ui`. Não há lógica de negócio
//! aqui, nem servidor loopback, token, porta ou chamada de rede externa.

use serde_json::json;
use tauri::{AppHandle, Emitter, State};

use sentinela_core::config::EnvironmentConfig;
use sentinela_core::paths::Paths;
use sentinela_runner::OrphanResource;
use sentinela_ui::{
    BackupsPayload, CleanupPayload, EnvironmentsPayload, RemovedEnvironmentPayload,
    ResourcesPayload, RunDetailPayload, RunsPayload, SavedEnvironmentPayload, StartRunPayload,
};

use sentinela_gui::{app::UiState, command_error, report_json_text as verified_report_json};

struct GuiState {
    ui: UiState,
}

type CommandResult<T> = std::result::Result<T, String>;

#[tauri::command]
fn health() -> serde_json::Value {
    json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION"), "desktop": true })
}

#[tauri::command]
async fn list_environments(state: State<'_, GuiState>) -> CommandResult<EnvironmentsPayload> {
    state.ui.list_environments().map_err(command_error)
}

#[tauri::command]
async fn get_environment(
    state: State<'_, GuiState>,
    name: String,
) -> CommandResult<EnvironmentConfig> {
    state.ui.get_environment(&name).map_err(command_error)
}

#[tauri::command]
async fn save_environment(
    state: State<'_, GuiState>,
    name: String,
    config: EnvironmentConfig,
) -> CommandResult<SavedEnvironmentPayload> {
    state
        .ui
        .save_environment(&name, &config)
        .map_err(command_error)
}

#[tauri::command]
async fn delete_environment(
    state: State<'_, GuiState>,
    name: String,
) -> CommandResult<RemovedEnvironmentPayload> {
    state.ui.delete_environment(&name).map_err(command_error)
}

#[tauri::command]
async fn list_backups(state: State<'_, GuiState>, name: String) -> CommandResult<BackupsPayload> {
    state.ui.list_backups(&name).await.map_err(command_error)
}

#[tauri::command]
async fn list_runs(
    state: State<'_, GuiState>,
    environment: Option<String>,
    status: Option<String>,
) -> CommandResult<RunsPayload> {
    state
        .ui
        .list_runs(environment.as_deref(), status.as_deref())
        .map_err(command_error)
}

#[tauri::command]
async fn get_run(state: State<'_, GuiState>, id: String) -> CommandResult<RunDetailPayload> {
    state.ui.get_run(&id).await.map_err(command_error)
}

#[tauri::command]
async fn start_run(
    app: AppHandle,
    state: State<'_, GuiState>,
    environment: String,
    backup: Option<String>,
) -> CommandResult<StartRunPayload> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let payload = state
        .ui
        .start_run(&environment, backup, Some(sender))
        .await
        .map_err(command_error)?;

    tauri::async_runtime::spawn(async move {
        while let Some(progress) = receiver.recv().await {
            let _ = app.emit("run-progress", progress);
        }
    });

    Ok(payload)
}

#[tauri::command]
async fn cancel_run(state: State<'_, GuiState>, id: String) -> CommandResult<serde_json::Value> {
    let cancelled = state.ui.cancel_run(&id).await.map_err(command_error)?;
    Ok(json!({ "cancelled": cancelled }))
}

#[tauri::command]
async fn report_json_text(state: State<'_, GuiState>, id: String) -> CommandResult<String> {
    verified_report_json(&state.ui, &id).map_err(command_error)
}

#[tauri::command]
async fn report_html(state: State<'_, GuiState>, id: String) -> CommandResult<String> {
    state.ui.report_html(&id).map_err(command_error)
}

#[tauri::command]
async fn list_resources(
    state: State<'_, GuiState>,
) -> CommandResult<ResourcesPayload<OrphanResource>> {
    state.ui.list_resources().await.map_err(command_error)
}

#[tauri::command]
async fn cleanup(state: State<'_, GuiState>) -> CommandResult<CleanupPayload> {
    state.ui.cleanup().await.map_err(command_error)
}

#[tauri::command]
async fn close_app(app: AppHandle) {
    app.exit(0);
}

fn main() {
    let paths = match Paths::discover().and_then(|paths| {
        paths.ensure()?;
        Ok(paths)
    }) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    let result = tauri::Builder::default()
        .manage(GuiState {
            ui: UiState::new(paths),
        })
        .invoke_handler(tauri::generate_handler![
            health,
            list_environments,
            get_environment,
            save_environment,
            delete_environment,
            list_backups,
            list_runs,
            get_run,
            start_run,
            cancel_run,
            report_json_text,
            report_html,
            list_resources,
            cleanup,
            close_app,
        ])
        .run(tauri::generate_context!());

    if let Err(error) = result {
        eprintln!("falha ao executar o aplicativo: {error}");
        std::process::exit(1);
    }
}
