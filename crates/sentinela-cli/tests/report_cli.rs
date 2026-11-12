//! Testes do comando `report show` sem Docker.
//!
//! Semeia um relatório assinado no diretório de dados temporário e verifica o
//! caminho real da CLI: leitura do SQLite, sidecars, hash e assinatura.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use base64::Engine as _;
use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use sentinela_core::hash::{sha256_digest, sha256_hex};
use sentinela_core::model::{
    BackupReport, ObservedValue, Report, RestoreReport, RunStatus, TestOutcome, TestStatus,
    TestSummary, TimingsReport,
};
use sentinela_store::{ReportArtifacts, Store};

fn sample_report() -> Report {
    let test = TestOutcome {
        name: "users_exist".into(),
        status: TestStatus::Passed,
        value: Some(ObservedValue::Int(2)),
        duration_ms: 3,
        message: None,
    };
    let summary = TestSummary::from_outcomes(std::slice::from_ref(&test));
    Report {
        schema_version: Report::SCHEMA_VERSION,
        run_id: "22222222-2222-4222-8222-222222222222".into(),
        environment: "producao".into(),
        status: RunStatus::Ok,
        exit_code: 0,
        started_at: Utc::now(),
        finished_at: Utc::now(),
        backup: BackupReport {
            id: "db.dump".into(),
            path: "/tmp/db.dump".into(),
            provider: "logical".into(),
            size_bytes: 10,
            modified_at: Utc::now(),
            sha256: "backup-sha".into(),
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
            duration_ms: 2,
            list_ok: true,
            exit_code: Some(0),
            stderr_tail: None,
        }),
        tests: vec![test],
        summary,
        error: None,
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    config_dir: PathBuf,
    data_dir: PathBuf,
    report_path: PathBuf,
    run_id: String,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("diretório temporário");
    let config_dir = temp.path().join("config");
    let data_dir = temp.path().join("data");
    let reports_dir = data_dir.join("reports");
    fs::create_dir_all(&config_dir).expect("criar config");
    fs::create_dir_all(&reports_dir).expect("criar reports");

    let report = sample_report();
    let run_id = report.run_id.clone();
    let json = serde_json::to_vec_pretty(&report).expect("serializar relatório");
    let digest = sha256_digest(&json);
    let sha256 = sha256_hex(&json);

    let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
    let signature =
        base64::engine::general_purpose::STANDARD.encode(signing_key.sign(&digest).to_bytes());
    let public_key =
        base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key().to_bytes());

    let base = reports_dir.join(&run_id);
    let report_path = base.with_extension("json");
    fs::write(&report_path, &json).expect("escrever JSON");
    fs::write(base.with_extension("html"), "<html></html>").expect("escrever HTML");
    fs::write(base.with_extension("sha256"), format!("{sha256}\n")).expect("escrever sha256");
    fs::write(base.with_extension("sig"), format!("{signature}\n")).expect("escrever sig");
    fs::write(base.with_extension("pub"), format!("{public_key}\n")).expect("escrever pub");

    let mut store = Store::open(&data_dir.join("sentinela.db")).expect("abrir store");
    let artifacts = ReportArtifacts {
        json_path: report_path.clone(),
        html_path: base.with_extension("html"),
        sha256,
        signature,
        public_key,
    };
    store
        .save_run(&report, &report.tests, &artifacts)
        .expect("salvar run");

    Fixture {
        _temp: temp,
        config_dir,
        data_dir,
        report_path,
        run_id,
    }
}

fn cli(fixture: &Fixture, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sentinela"))
        .env("SENTINELA_CONFIG_DIR", &fixture.config_dir)
        .env("SENTINELA_DATA_DIR", &fixture.data_dir)
        .args(arguments)
        .output()
        .expect("executar CLI")
}

#[test]
fn report_show_verifies_database_and_sidecars() {
    let fixture = fixture();

    let shown = cli(&fixture, &["report", "show", &fixture.run_id]);
    assert!(
        shown.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let stdout = String::from_utf8_lossy(&shown.stdout);
    assert!(stdout.contains("assinatura: ok"), "{stdout}");
    assert!(stdout.contains("users_exist"), "{stdout}");

    let raw = cli(&fixture, &["report", "show", &fixture.run_id, "--json"]);
    assert!(raw.status.success());
    let parsed: serde_json::Value =
        serde_json::from_slice(&raw.stdout).expect("JSON do relatório inválido");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["tests"][0]["value"], 2);
}

#[test]
fn report_show_rejects_tampered_json() {
    let fixture = fixture();
    let mut json = fs::read(&fixture.report_path).expect("ler JSON");
    json.extend_from_slice(b" ");
    fs::write(&fixture.report_path, json).expect("adulterar JSON");

    let output = cli(&fixture, &["report", "show", &fixture.run_id]);
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("SHA-256") || stderr.contains("assinatura"),
        "{stderr}"
    );
}

#[test]
fn report_show_rejects_missing_or_unsafe_run_id() {
    let fixture = fixture();

    let missing = cli(&fixture, &["report", "show", "nao-existe"]);
    assert_eq!(missing.status.code(), Some(3));

    let unsafe_id = cli(&fixture, &["report", "show", "../segredo"]);
    assert_eq!(unsafe_id.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&unsafe_id.stderr).contains("run_id inválido"));
}
