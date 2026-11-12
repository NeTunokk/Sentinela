use std::fs;
use std::path::PathBuf;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use sentinela_backup::provider_for;
use sentinela_cli::cli::RunArgs;
use sentinela_cli::commands::run::execute_with_run_id;
use sentinela_cli::report::verify_report_bytes;
use sentinela_core::config::EnvironmentConfig;
use sentinela_core::model::Report;
use sentinela_core::paths::{validate_env_name, Paths};
use sentinela_core::SentinelaError;
use sentinela_runner::{cleanup_orphans, list_orphans};
use sentinela_store::{RunDetail, RunRow, Store, TestResultRow};

use crate::{ActiveRun, ActiveRunState, AppState};

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

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    fn unprocessable_entity(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
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
            SentinelaError::Docker(_)
            | SentinelaError::Store(_)
            | SentinelaError::Io(_)
            | SentinelaError::Interrupted => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, error.to_string())
    }
}

/// Exige o token local em todas as rotas `/api/*`.
///
/// O token é gerado a cada inicialização e injetado apenas no HTML servido por
/// `127.0.0.1`, impedindo que outra origem use a API por CSRF.
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

pub(crate) async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Serialize)]
struct EnvironmentDto {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<EnvironmentConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub(crate) async fn list_environments(
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let environments = read_environments(&state.paths)?;
    Ok(Json(json!({ "environments": environments })))
}

fn read_environments(paths: &Paths) -> Result<Vec<EnvironmentDto>, ApiError> {
    let mut files = fs::read_dir(paths.envs_dir())
        .map_err(|error| ApiError::internal(format!("não foi possível listar ambientes: {error}")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_yaml(path))
        .collect::<Vec<_>>();
    files.sort();

    let mut environments = Vec::with_capacity(files.len());
    for file in files {
        let name = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("?")
            .to_string();
        match EnvironmentConfig::load(&file) {
            Ok(config) => environments.push(EnvironmentDto {
                name,
                config: Some(config),
                error: None,
            }),
            Err(error) => environments.push(EnvironmentDto {
                name,
                config: None,
                error: Some(error.to_string()),
            }),
        }
    }
    environments.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(environments)
}

fn is_yaml(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        })
}

