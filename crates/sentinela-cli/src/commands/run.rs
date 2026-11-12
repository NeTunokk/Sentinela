use std::time::Instant;

use anyhow::Result;
use chrono::Utc;
use sentinela_backup::{provider_for, select_backup};
use sentinela_core::config::{load_tests_from_dir, EnvironmentConfig};
use sentinela_core::model::{BackupReport, Report, RunStatus, TestSummary, TimingsReport};
use sentinela_core::paths::{validate_env_name, Paths};
use sentinela_core::{Result as CoreResult, SentinelaError};
use sentinela_runner::{cleanup_orphans, DockerSession, RunExecution, SandboxSpec};
use sentinela_store::Store;
use tracing::{info, warn};
use uuid::Uuid;

use crate::cli::RunArgs;
use crate::report::sign::ReportSigner;

pub async fn execute(args: RunArgs, paths: &Paths) -> Result<i32> {
    execute_with_run_id(args, paths, Uuid::new_v4().to_string()).await
}

/// Executa uma prova de restauração usando um `run_id` já conhecido.
///
/// O servidor do painel usa esta variante para responder imediatamente qual
/// execução foi enfileirada e acompanhar o resultado no histórico.
pub async fn execute_with_run_id(args: RunArgs, paths: &Paths, run_id: String) -> Result<i32> {
    validate_env_name(&args.env)?;
    let started_at = Utc::now();
    let total_started = Instant::now();

    let config = EnvironmentConfig::load(&paths.env_file(&args.env))?;
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
    let backup = select_backup(&backups, args.backup.as_deref())?;

    let spec = SandboxSpec::from_environment(&config, run_id.clone(), backup.path.clone());

    if !args.keep {
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

    let execution: CoreResult<RunExecution> = tokio::select! {
        result = async {
            let startup_started = Instant::now();
            let prepared = session.prepare(&spec).await;
            timings.startup_ms = millis(startup_started.elapsed());
            prepared?;
            session
                .execute(&spec, &tests, config.tests.statement_timeout_ms)
                .await
        } => result,
        signal = crate::signals::shutdown() => {
            warn!(signal, "sinal recebido; iniciando teardown");
            Err(SentinelaError::Interrupted)
        }
    };

    let teardown_result = session.teardown(args.keep).await;

    let mut status;
    let mut restore = None;
    let mut outcomes = Vec::new();
    let mut error;
    match execution {
        Ok(execution) => {
            timings.restore_ms = execution.restore.duration_ms;
            timings.tests_ms = execution.tests_duration_ms;
            status = execution.status;
            restore = Some(execution.restore);
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
    let finished_at = Utc::now();
    let summary = TestSummary::from_outcomes(&outcomes);

    let report = Report {
        schema_version: Report::SCHEMA_VERSION,
        run_id: run_id.clone(),
        environment: config.name.clone(),
        status,
        exit_code: status.exit_code(),
        started_at,
        finished_at,
        backup: BackupReport {
            id: backup.id.clone(),
            path: backup.path.display().to_string(),
            provider: backup.provider.clone(),
            size_bytes: backup.size_bytes,
            modified_at: backup.modified_at,
            sha256: backup.sha256.clone(),
            host_restore_list_ok: backup.host_restore_list_ok,
            restore_list_ok: restore.as_ref().map(|restore| restore.list_ok),
        },
        timings_ms: timings,
        restore,
        tests: outcomes,
        summary,
        error,
    };

    if let Some(stderr) = report
        .restore
        .as_ref()
        .and_then(|restore| restore.stderr_tail.as_deref())
        .filter(|stderr| !stderr.trim().is_empty())
    {
        warn!(
            stderr = %stderr,
            "stderr do pg_restore (exibido apenas no terminal; não é persistido no relatório)"
        );
    }

    let signer = ReportSigner::load_or_create(paths)?;
    let artifacts = crate::report::write_reports(paths, &report, &signer)?;

    let mut store = Store::open(&paths.database_path())?;
    store.save_run(&report, &report.tests, &artifacts)?;

    println!();
    println!("Run {} concluído: {}", report.run_id, report.status);
    println!("  environment: {}", report.environment);
    println!(
        "  backup:      {} ({})",
        report.backup.id, report.backup.sha256
    );
    println!(
        "  testes:      {}/{} passaram ({} falhas, {} erros)",
        report.summary.passed, report.summary.total, report.summary.failed, report.summary.errors
    );
    println!("  duração:     {} ms", report.timings_ms.total_ms);
    if let Some(error) = &report.error {
        println!("  erro:        {error}");
    }
    println!("  JSON:        {}", artifacts.json_path.display());
    println!("  HTML:        {}", artifacts.html_path.display());
    println!("  SHA-256:     {}", artifacts.sha256);
    println!("  assinatura:  {}", artifacts.signature);

    Ok(report.exit_code)
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
