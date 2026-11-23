use sentinela_core::paths::Paths;

use crate::{dto::report_dto, UiState};

fn temp_state() -> (tempfile::TempDir, UiState) {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = Paths {
        config_dir: directory.path().join("config"),
        data_dir: directory.path().join("data"),
    };
    paths.ensure().expect("ensure");
    (directory, UiState::new(paths))
}

#[test]
fn report_dto_marks_tampering_as_unverified_without_hiding_sidecars() {
    let stored = sentinela_store::ReportRow {
        run_id: "run-1".to_string(),
        json_path: "/tmp/run-1.json".to_string(),
        html_path: "/tmp/run-1.html".to_string(),
        sha256: "abc".to_string(),
        signature: "def".to_string(),
        public_key: "ghi".to_string(),
        signed_at: "2026-01-01T00:00:00Z".to_string(),
    };

    let verified = report_dto(Some(&stored), None);
    assert!(verified.verified);
    assert!(verified.verification_error.is_none());
    assert_eq!(verified.sha256.as_deref(), Some("abc"));

    let tampered = report_dto(Some(&stored), Some("SHA-256 do relatório não confere"));
    assert!(!tampered.verified);
    assert!(tampered
        .verification_error
        .as_deref()
        .is_some_and(|message| message.contains("SHA-256")));
    assert_eq!(tampered.signature.as_deref(), Some("def"));
    assert_eq!(tampered.public_key.as_deref(), Some("ghi"));
}

#[tokio::test]
async fn get_run_rejects_invalid_run_id() {
    let (_temp, state) = temp_state();
    assert!(state.get_run("../segredo").await.is_err());
    assert!(state.cancel_run("id com espaco").await.is_err());
}

#[tokio::test]
async fn cancel_run_reports_when_there_is_no_active_run() {
    let (_temp, state) = temp_state();
    assert!(!state.cancel_run("run-1").await.expect("cancel"));
}
