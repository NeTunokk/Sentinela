//! Orquestração de uma prova de restauração.
//!
//! A mesma função é usada pela CLI, pelo painel legado e pelo app desktop.
//! O progresso é enviado por canal e o cancelamento por token; nenhuma linha
//! de dados do cliente entra nos eventos.

use std::future::Future;
use std::time::Instant;

use chrono::Utc;
use sentinela_backup::{provider_for, select_backup, verify_unchanged};
use sentinela_core::config::{load_tests_from_dir, EnvironmentConfig};
use sentinela_core::model::{BackupReport, Report, RunStatus, TestSummary, TimingsReport};
use sentinela_core::paths::{validate_env_name, Paths};
use sentinela_core::report::{write_reports, ReportArtifacts, ReportSigner};
use sentinela_core::{Result, SentinelaError};
use sentinela_store::Store;
use serde::Serialize;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::{cleanup_orphans, DockerSession, RunExecution, SandboxSpec};

/// Fase de uma execução, enviada como evento para a interface.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Discovery,
    BackupSelected,
    Startup,
    Restore,
    Tests,
    Report,
    Finished,
    Failed,
}

/// Evento de progresso: nunca contém linhas de dados nem caminhos do cliente.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RunProgress {
    pub run_id: String,
    pub phase: RunPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl RunProgress {
    fn new(run_id: &str, phase: RunPhase) -> Self {
        Self {
            run_id: run_id.to_string(),
            phase,
            detail: None,
        }
    }

    fn with_detail(run_id: &str, phase: RunPhase, detail: impl Into<String>) -> Self {
        Self {
            run_id: run_id.to_string(),
            phase,
            detail: Some(detail.into()),
        }
    }
}

/// Canal de progresso usado pelos comandos da interface.
pub type ProgressSender = mpsc::UnboundedSender<RunProgress>;

/// Token de cancelamento compartilhado entre a interface e a execução.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    sender: watch::Sender<bool>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sender: watch::Sender::new(false),
        }
    }

    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    /// Resolve quando `cancel()` for chamado (ou imediatamente, se já foi).
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut receiver = self.sender.subscribe();
        while !self.is_cancelled() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Parâmetros de uma execução, sem tipos de CLI e sem I/O de terminal.
pub struct RunRequest {
    pub run_id: String,
    pub environment: String,
    pub backup: Option<String>,
    pub keep: bool,
    pub paths: Paths,
    pub progress: Option<ProgressSender>,
    pub cancel: CancellationToken,
}

/// Resultado persistido de uma execução.
#[derive(Debug)]
pub struct RunOutcome {
    pub report: Report,
    pub artifacts: ReportArtifacts,
}

fn emit(progress: &Option<ProgressSender>, event: RunProgress) {
    if let Some(sender) = progress {
        let _ = sender.send(event);
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Cancela a etapa em andamento removendo todos os recursos do Sentinela.
async fn guarded<F, T>(cancel: &CancellationToken, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::select! {
        result = future => result,
        () = cancel.cancelled() => {
            if let Err(error) = cleanup_orphans().await {
                warn!(error = %error, "falha na limpeza após cancelamento");
            }
            Err(SentinelaError::Interrupted)
        }
    }
}

fn check_cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SentinelaError::Interrupted)
    } else {
        Ok(())
    }
}

fn report_failure(progress: &Option<ProgressSender>, run_id: &str, error: &SentinelaError) {
    emit(
        progress,
        RunProgress::with_detail(run_id, RunPhase::Failed, error.to_string()),
    );
}

