//! Persistência local do histórico do Sentinela em SQLite.
//!
//! O banco guarda configurações de ambientes, execuções, resultados de testes
//! (somente status/valor escalar/duração) e metadados de relatórios assinados.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use sentinela_core::model::{Report, TestOutcome};
use sentinela_core::{Result, SentinelaError};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS environments (
    name TEXT PRIMARY KEY,
    config_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    environment TEXT NOT NULL,
    backup_id TEXT,
    backup_sha256 TEXT,
    status TEXT NOT NULL,
    exit_code INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    total_duration_ms INTEGER,
    restore_exit_code INTEGER,
    report_json_path TEXT,
    report_html_path TEXT,
    report_sha256 TEXT,
    report_signature TEXT,
    error_message TEXT
);

CREATE TABLE IF NOT EXISTS test_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    observed_value TEXT,
    duration_ms INTEGER NOT NULL,
    message TEXT,
    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS reports (
    run_id TEXT PRIMARY KEY,
    json_path TEXT NOT NULL,
    html_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    signature TEXT NOT NULL,
    public_key TEXT NOT NULL,
    signed_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_runs_environment ON runs(environment, started_at);
CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status, started_at);
CREATE INDEX IF NOT EXISTS idx_test_results_run ON test_results(run_id);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentRow {
    pub name: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
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
    pub report_json_path: Option<String>,
    pub report_html_path: Option<String>,
    pub report_sha256: Option<String>,
    pub report_signature: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestResultRow {
    pub name: String,
    pub status: String,
    pub value: Option<serde_json::Value>,
    pub duration_ms: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRow {
    pub run_id: String,
    pub json_path: String,
    pub html_path: String,
    pub sha256: String,
    pub signature: String,
    pub public_key: String,
    pub signed_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunDetail {
    pub run: RunRow,
    pub tests: Vec<TestResultRow>,
    pub report: Option<ReportRow>,
}

/// Artefatos assinados de um relatório gravados em disco.
#[derive(Debug, Clone)]
pub struct ReportArtifacts {
    pub json_path: PathBuf,
    pub html_path: PathBuf,
    pub sha256: String,
    pub signature: String,
    pub public_key: String,
}

/// Conexão SQLite local. Não é `Send`; use fora de pontos de await.
pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                SentinelaError::Store(format!(
                    "não foi possível criar {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let connection = Connection::open(path).map_err(store_error("abrir banco"))?;
        Self::initialize(connection)
    }

    #[cfg(test)]
    fn open_in_memory() -> Result<Self> {
        let connection =
            Connection::open_in_memory().map_err(store_error("abrir banco em memória"))?;
        Self::initialize(connection)
    }

    fn initialize(connection: Connection) -> Result<Self> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(store_error("configurar busy_timeout"))?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(store_error("ativar foreign_keys"))?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(store_error("ativar WAL"))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(store_error("criar schema"))?;
        Ok(Self { connection })
    }

    pub fn upsert_environment(&self, name: &str, config_json: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.connection
            .execute(
                "INSERT INTO environments (name, config_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(name) DO UPDATE SET
                    config_json = excluded.config_json,
                    updated_at = excluded.updated_at",
                params![name, config_json, now],
            )
            .map(|_| ())
            .map_err(store_error("salvar ambiente"))
    }

    pub fn remove_environment(&self, name: &str) -> Result<bool> {
        self.connection
            .execute("DELETE FROM environments WHERE name = ?1", params![name])
            .map(|rows| rows > 0)
            .map_err(store_error("remover ambiente"))
    }

    pub fn list_environments(&self) -> Result<Vec<EnvironmentRow>> {
        let mut statement = self
            .connection
            .prepare("SELECT name, updated_at FROM environments ORDER BY name")
            .map_err(store_error("preparar listagem de ambientes"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(EnvironmentRow {
                    name: row.get(0)?,
                    updated_at: row.get(1)?,
                })
            })
            .map_err(store_error("listar ambientes"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(store_error("ler ambientes"))
    }

    /// Grava run, testes e relatório em uma única transação.
    pub fn save_run(
        &mut self,
        report: &Report,
        tests: &[TestOutcome],
        artifacts: &ReportArtifacts,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(store_error("iniciar transação de persistência"))?;

        let restore_exit_code = report
            .restore
            .as_ref()
            .and_then(|restore| restore.exit_code);
        transaction
            .execute(
                "INSERT INTO runs (
                    id, environment, backup_id, backup_sha256, status, exit_code,
                    started_at, finished_at, total_duration_ms, restore_exit_code,
                    report_json_path, report_html_path, report_sha256, report_signature,
                    error_message
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    report.run_id,
                    report.environment,
                    report.backup.id,
                    report.backup.sha256,
                    report.status.as_str(),
                    report.exit_code,
                    report.started_at.to_rfc3339(),
                    report.finished_at.to_rfc3339(),
                    u64_to_i64(report.timings_ms.total_ms),
                    restore_exit_code,
                    artifacts.json_path.to_string_lossy(),
                    artifacts.html_path.to_string_lossy(),
                    artifacts.sha256,
                    artifacts.signature,
                    report.error,
                ],
            )
            .map_err(store_error("salvar run"))?;

        for outcome in tests {
            let value = outcome
                .value
                .map(|value| serde_json::to_string(&value))
                .transpose()
                .map_err(|error| {
                    SentinelaError::Store(format!("serializar valor de teste: {error}"))
                })?;
            transaction
                .execute(
                    "INSERT INTO test_results
                        (run_id, name, status, observed_value, duration_ms, message)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        report.run_id,
                        outcome.name,
                        outcome.status.as_str(),
                        value,
                        u64_to_i64(outcome.duration_ms),
                        outcome.message,
                    ],
                )
                .map_err(store_error("salvar resultado de teste"))?;
        }

        transaction
            .execute(
                "INSERT INTO reports
                    (run_id, json_path, html_path, sha256, signature, public_key, signed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    report.run_id,
                    artifacts.json_path.to_string_lossy(),
                    artifacts.html_path.to_string_lossy(),
                    artifacts.sha256,
                    artifacts.signature,
                    artifacts.public_key,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(store_error("salvar relatório"))?;

        transaction
            .commit()
            .map_err(store_error("confirmar transação de persistência"))
    }

    pub fn list_runs(
        &self,
        environment: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<RunRow>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, environment, backup_id, backup_sha256, status, exit_code,
                        started_at, finished_at, total_duration_ms, restore_exit_code,
                        report_json_path, report_html_path, report_sha256,
                        report_signature, error_message
                 FROM runs
                 WHERE (?1 IS NULL OR environment = ?1)
                   AND (?2 IS NULL OR (?2 = 'failed' AND status != 'ok') OR status = ?2)
                 ORDER BY started_at DESC",
            )
            .map_err(store_error("preparar histórico"))?;
        let rows = statement
            .query_map(params![environment, status], map_run_row)
            .map_err(store_error("listar histórico"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(store_error("ler histórico"))
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<RunDetail>> {
        let run = self
            .connection
            .query_row(
                "SELECT id, environment, backup_id, backup_sha256, status, exit_code,
                        started_at, finished_at, total_duration_ms, restore_exit_code,
                        report_json_path, report_html_path, report_sha256,
                        report_signature, error_message
                 FROM runs WHERE id = ?1",
                params![run_id],
                map_run_row,
            )
            .optional()
            .map_err(store_error("buscar run"))?;

        let Some(run) = run else {
            return Ok(None);
        };

        let mut statement = self
            .connection
            .prepare(
                "SELECT name, status, observed_value, duration_ms, message
                 FROM test_results WHERE run_id = ?1 ORDER BY id",
            )
            .map_err(store_error("preparar resultados do run"))?;
        let tests = statement
            .query_map(params![run_id], |row| {
                let raw_value: Option<String> = row.get(2)?;
                Ok(TestResultRow {
                    name: row.get(0)?,
                    status: row.get(1)?,
                    value: raw_value.and_then(|raw| serde_json::from_str(&raw).ok()),
                    duration_ms: i64_to_u64(row.get(3)?),
                    message: row.get(4)?,
                })
            })
            .map_err(store_error("listar resultados do run"))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(store_error("ler resultados do run"))?;

        let report = self
            .connection
            .query_row(
                "SELECT run_id, json_path, html_path, sha256, signature, public_key, signed_at
                 FROM reports WHERE run_id = ?1",
                params![run_id],
                |row| {
                    Ok(ReportRow {
                        run_id: row.get(0)?,
                        json_path: row.get(1)?,
                        html_path: row.get(2)?,
                        sha256: row.get(3)?,
                        signature: row.get(4)?,
                        public_key: row.get(5)?,
                        signed_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(store_error("buscar relatório do run"))?;

        Ok(Some(RunDetail { run, tests, report }))
    }
}

fn map_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: row.get(0)?,
        environment: row.get(1)?,
        backup_id: row.get(2)?,
        backup_sha256: row.get(3)?,
        status: row.get(4)?,
        exit_code: row.get(5)?,
        started_at: row.get(6)?,
        finished_at: row.get(7)?,
        total_duration_ms: row.get::<_, Option<i64>>(8)?.map(i64_to_u64),
        restore_exit_code: row.get(9)?,
        report_json_path: row.get(10)?,
        report_html_path: row.get(11)?,
        report_sha256: row.get(12)?,
        report_signature: row.get(13)?,
        error_message: row.get(14)?,
    })
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn store_error(context: &'static str) -> impl Fn(rusqlite::Error) -> SentinelaError {
    move |error| SentinelaError::Store(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sentinela_core::model::{
        BackupReport, ObservedValue, Report, RestoreReport, RunStatus, TestOutcome, TestStatus,
        TestSummary, TimingsReport,
    };

    use super::*;

    fn report() -> Report {
        Report {
            schema_version: Report::SCHEMA_VERSION,
            run_id: "run-1".into(),
            environment: "prod".into(),
            status: RunStatus::Ok,
            exit_code: 0,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            backup: BackupReport {
                id: "db.dump".into(),
                path: "/tmp/db.dump".into(),
                provider: "logical".into(),
                size_bytes: 123,
                modified_at: Utc::now(),
                sha256: "abc".into(),
                host_restore_list_ok: Some(true),
                restore_list_ok: Some(true),
            },
            timings_ms: TimingsReport {
                discovery_ms: 1,
                startup_ms: 2,
                restore_ms: 3,
                tests_ms: 4,
                total_ms: 10,
            },
            restore: Some(RestoreReport {
                duration_ms: 3,
                list_ok: true,
                exit_code: Some(0),
                stderr_tail: None,
            }),
            tests: Vec::new(),
            summary: TestSummary::default(),
            error: None,
        }
    }

    #[test]
    fn persists_and_reads_run() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_environment("prod", "{}").unwrap();
        assert_eq!(store.list_environments().unwrap().len(), 1);

        let report = report();
        let test = TestOutcome {
            name: "users".into(),
            status: TestStatus::Passed,
            value: Some(ObservedValue::Int(7)),
            duration_ms: 12,
            message: None,
        };
        let artifacts = ReportArtifacts {
            json_path: "/tmp/run-1.json".into(),
            html_path: "/tmp/run-1.html".into(),
            sha256: "hash".into(),
            signature: "sig".into(),
            public_key: "pub".into(),
        };
        store
            .save_run(&report, std::slice::from_ref(&test), &artifacts)
            .unwrap();

        let runs = store.list_runs(Some("prod"), Some("ok")).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "run-1");

        let detail = store.get_run("run-1").unwrap().unwrap();
        assert_eq!(detail.tests.len(), 1);
        assert_eq!(detail.tests[0].value, Some(serde_json::json!(7)));
        assert_eq!(detail.report.unwrap().sha256, "hash");
    }
}
