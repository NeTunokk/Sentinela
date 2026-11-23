//! DTOs da interface, compartilhados pelo painel legado e pelo app desktop.
//!
//! Os nomes de campos e a ordinalidade são idênticos aos do painel web 0.1: o
//! frontend continua recebendo exatamente o mesmo JSON de antes.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use sentinela_core::config::EnvironmentConfig;
use sentinela_core::model::Report;
use sentinela_store::{ReportRow, RunDetail, RunRow, TestResultRow};

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentDto {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<EnvironmentConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupDto {
    pub id: String,
    pub provider: String,
    pub size_bytes: u64,
    pub modified_at: String,
    pub sha256: String,
    pub host_restore_list_ok: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunDto {
    pub id: String,
    pub environment: String,
    pub backup_id: Option<String>,
    pub backup_sha256: Option<String>,
    pub status: String,
    pub exit_code: i32,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub total_duration_ms: Option<u64>,
    pub restore_exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub tests_total: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub tests_errors: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestDto {
    pub name: String,
    pub status: String,
    pub value: Option<Value>,
    pub duration_ms: u64,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportDto {
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TimingsDto {
    pub discovery_ms: u64,
    pub startup_ms: u64,
    pub restore_ms: u64,
    pub tests_ms: u64,
    pub total_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupReportDto {
    pub id: String,
    pub provider: String,
    pub size_bytes: u64,
    pub modified_at: String,
    pub sha256: String,
    pub host_restore_list_ok: Option<bool>,
    pub restore_list_ok: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SummaryDto {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentsPayload {
    pub environments: Vec<EnvironmentDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupsPayload {
    pub backups: Vec<BackupDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunsPayload {
    pub runs: Vec<RunDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunDetailPayload {
    pub pending: bool,
    pub run: RunDto,
    pub tests: Vec<TestDto>,
    pub report: Option<ReportDto>,
    pub timings: Option<TimingsDto>,
    pub backup: Option<BackupReportDto>,
    pub summary: Option<SummaryDto>,
}

/// `Option<Value>` explícito para manter `report: null` no detalhe pendente.
pub type NullableReport = Option<ReportDto>;

#[derive(Clone, Debug, Serialize)]
pub struct StartRunPayload {
    pub run_id: String,
    pub status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct CleanupPayload {
    pub containers_removed: usize,
    pub networks_removed: usize,
    pub volumes_removed: usize,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResourcesPayload<T> {
    pub resources: Vec<T>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SavedEnvironmentPayload {
    pub saved: bool,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemovedEnvironmentPayload {
    pub removed: bool,
    pub name: String,
    pub history_preserved: bool,
}

impl EnvironmentDto {
    #[must_use]
    pub fn new(name: &str, config: Option<&EnvironmentConfig>, error: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            config: config.cloned(),
            error,
        }
    }
}

/// Conta os resultados de teste na ordem total, passou, falhou e erro.
#[must_use]
pub fn count_tests(tests: &[TestResultRow]) -> (usize, usize, usize, usize) {
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

/// Constrói o DTO de uma execução do histórico.
#[must_use]
pub fn run_dto(run: &RunRow, detail: Option<&RunDetail>) -> RunDto {
    let (tests_total, tests_passed, tests_failed, tests_errors) =
        detail.map_or((0, 0, 0, 0), |detail| count_tests(&detail.tests));
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

/// DTO de uma execução que ainda não gravou histórico (iniciada agora).
#[must_use]
pub fn run_dto_active(
    id: &str,
    environment: &str,
    status: &str,
    error_message: Option<String>,
    started_at: DateTime<Utc>,
) -> RunDto {
    RunDto {
        id: id.to_string(),
        environment: environment.to_string(),
        backup_id: None,
        backup_sha256: None,
        status: status.to_string(),
        exit_code: 3,
        started_at: started_at.to_rfc3339(),
        finished_at: None,
        total_duration_ms: None,
        restore_exit_code: None,
        error_message,
        tests_total: 0,
        tests_passed: 0,
        tests_failed: 0,
        tests_errors: 0,
    }
}

/// Constrói o DTO de um teste do histórico (somente status e valor escalar).
#[must_use]
pub fn test_dto(test: &TestResultRow) -> TestDto {
    TestDto {
        name: test.name.clone(),
        status: test.status.clone(),
        value: test.value.clone(),
        duration_ms: test.duration_ms,
        message: test.message.clone(),
    }
}

/// Monta o campo `report` do detalhe da execução.
#[must_use]
pub fn report_dto(stored: Option<&ReportRow>, verification_error: Option<&str>) -> ReportDto {
    ReportDto {
        verified: verification_error.is_none(),
        verification_error: verification_error.map(str::to_string),
        sha256: stored.map(|report| report.sha256.clone()),
        signature: stored.map(|report| report.signature.clone()),
        public_key: stored.map(|report| report.public_key.clone()),
        signed_at: stored.map(|report| report.signed_at.clone()),
    }
}

impl From<&Report> for TimingsDto {
    fn from(report: &Report) -> Self {
        Self {
            discovery_ms: report.timings_ms.discovery_ms,
            startup_ms: report.timings_ms.startup_ms,
            restore_ms: report.timings_ms.restore_ms,
            tests_ms: report.timings_ms.tests_ms,
            total_ms: report.timings_ms.total_ms,
        }
    }
}

impl From<&Report> for BackupReportDto {
    fn from(report: &Report) -> Self {
        Self {
            id: report.backup.id.clone(),
            provider: report.backup.provider.clone(),
            size_bytes: report.backup.size_bytes,
            modified_at: report.backup.modified_at.to_rfc3339(),
            sha256: report.backup.sha256.clone(),
            host_restore_list_ok: report.backup.host_restore_list_ok,
            restore_list_ok: report.backup.restore_list_ok,
        }
    }
}

impl From<&Report> for SummaryDto {
    fn from(report: &Report) -> Self {
        Self {
            total: report.summary.total,
            passed: report.summary.passed,
            failed: report.summary.failed,
            errors: report.summary.errors,
        }
    }
}
