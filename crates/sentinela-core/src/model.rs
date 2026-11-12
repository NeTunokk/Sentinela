use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

/// Status final de uma execução.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    TestsFailed,
    RestoreFailed,
    InfraError,
}

impl RunStatus {
    #[must_use]
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::TestsFailed => 1,
            Self::RestoreFailed => 2,
            Self::InfraError => 3,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::TestsFailed => "tests_failed",
            Self::RestoreFailed => "restore_failed",
            Self::InfraError => "infra_error",
        }
    }
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Status de um teste individual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Passed,
    Failed,
    Error,
}

impl TestStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
        }
    }
}

/// Valor escalar observado. Somente booleano ou numérico é persistido.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ObservedValue {
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl ObservedValue {
    #[must_use]
    pub fn to_json(self) -> Value {
        match self {
            Self::Int(value) => Value::Number(Number::from(value)),
            Self::Float(value) => Number::from_f64(value).map_or(Value::Null, Value::Number),
            Self::Bool(value) => Value::Bool(value),
        }
    }

    #[must_use]
    pub fn display(self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
        }
    }
}

/// Resultado de um teste SQL. Nunca contém linhas de dados do cliente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestOutcome {
    pub name: String,
    pub status: TestStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ObservedValue>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl TestOutcome {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.status == TestStatus::Passed
    }
}

/// Metadados de um backup descoberto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupInfo {
    pub id: String,
    pub path: PathBuf,
    pub provider: String,
    pub size_bytes: u64,
    pub modified_at: DateTime<Utc>,
    pub sha256: String,
    /// Resultado de `pg_restore --list` no host, quando a ferramenta existe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_restore_list_ok: Option<bool>,
}

/// Backup conforme aparece no relatório, sem expor o conteúdo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupReport {
    pub id: String,
    pub path: String,
    pub provider: String,
    pub size_bytes: u64,
    pub modified_at: DateTime<Utc>,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_restore_list_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_list_ok: Option<bool>,
}

/// Resultado da fase de restauração.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreReport {
    pub duration_ms: u64,
    pub list_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Diagnóstico mantido apenas em memória e impresso no terminal. Nunca é
    /// persistido no relatório para não gravar valores de linhas do cliente.
    #[serde(skip)]
    pub stderr_tail: Option<String>,
}

/// Tempos medidos com `std::time::Instant`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingsReport {
    pub discovery_ms: u64,
    pub startup_ms: u64,
    pub restore_ms: u64,
    pub tests_ms: u64,
    pub total_ms: u64,
}

/// Resumo dos testes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
}

impl TestSummary {
    #[must_use]
    pub fn from_outcomes(outcomes: &[TestOutcome]) -> Self {
        let mut summary = Self {
            total: outcomes.len(),
            ..Self::default()
        };
        for outcome in outcomes {
            match outcome.status {
                TestStatus::Passed => summary.passed += 1,
                TestStatus::Failed => summary.failed += 1,
                TestStatus::Error => summary.errors += 1,
            }
        }
        summary
    }
}

/// Relatório JSON canônico, posteriormente hasheado e assinado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub run_id: String,
    pub environment: String,
    pub status: RunStatus,
    pub exit_code: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub backup: BackupReport,
    pub timings_ms: TimingsReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreReport>,
    pub tests: Vec<TestOutcome>,
    pub summary: TestSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Report {
    pub const SCHEMA_VERSION: u32 = 1;
}