pub(crate) async fn get_environment(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<EnvironmentConfig>, ApiError> {
    validate_env_name(&name)?;
    let path = state.paths.env_file(&name);
    if !path.exists() {
        return Err(ApiError::not_found(format!(
            "ambiente '{}' não encontrado",
            name
        )));
    }
    Ok(Json(EnvironmentConfig::load(&path)?))
}

pub(crate) async fn put_environment(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(config): Json<EnvironmentConfig>,
) -> Result<Json<Value>, ApiError> {
    validate_env_name(&name)?;
    if config.name != name {
        return Err(ApiError::bad_request(format!(
            "o nome do ambiente no corpo ('{}') precisa ser igual ao da URL ('{}')",
            config.name, name
        )));
    }

    let path = state.paths.env_file(&name);
    config.save(&path)?;

    let store = Store::open(&state.paths.database_path())?;
    let config_json = serde_json::to_string(&config)
        .map_err(|error| SentinelaError::Store(format!("serializar ambiente: {error}")))?;
    store.upsert_environment(&config.name, &config_json)?;

    Ok(Json(json!({ "saved": true, "name": name })))
}

pub(crate) async fn delete_environment(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_env_name(&name)?;
    let path = state.paths.env_file(&name);
    let file_removed = if path.exists() {
        fs::remove_file(&path).map_err(|error| {
            ApiError::internal(format!(
                "não foi possível remover {}: {error}",
                path.display()
            ))
        })?;
        true
    } else {
        false
    };

    let store = Store::open(&state.paths.database_path())?;
    let row_removed = store.remove_environment(&name)?;

    if !file_removed && !row_removed {
        return Err(ApiError::not_found(format!(
            "ambiente '{}' não encontrado",
            name
        )));
    }

    Ok(Json(json!({
        "removed": true,
        "name": name,
        "history_preserved": true,
    })))
}

#[derive(Serialize)]
struct BackupDto {
    id: String,
    provider: String,
    size_bytes: u64,
    modified_at: String,
    sha256: String,
    host_restore_list_ok: Option<bool>,
}

pub(crate) async fn list_backups(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_env_name(&name)?;
    let path = state.paths.env_file(&name);
    if !path.exists() {
        return Err(ApiError::not_found(format!(
            "ambiente '{}' não encontrado",
            name
        )));
    }

    let config = EnvironmentConfig::load(&path)?;
    let provider = provider_for(config.backup.provider);
    let backups = provider.discover(&config).await?;
    let backups = backups
        .into_iter()
        .map(|backup| BackupDto {
            id: backup.id,
            provider: backup.provider,
            size_bytes: backup.size_bytes,
            modified_at: backup.modified_at.to_rfc3339(),
            sha256: backup.sha256,
            host_restore_list_ok: backup.host_restore_list_ok,
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({ "backups": backups })))
}

#[derive(Deserialize)]
pub(crate) struct RunsQuery {
    env: Option<String>,
    status: Option<String>,
}

pub(crate) async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<Value>, ApiError> {
    let environment = query.env.as_deref().filter(|value| !value.is_empty());
    if let Some(environment) = environment {
        validate_env_name(environment)?;
    }
    let status = query.status.as_deref().filter(|value| !value.is_empty());

    let store = Store::open(&state.paths.database_path())?;
    let runs = store.list_runs(environment, status)?;
    let mut items = Vec::with_capacity(runs.len());
    for run in &runs {
        let detail = store.get_run(&run.id)?;
        items.push(run_dto(run, detail.as_ref()));
    }

    Ok(Json(json!({ "runs": items })))
}

pub(crate) async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_run_id(&id)?;

    let detail = {
        let store = Store::open(&state.paths.database_path())?;
        store.get_run(&id)?
    };

    if let Some(detail) = detail {
        let verification = verify_stored_report(&state.paths, &id, &detail);
        let report = report_dto(&detail, &verification);
        let parsed = verification
            .as_ref()
            .ok()
            .and_then(|verified| serde_json::from_slice::<Report>(&verified.bytes).ok());
        let timings = parsed.as_ref().map(|report| TimingsDto {
            discovery_ms: report.timings_ms.discovery_ms,
            startup_ms: report.timings_ms.startup_ms,
            restore_ms: report.timings_ms.restore_ms,
            tests_ms: report.timings_ms.tests_ms,
            total_ms: report.timings_ms.total_ms,
        });
        let backup = parsed.as_ref().map(|report| BackupReportDto {
            id: report.backup.id.clone(),
            provider: report.backup.provider.clone(),
            size_bytes: report.backup.size_bytes,
            modified_at: report.backup.modified_at.to_rfc3339(),
            sha256: report.backup.sha256.clone(),
            host_restore_list_ok: report.backup.host_restore_list_ok,
            restore_list_ok: report.backup.restore_list_ok,
        });
        let summary = parsed.as_ref().map(|report| SummaryDto {
            total: report.summary.total,
            passed: report.summary.passed,
            failed: report.summary.failed,
            errors: report.summary.errors,
        });

        return Ok(Json(json!({
            "pending": false,
            "run": run_dto(&detail.run, Some(&detail)),
            "tests": detail.tests.iter().map(test_dto).collect::<Vec<_>>(),
            "report": report,
            "timings": timings,
            "backup": backup,
            "summary": summary,
        })));
    }

    let active = {
        let active_runs = state.active_runs.lock().await;
        active_runs.get(&id).cloned()
    };
    let Some(active) = active else {
        return Err(ApiError::not_found(format!(
            "execução '{}' não encontrada",
            id
        )));
    };

    let (status, error_message, pending) = match &active.state {
        ActiveRunState::Running => ("running", None, true),
        ActiveRunState::Failed(message) => ("infra_error", Some(message.clone()), false),
    };

    let run = RunDto {
        id,
        environment: active.environment,
        backup_id: None,
        backup_sha256: None,
        status: status.to_string(),
        exit_code: 3,
        started_at: active.started_at.to_rfc3339(),
        finished_at: None,
        total_duration_ms: None,
        restore_exit_code: None,
        error_message,
        tests_total: 0,
        tests_passed: 0,
        tests_failed: 0,
        tests_errors: 0,
    };

    Ok(Json(json!({
        "pending": pending,
        "run": run,
        "tests": Vec::<TestDto>::new(),
        "report": Value::Null,
        "timings": Value::Null,
        "backup": Value::Null,
        "summary": Value::Null,
    })))
}

