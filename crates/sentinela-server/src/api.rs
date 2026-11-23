//! Camada HTTP legada. Toda a lógica vive em `sentinela-ui`; aqui apenas
//! traduzimos requisições/respostas para axum.

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use sentinela_core::config::EnvironmentConfig;
use sentinela_core::SentinelaError;
use sentinela_runner::OrphanResource;
use sentinela_ui::{
    BackupsPayload, CleanupPayload, EnvironmentsPayload, RemovedEnvironmentPayload,
    ResourcesPayload, RunDetailPayload, RunsPayload, SavedEnvironmentPayload, StartRunPayload,
};

use crate::AppState;

/// Erro de API convertido em uma resposta JSON uniforme.
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<SentinelaError> for ApiError {
    fn from(error: SentinelaError) -> Self {
        let status = match &error {
            SentinelaError::Config(_) | SentinelaError::Backup(_) => StatusCode::BAD_REQUEST,
            SentinelaError::BackupInvalid(_)
            | SentinelaError::Restore(_)
            | SentinelaError::TestsFailed(_)
            | SentinelaError::Report(_) => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, error.to_string())
    }
}

/// Exige o token local em todas as rotas `/api/*`.
pub(crate) async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let provided = request
        .headers()
        .get("x-sentinela-token")
        .and_then(|value| value.to_str().ok());
    let authorized = provided.is_some_and(|value| value == state.token.as_str());

    if authorized {
        next.run(request).await
    } else {
        ApiError::unauthorized("token local inválido ou ausente").into_response()
    }
}

pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub(crate) async fn shutdown(State(state): State<AppState>) -> Json<serde_json::Value> {
    state.shutdown.notify_one();
    Json(json!({ "status": "shutting_down" }))
}

pub(crate) async fn list_environments(
    State(state): State<AppState>,
) -> Result<Json<EnvironmentsPayload>, ApiError> {
    Ok(Json(state.ui.list_environments()?))
}

pub(crate) async fn get_environment(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<EnvironmentConfig>, ApiError> {
    Ok(Json(state.ui.get_environment(&name)?))
}

pub(crate) async fn put_environment(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(config): Json<EnvironmentConfig>,
) -> Result<Json<SavedEnvironmentPayload>, ApiError> {
    Ok(Json(state.ui.save_environment(&name, &config)?))
}

pub(crate) async fn delete_environment(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<RemovedEnvironmentPayload>, ApiError> {
    Ok(Json(state.ui.delete_environment(&name)?))
}

pub(crate) async fn list_backups(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<BackupsPayload>, ApiError> {
    Ok(Json(state.ui.list_backups(&name).await?))
}

#[derive(Deserialize)]
pub(crate) struct RunsQuery {
    env: Option<String>,
    status: Option<String>,
}

pub(crate) async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<RunsPayload>, ApiError> {
    Ok(Json(state.ui.list_runs(
        query.env.as_deref(),
        query.status.as_deref(),
    )?))
}

pub(crate) async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RunDetailPayload>, ApiError> {
    Ok(Json(state.ui.get_run(&id).await?))
}

#[derive(Deserialize)]
pub(crate) struct StartRunBody {
    environment: String,
    backup: Option<String>,
}

pub(crate) async fn start_run(
    State(state): State<AppState>,
    Json(body): Json<StartRunBody>,
) -> Result<(StatusCode, Json<StartRunPayload>), ApiError> {
    let payload = state
        .ui
        .start_run(&body.environment, body.backup, None)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(payload)))
}

pub(crate) async fn cancel_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cancelled = state.ui.cancel_run(&id).await?;
    Ok(Json(json!({ "cancelled": cancelled })))
}

pub(crate) async fn report_json(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let bytes = state.ui.report_json(&id)?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub(crate) async fn report_html(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let html = state.ui.report_html(&id)?;
    let mut response = html.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub(crate) async fn list_resources(
    State(state): State<AppState>,
) -> Result<Json<ResourcesPayload<OrphanResource>>, ApiError> {
    Ok(Json(state.ui.list_resources().await?))
}

pub(crate) async fn run_cleanup(
    State(state): State<AppState>,
) -> Result<Json<CleanupPayload>, ApiError> {
    Ok(Json(state.ui.cleanup().await?))
}
