//! Compatibilidade: geração e verificação de relatórios vivem no core.
//!
//! A CLI continua reexportizando os símbolos para não quebrar consumidores
//! externos, mas não há mais nenhuma implementação duplicada aqui.

pub mod sign;

pub use sentinela_core::paths::Paths;
pub use sentinela_core::report::{
    html, json, verify_report_bytes, write_reports, ReportArtifacts, ReportSigner,
};

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