/// Executa a prova completa e retorna o relatório já assinado e persistido.
pub async fn execute_run(request: RunRequest) -> Result<RunOutcome> {
    let run_id = request.run_id;
    let paths = request.paths;
    let environment = request.environment;
    let cancel = request.cancel;
    let progress = request.progress;

    emit(&progress, RunProgress::new(&run_id, RunPhase::Discovery));
    check_cancelled(&cancel).inspect_err(|error| report_failure(&progress, &run_id, error))?;

    validate_env_name(&environment)?;
    let started_at = Utc::now();
    let total_started = Instant::now();

    let config = EnvironmentConfig::load(&paths.env_file(&environment))?;
    let tests = load_tests_from_dir(&config.tests.directory)?;

    let store = Store::open(&paths.database_path())?;
    let config_json = serde_json::to_string(&config).map_err(|error| {
        SentinelaError::Store(format!("não foi possível serializar o ambiente: {error}"))
    })?;
    store.upsert_environment(&config.name, &config_json)?;
    drop(store);

    let provider = provider_for(config.backup.provider);
    let discovery_started = Instant::now();
    let backups = provider.discover(&config).await?;
    let discovery_ms = millis(discovery_started.elapsed());
    let backup = select_backup(&backups, request.backup.as_deref())?;
    verify_unchanged(&backup.path, &backup.sha256).await?;
    info!(
        backup = %backup.id,
        sha256 = %backup.sha256,
        "integridade do backup conferida antes do restore"
    );
    emit(
        &progress,
        RunProgress::with_detail(&run_id, RunPhase::BackupSelected, backup.id.clone()),
    );
    check_cancelled(&cancel).inspect_err(|error| report_failure(&progress, &run_id, error))?;

    let spec = SandboxSpec::from_environment(&config, run_id.clone(), backup.path.clone());

    if !request.keep {
        match cleanup_orphans().await {
            Ok(report) => {
                if report.containers_removed > 0
                    || report.networks_removed > 0
                    || report.volumes_removed > 0
                {
                    info!(
                        containers = report.containers_removed,
                        networks = report.networks_removed,
                        volumes = report.volumes_removed,
                        "recursos órfãos removidos"
                    );
                }
                for error in report.errors {
                    warn!(error, "falha ao remover recurso órfão");
                }
            }
            Err(error) => warn!(error = %error, "limpeza de órfãos indisponível; seguindo"),
        }
    }

    let mut session = DockerSession::connect(&run_id)?;
    let mut timings = TimingsReport {
        discovery_ms,
        ..TimingsReport::default()
    };

    emit(&progress, RunProgress::new(&run_id, RunPhase::Startup));
    let execution: Result<RunExecution> = async {
        let startup_started = Instant::now();
        let prepared = guarded(&cancel, session.prepare(&spec)).await;
        timings.startup_ms = millis(startup_started.elapsed());
        prepared?;
        check_cancelled(&cancel)?;

        emit(&progress, RunProgress::new(&run_id, RunPhase::Restore));
        let restore = guarded(&cancel, session.execute_restore(&spec)).await?;
        if restore.list_ok && restore.exit_code == Some(0) {
            emit(&progress, RunProgress::new(&run_id, RunPhase::Tests));
        }
        guarded(
            &cancel,
            session.execute_tests(&spec, restore, &tests, config.tests.statement_timeout_ms),
        )
        .await
    }
    .await;

    if matches!(execution, Err(SentinelaError::Interrupted)) {
        session.mark_cleaned();
        report_failure(&progress, &run_id, &SentinelaError::Interrupted);
        return Err(SentinelaError::Interrupted);
    }

    let teardown_result = session.teardown(request.keep).await;

    let mut status;
    let mut restore_report = None;
    let mut outcomes = Vec::new();
    let mut error;
    match execution {
        Ok(execution) => {
            timings.restore_ms = execution.restore.duration_ms;
            timings.tests_ms = execution.tests_duration_ms;
            status = execution.status;
            restore_report = Some(execution.restore);
            outcomes = execution.tests;
            error = execution.error;
        }
        Err(run_error) => {
            status = RunStatus::InfraError;
            error = Some(run_error.to_string());
        }
    }

    if let Err(teardown_error) = teardown_result {
        warn!(error = %teardown_error, "falha no teardown");
        if status == RunStatus::Ok {
            status = RunStatus::InfraError;
            error = Some(format!("falha no teardown: {teardown_error}"));
        } else {
            error = Some(match error {
                Some(existing) => format!("{existing}; teardown: {teardown_error}"),
                None => format!("teardown: {teardown_error}"),
            });
        }
    }

    timings.total_ms = millis(total_started.elapsed());
    let summary = TestSummary::from_outcomes(&outcomes);
    let report = Report {
        schema_version: Report::SCHEMA_VERSION,
        run_id: run_id.clone(),
        environment: config.name.clone(),
        status,
        exit_code: status.exit_code(),
        started_at,
        finished_at: Utc::now(),
        backup: BackupReport {
            id: backup.id.clone(),
            path: backup.path.display().to_string(),
            provider: backup.provider.clone(),
            size_bytes: backup.size_bytes,
            modified_at: backup.modified_at,
            sha256: backup.sha256.clone(),
            host_restore_list_ok: backup.host_restore_list_ok,
            restore_list_ok: restore_report.as_ref().map(|restore| restore.list_ok),
        },
        timings_ms: timings,
        restore: restore_report,
        tests: outcomes,
        summary,
        error,
    };

    emit(&progress, RunProgress::new(&run_id, RunPhase::Report));
    let signer = ReportSigner::load_or_create(&paths)?;
    let artifacts = write_reports(&paths, &report, &signer)?;

    let mut store = Store::open(&paths.database_path())?;
    store.save_run(&report, &report.tests, &artifacts)?;

    emit(
        &progress,
        RunProgress::with_detail(&run_id, RunPhase::Finished, report.status.as_str()),
    );
    Ok(RunOutcome { report, artifacts })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_is_idempotent_and_immediate() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
        token.cancelled().await;

        let other = CancellationToken::new();
        let waiter = other.clone();
        let handle = tokio::spawn(async move { waiter.cancelled().await });
        other.cancel();
        handle.await.expect("tarefa de cancelamento");
    }

    #[test]
    fn progress_events_are_serializable() {
        let event = RunProgress::with_detail("run-1", RunPhase::BackupSelected, "db.dump");
        let json = serde_json::to_string(&event).expect("serializar evento");
        assert!(json.contains("backup_selected"));
        assert!(json.contains("db.dump"));
    }
}