#[derive(Deserialize)]
pub(crate) struct StartRunBody {
    environment: String,
    backup: Option<String>,
}

pub(crate) async fn start_run(
    State(state): State<AppState>,
    Json(body): Json<StartRunBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate_env_name(&body.environment)?;
    let env_path = state.paths.env_file(&body.environment);
    if !env_path.exists() {
        return Err(ApiError::not_found(format!(
            "ambiente '{}' não encontrado",
            body.environment
        )));
    }
    EnvironmentConfig::load(&env_path)?;

    let run_id = Uuid::new_v4().to_string();
    let started_at = Utc::now();
    {
        let mut active = state.active_runs.lock().await;
        active.retain(|_, run| matches!(run.state, ActiveRunState::Running));
        if let Some((existing, _)) = active
            .iter()
            .find(|(_, run)| matches!(run.state, ActiveRunState::Running))
        {
            return Err(ApiError::conflict(format!(
                "já existe uma execução em andamento ({existing})"
            )));
        }
        active.insert(
            run_id.clone(),
            ActiveRun {
                environment: body.environment.clone(),
                started_at,
                state: ActiveRunState::Running,
            },
        );
    }

    let paths = state.paths.clone();
    let active_runs = state.active_runs.clone();
    let environment = body.environment.clone();
    let args = RunArgs {
        env: body.environment,
        backup: body.backup,
        keep: false,
    };
    let task_run_id = run_id.clone();
    tokio::spawn(async move {
        let outcome = execute_with_run_id(args, &paths, task_run_id.clone()).await;
        let mut active = active_runs.lock().await;
        match outcome {
            Ok(_) => {
                active.remove(&task_run_id);
            }
            Err(error) => {
                active.insert(
                    task_run_id,
                    ActiveRun {
                        environment,
                        started_at,
                        state: ActiveRunState::Failed(format!("{error:#}")),
                    },
                );
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "run_id": run_id, "status": "running" })),
    ))
}

