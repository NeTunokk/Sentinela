pub mod html;
pub mod json;
pub mod sign;

use sentinela_core::hash::{sha256_digest, sha256_hex};
use sentinela_core::model::Report;
use sentinela_core::paths::{create_private_dir, write_private_file, Paths};
use sentinela_core::{Result, SentinelaError};
use sentinela_store::ReportArtifacts;

use crate::report::sign::ReportSigner;

/// Gera JSON, HTML, hash e assinatura, e grava todos os artefatos em disco.
pub fn write_reports(
    paths: &Paths,
    report: &Report,
    signer: &ReportSigner,
) -> Result<ReportArtifacts> {
    create_private_dir(&paths.reports_dir())?;

    let json_bytes = json::serialize(report)?;
    let digest = sha256_digest(&json_bytes);
    let sha256 = sha256_hex(&json_bytes);
    let signature = signer.sign_digest(&digest);
    let public_key = signer.public_key_base64().to_string();
    let html = html::render(report, &sha256, &signature, &public_key)?;

    let base = paths.reports_dir().join(&report.run_id);
    let json_path = base.with_extension("json");
    let html_path = base.with_extension("html");
    let sha_path = base.with_extension("sha256");
    let signature_path = base.with_extension("sig");
    let public_path = base.with_extension("pub");

    write_private_file(&json_path, &json_bytes)?;
    write_private_file(&html_path, html.as_bytes())?;
    write_private_file(&sha_path, format!("{sha256}\n").as_bytes())?;
    write_private_file(&signature_path, format!("{signature}\n").as_bytes())?;
    write_private_file(&public_path, format!("{public_key}\n").as_bytes())?;

    Ok(ReportArtifacts {
        json_path,
        html_path,
        sha256,
        signature,
        public_key,
    })
}

/// Recalcula o hash, confere o hash esperado (se houver) e valida a assinatura.
pub fn verify_report_bytes(
    json_bytes: &[u8],
    expected_sha256: Option<&str>,
    signature: &str,
    public_key: &str,
) -> Result<String> {
    let digest = sha256_digest(json_bytes);
    let sha256 = sha256_hex(json_bytes);
    if let Some(expected) = expected_sha256 {
        if !expected.eq_ignore_ascii_case(&sha256) {
            return Err(SentinelaError::Report(format!(
                "SHA-256 do relatório não confere: esperado {expected}, calculado {sha256}"
            )));
        }
    }
    sign::verify_digest(&digest, signature, public_key)?;
    Ok(sha256)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use sentinela_core::model::{
        BackupReport, Report, RestoreReport, RunStatus, TestSummary, TimingsReport,
    };

    use super::*;

    fn sample_report() -> Report {
        Report {
            schema_version: Report::SCHEMA_VERSION,
            run_id: "11111111-1111-4111-8111-111111111111".into(),
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
    fn writes_sidecars_and_verifies_report() {
        let (_temp, paths) = temp_paths();
        let signer = ReportSigner::load_or_create(&paths).unwrap();
        let report = sample_report();
        let artifacts = write_reports(&paths, &report, &signer).unwrap();

        let json = fs::read(&artifacts.json_path).unwrap();
        let sha256 = fs::read_to_string(artifacts.json_path.with_extension("sha256")).unwrap();
        let signature = fs::read_to_string(artifacts.json_path.with_extension("sig")).unwrap();
        let public_key = fs::read_to_string(artifacts.json_path.with_extension("pub")).unwrap();

        let verified = verify_report_bytes(
            &json,
            Some(sha256.trim()),
            signature.trim(),
            public_key.trim(),
        )
        .unwrap();
        assert_eq!(verified, artifacts.sha256);

        let mut tampered = json;
        tampered.push(b' ');
        assert!(verify_report_bytes(
            &tampered,
            Some(sha256.trim()),
            signature.trim(),
            public_key.trim()
        )
        .is_err());
    }
}
