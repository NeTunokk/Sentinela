use std::fs;

use anyhow::Result;
use sentinela_core::paths::Paths;
use sentinela_core::report::{self as report_core, StoredReportRef};
use sentinela_core::SentinelaError;
use sentinela_store::{RunDetail, Store};

use crate::cli::ReportCommand;
use crate::report::verify_report_bytes;

pub fn execute(command: ReportCommand, paths: &Paths) -> Result<i32> {
    match command {
        ReportCommand::Show { run_id, json } => show(&run_id, json, paths),
    }
}

fn show(run_id: &str, raw_json: bool, paths: &Paths) -> Result<i32> {
    report_core::validate_run_id(run_id)?;

    let store = Store::open(&paths.database_path())?;
    let detail = store.get_run(run_id)?;
    let stored = detail
        .as_ref()
        .and_then(|detail| detail.report.as_ref())
        .map(|report| {
            StoredReportRef::new(
                &report.json_path,
                &report.sha256,
                &report.signature,
                &report.public_key,
            )
        });

    let location = report_core::locate_artifacts(paths, run_id, stored.as_ref())?;
    if !location.json_path.exists() {
        return Err(SentinelaError::Report(format!(
            "relatório não encontrado: {}",
            location.json_path.display()
        ))
        .into());
    }

    let json_bytes = fs::read(&location.json_path).map_err(|error| {
        SentinelaError::Report(format!(
            "não foi possível ler o relatório {}: {error}",
            location.json_path.display()
        ))
    })?;

    let sha256 = verify_report_bytes(
        &json_bytes,
        location.expected_sha256.as_deref(),
        &location.signature,
        &location.public_key,
    )?;

    if raw_json {
        print!("{}", String::from_utf8_lossy(&json_bytes));
        if !json_bytes.ends_with(b"\n") {
            println!();
        }
        return Ok(0);
    }

    println!("Relatório verificado: {}", location.json_path.display());
    println!("  SHA-256:    {sha256}");
    println!("  assinatura: ok (Ed25519)");
    println!("  chave:      {}", location.public_key);

    if let Some(detail) = detail {
        print_run(&detail);
    }

    let html_path = location.html_path();
    if html_path.exists() {
        println!("  HTML:       {}", html_path.display());
    }

    Ok(0)
}

fn print_run(detail: &RunDetail) {
    println!();
    println!("Run {}", detail.run.id);
    println!("  ambiente:   {}", detail.run.environment);
    println!(
        "  status:     {} (exit {})",
        detail.run.status, detail.run.exit_code
    );
    if let Some(backup) = &detail.run.backup_id {
        println!("  backup:     {backup}");
    }
    if let Some(sha256) = &detail.run.backup_sha256 {
        println!("  backup SHA: {sha256}");
    }
    println!("  início:     {}", detail.run.started_at);
    if let Some(finished) = &detail.run.finished_at {
        println!("  fim:        {finished}");
    }
    if let Some(total) = detail.run.total_duration_ms {
        println!("  duração:    {total} ms");
    }
    if let Some(error) = &detail.run.error_message {
        println!("  erro:       {error}");
    }

    if !detail.tests.is_empty() {
        println!();
        println!(
            "  {:<32} {:<8} {:>12} {:>10}",
            "TESTE", "STATUS", "VALOR", "DURAÇÃO"
        );
        for test in &detail.tests {
            let value = test
                .value
                .as_ref()
                .map_or_else(|| "—".to_string(), ToString::to_string);
            println!(
                "  {:<32} {:<8} {:>12} {:>8} ms",
                test.name, test.status, value, test.duration_ms
            );
            if let Some(message) = &test.message {
                println!("    {}", message);
            }
        }
    }
}