pub(crate) async fn report_json(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    validate_run_id(&id)?;
    let detail = {
        let store = Store::open(&state.paths.database_path())?;
        store.get_run(&id)?
    }
    .ok_or_else(|| ApiError::not_found(format!("execução '{}' não encontrada", id)))?;

    let verified =
        verify_stored_report(&state.paths, &id, &detail).map_err(ApiError::unprocessable_entity)?;

    let mut response = verified.bytes.into_response();
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
    validate_run_id(&id)?;
    let detail = {
        let store = Store::open(&state.paths.database_path())?;
        store.get_run(&id)?
    }
    .ok_or_else(|| ApiError::not_found(format!("execução '{}' não encontrada", id)))?;

    verify_stored_report(&state.paths, &id, &detail).map_err(ApiError::unprocessable_entity)?;

    let html_path = detail.report.as_ref().map_or_else(
        || state.paths.reports_dir().join(format!("{id}.html")),
        |report| PathBuf::from(&report.html_path),
    );
    let html = fs::read_to_string(&html_path).map_err(|error| {
        ApiError::not_found(format!(
            "relatório HTML não encontrado em {}: {error}",
            html_path.display()
        ))
    })?;

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

pub(crate) async fn list_resources() -> Result<Json<Value>, ApiError> {
    let resources = list_orphans().await?;
    Ok(Json(json!({ "resources": resources })))
}

pub(crate) async fn run_cleanup() -> Result<Json<Value>, ApiError> {
    let report = cleanup_orphans().await?;
    Ok(Json(json!({
        "containers_removed": report.containers_removed,
        "networks_removed": report.networks_removed,
        "volumes_removed": report.volumes_removed,
        "errors": report.errors,
    })))
}

#[derive(Serialize)]
struct RunDto {
    id: String,
    environment: String,
    backup_id: Option<String>,
    backup_sha256: Option<String>,
    status: String,
    exit_code: i32,
    started_at: String,
    finished_at: Option<String>,
    total_duration_ms: Option<u64>,
    restore_exit_code: Option<i32>,
    error_message: Option<String>,
    tests_total: usize,
    tests_passed: usize,
    tests_failed: usize,
    tests_errors: usize,
}

fn run_dto(run: &RunRow, detail: Option<&RunDetail>) -> RunDto {
    let (tests_total, tests_passed, tests_failed, tests_errors) = match detail {
        Some(detail) => counts(&detail.tests),
        None => (0, 0, 0, 0),
    };
    RunDto {
        id: run.id.clone(),
        environment: run.environment.clone(),
        backup_id: run.backup_id.clone(),
        backup_sha256: run.backup_sha256.clone(),
        status: run.status.clone(),
        exit_code: run.exit_code,
        started_at: run.started_at.clone(),
        finished_at: run.finished_at.clone(),
        total_duration_ms: run.total_duration_ms,
        restore_exit_code: run.restore_exit_code,
        error_message: run.error_message.clone(),
        tests_total,
        tests_passed,
        tests_failed,
        tests_errors,
    }
}

fn counts(tests: &[TestResultRow]) -> (usize, usize, usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    let mut errors = 0;
    for test in tests {
        match test.status.as_str() {
            "passed" => passed += 1,
            "failed" => failed += 1,
            "error" => errors += 1,
            _ => {}
        }
    }
    (tests.len(), passed, failed, errors)
}

#[derive(Serialize)]
struct TestDto {
    name: String,
    status: String,
    value: Option<Value>,
    duration_ms: u64,
    message: Option<String>,
}

fn test_dto(test: &TestResultRow) -> TestDto {
    TestDto {
        name: test.name.clone(),
        status: test.status.clone(),
        value: test.value.clone(),
        duration_ms: test.duration_ms,
        message: test.message.clone(),
    }
}

#[derive(Serialize)]
struct ReportDto {
    verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signed_at: Option<String>,
}

#[derive(Serialize)]
struct TimingsDto {
    discovery_ms: u64,
    startup_ms: u64,
    restore_ms: u64,
    tests_ms: u64,
    total_ms: u64,
}

#[derive(Serialize)]
struct BackupReportDto {
    id: String,
    provider: String,
    size_bytes: u64,
    modified_at: String,
    sha256: String,
    host_restore_list_ok: Option<bool>,
    restore_list_ok: Option<bool>,
}

#[derive(Serialize)]
struct SummaryDto {
    total: usize,
    passed: usize,
    failed: usize,
    errors: usize,
}

struct VerifiedReport {
    bytes: Vec<u8>,
    sha256: String,
    signature: String,
    public_key: String,
}

fn verify_stored_report(
    paths: &Paths,
    run_id: &str,
    detail: &RunDetail,
) -> Result<VerifiedReport, String> {
    let stored = detail.report.as_ref();
    let json_path = stored.map_or_else(
        || paths.reports_dir().join(format!("{run_id}.json")),
        |report| PathBuf::from(&report.json_path),
    );

    let expected_sha256 = read_sidecar(&json_path.with_extension("sha256"))
        .or_else(|| stored.map(|report| report.sha256.clone()));
    let signature = read_sidecar(&json_path.with_extension("sig"))
        .or_else(|| stored.map(|report| report.signature.clone()))
        .ok_or_else(|| {
            format!(
                "assinatura ausente para {} (esperado o sidecar .sig)",
                json_path.display()
            )
        })?;
    let public_key = read_sidecar(&json_path.with_extension("pub"))
        .or_else(|| stored.map(|report| report.public_key.clone()))
        .ok_or_else(|| {
            format!(
                "chave pública ausente para {} (esperado o sidecar .pub)",
                json_path.display()
            )
        })?;

    let bytes = fs::read(&json_path).map_err(|error| {
        format!(
            "não foi possível ler o relatório {}: {error}",
            json_path.display()
        )
    })?;
    let sha256 = verify_report_bytes(&bytes, expected_sha256.as_deref(), &signature, &public_key)
        .map_err(|error| error.to_string())?;

    Ok(VerifiedReport {
        bytes,
        sha256,
        signature,
        public_key,
    })
}

fn report_dto(detail: &RunDetail, verification: &Result<VerifiedReport, String>) -> ReportDto {
    let stored = detail.report.as_ref();
    match verification {
        Ok(verified) => ReportDto {
            verified: true,
            verification_error: None,
            sha256: Some(verified.sha256.clone()),
            signature: Some(verified.signature.clone()),
            public_key: Some(verified.public_key.clone()),
            signed_at: stored.map(|report| report.signed_at.clone()),
        },
        Err(error) => ReportDto {
            verified: false,
            verification_error: Some(error.clone()),
            sha256: stored.map(|report| report.sha256.clone()),
            signature: stored.map(|report| report.signature.clone()),
            public_key: stored.map(|report| report.public_key.clone()),
            signed_at: stored.map(|report| report.signed_at.clone()),
        },
    }
}

fn read_sidecar(path: &std::path::Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
}

fn validate_run_id(run_id: &str) -> Result<(), ApiError> {
    let valid = !run_id.is_empty()
        && run_id.len() <= 64
        && run_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "run_id inválido: {run_id:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sentinela_cli::report::sign::ReportSigner;
    use sentinela_cli::report::write_reports;
    use sentinela_core::model::{
        BackupReport, Report, RestoreReport, RunStatus, TestSummary, TimingsReport,
    };

    use super::*;

    fn sample_report() -> Report {
        Report {
            schema_version: Report::SCHEMA_VERSION,
            run_id: "33333333-3333-4333-8333-333333333333".into(),
            environment: "producao".into(),
            status: RunStatus::Ok,
            exit_code: 0,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            backup: BackupReport {
                id: "db.dump".into(),
                path: "/tmp/db.dump".into(),
                provider: "logical".into(),
                size_bytes: 128,
                modified_at: Utc::now(),
                sha256: "abc".into(),
                host_restore_list_ok: Some(true),
                restore_list_ok: Some(true),
            },
            timings_ms: TimingsReport::default(),
            restore: Some(RestoreReport {
                duration_ms: 10,
                list_ok: true,
                exit_code: Some(0),
                stderr_tail: None,
            }),
            tests: Vec::new(),
            summary: TestSummary::default(),
            error: None,
        }
    }

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let directory = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: directory.path().join("config"),
            data_dir: directory.path().join("data"),
        };
        paths.ensure().unwrap();
        (directory, paths)
    }

    #[test]
    fn verifies_report_written_by_the_cli() {
        let (_directory, paths) = temp_paths();
        let signer = ReportSigner::load_or_create(&paths).unwrap();
        let report = sample_report();
        let artifacts = write_reports(&paths, &report, &signer).unwrap();

        let mut store = Store::open(&paths.database_path()).unwrap();
        store.save_run(&report, &report.tests, &artifacts).unwrap();
        let detail = store.get_run(&report.run_id).unwrap().unwrap();

        let verified = verify_stored_report(&paths, &report.run_id, &detail).unwrap();
        assert_eq!(verified.sha256, artifacts.sha256);
        assert_eq!(verified.signature, artifacts.signature);
        assert_eq!(verified.public_key, artifacts.public_key);
        assert_eq!(verified.bytes, fs::read(&artifacts.json_path).unwrap());
    }

    #[test]
    fn rejects_tampered_report_bytes() {
        let (_directory, paths) = temp_paths();
        let signer = ReportSigner::load_or_create(&paths).unwrap();
        let report = sample_report();
        let artifacts = write_reports(&paths, &report, &signer).unwrap();

        let mut store = Store::open(&paths.database_path()).unwrap();
        store.save_run(&report, &report.tests, &artifacts).unwrap();
        fs::write(&artifacts.json_path, b"{\"run_id\":\"alterado\"}").unwrap();

        let detail = store.get_run(&report.run_id).unwrap().unwrap();
        assert!(verify_stored_report(&paths, &report.run_id, &detail).is_err());
    }
}
