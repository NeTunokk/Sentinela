use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sentinela_core::paths::Paths;
use sentinela_core::{Result as CoreResult, SentinelaError};
use sentinela_store::{ReportRow, RunDetail, Store};

use crate::cli::ReportCommand;
use crate::report::verify_report_bytes;

pub fn execute(command: ReportCommand, paths: &Paths) -> Result<i32> {
    match command {
        ReportCommand::Show { run_id, json } => show(&run_id, json, paths),
    }
}

fn show(run_id: &str, raw_json: bool, paths: &Paths) -> Result<i32> {
    validate_run_id(run_id)?;

    let store = Store::open(&paths.database_path())?;
    let detail = store.get_run(run_id)?;

    let (json_path, expected_sha256, signature, public_key) =
        locate_artifacts(paths, run_id, detail.as_ref())?;

    let json_bytes = fs::read(&json_path).map_err(|error| {
        SentinelaError::Report(format!(
            "não foi possível ler o relatório {}: {error}",
            json_path.display()
        ))
    })?;

    let sha256 = verify_report_bytes(
        &json_bytes,
        expected_sha256.as_deref(),
        &signature,
        &public_key,
    )?;

    if raw_json {
        print!("{}", String::from_utf8_lossy(&json_bytes));
        if !json_bytes.ends_with(b"\n") {
            println!();
        }
        return Ok(0);
    }

    println!("Relatório verificado: {}", json_path.display());
    println!("  SHA-256:    {sha256}");
    println!("  assinatura: ok (Ed25519)");
    println!("  chave:      {public_key}");

    if let Some(detail) = detail {
        print_run(&detail);
    }

    let html_path = json_path.with_extension("html");
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

type ArtifactLocation = (PathBuf, Option<String>, String, String);

fn locate_artifacts(
    paths: &Paths,
    run_id: &str,
    detail: Option<&RunDetail>,
) -> CoreResult<ArtifactLocation> {
    let json_path = detail
        .and_then(|detail| detail.report.as_ref())
        .map_or_else(
            || paths.reports_dir().join(format!("{run_id}.json")),
            |report| PathBuf::from(&report.json_path),
        );

    if !json_path.exists() {
        return Err(SentinelaError::Report(format!(
            "relatório não encontrado: {}",
            json_path.display()
        )));
    }

    let stored = detail.and_then(|detail| detail.report.as_ref());
    let expected_sha256 = read_sidecar(&json_path.with_extension("sha256"))
        .or_else(|| stored.map(|report| report.sha256.clone()));
    let signature = read_sidecar(&json_path.with_extension("sig"))
        .or_else(|| stored.map(|report| report.signature.clone()))
        .ok_or_else(|| {
            SentinelaError::Report(format!(
                "assinatura ausente para {} (esperado o sidecar .sig)",
                json_path.display()
            ))
        })?;
    let public_key = read_sidecar(&json_path.with_extension("pub"))
        .or_else(|| stored.map(|report: &ReportRow| report.public_key.clone()))
        .ok_or_else(|| {
            SentinelaError::Report(format!(
                "chave pública ausente para {} (esperado o sidecar .pub)",
                json_path.display()
            ))
        })?;

    Ok((json_path, expected_sha256, signature, public_key))
}

fn read_sidecar(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
}

fn validate_run_id(run_id: &str) -> CoreResult<()> {
    let valid = !run_id.is_empty()
        && run_id.len() <= 64
        && run_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if valid {
        Ok(())
    } else {
        Err(SentinelaError::Config(format!(
            "run_id inválido: {run_id:?}"
        )))
    }
}
