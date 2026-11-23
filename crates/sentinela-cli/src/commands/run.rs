use anyhow::Result;
use sentinela_core::paths::Paths;
use sentinela_core::SentinelaError;
use sentinela_runner::{execute_run, CancellationToken, RunRequest};
use tracing::warn;
use uuid::Uuid;

use crate::cli::RunArgs;

pub async fn execute(args: RunArgs, paths: &Paths) -> Result<i32> {
    execute_with_run_id(args, paths, Uuid::new_v4().to_string()).await
}

/// Executa uma prova de restauração usando um `run_id` já conhecido.
///
/// A orquestração vive em `sentinela-runner`; aqui só convertemos os argumentos
/// da CLI e imprimimos o resumo no terminal.
pub async fn execute_with_run_id(args: RunArgs, paths: &Paths, run_id: String) -> Result<i32> {
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        let signal = crate::signals::shutdown().await;
        warn!(signal, "sinal recebido; cancelando execução");
        signal_cancel.cancel();
    });

    let outcome = execute_run(RunRequest {
        run_id,
        environment: args.env,
        backup: args.backup,
        keep: args.keep,
        paths: paths.clone(),
        progress: None,
        cancel,
    })
    .await
    .map_err(|error| {
        if matches!(error, SentinelaError::Interrupted) {
            anyhow::anyhow!("execução interrompida; recursos Docker removidos")
        } else {
            anyhow::anyhow!(error)
        }
    })?;

    let report = &outcome.report;
    let artifacts = &outcome.artifacts;
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
