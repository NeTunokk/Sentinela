//! Teste ponta a ponta do Sentinela.
//!
//! Requer Docker em execução e a imagem `postgres:16` já disponível
//! localmente (o Sentinela nunca faz pull). O teste é marcado com `#[ignore]`
//! para não quebrar `cargo test` em máquinas sem Docker.
//!
//! Execute com:
//!
//! ```text
//! cargo test -p sentinela-cli --test docker_e2e -- --ignored --nocapture
//! ```

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const IMAGE: &str = "postgres:16";
const SOURCE_PASSWORD: &str = "sentinela-e2e-source";
const READINESS_TIMEOUT: Duration = Duration::from_secs(120);

struct SourceContainer {
    name: String,
}

impl SourceContainer {
    fn start() -> Self {
        let name = format!("sentinela-e2e-source-{}", std::process::id());
        let status = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "-e",
                &format!("POSTGRES_PASSWORD={SOURCE_PASSWORD}"),
                "-e",
                "POSTGRES_DB=sentinela",
                IMAGE,
            ])
            .status()
            .expect("falha ao executar docker run");
        assert!(
            status.success(),
            "não foi possível iniciar container de origem"
        );

        let container = Self { name };
        container.wait_until_ready();
        container
    }

    fn wait_until_ready(&self) {
        let started = Instant::now();
        loop {
            let status = Command::new("docker")
                .args([
                    "exec",
                    &self.name,
                    "pg_isready",
                    "-U",
                    "postgres",
                    "-d",
                    "sentinela",
                ])
                .status()
                .expect("falha ao executar pg_isready");
            if status.success() {
                return;
            }
            assert!(
                started.elapsed() < READINESS_TIMEOUT,
                "timeout aguardando o PostgreSQL de origem"
            );
            thread::sleep(Duration::from_millis(500));
        }
    }

    fn exec(&self, arguments: &[&str]) -> Output {
        let output = Command::new("docker")
            .arg("exec")
            .arg(&self.name)
            .args(arguments)
            .output()
            .expect("falha ao executar comando no container de origem");
        assert!(
            output.status.success(),
            "comando {:?} falhou: {}",
            arguments,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

impl Drop for SourceContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.name])
            .status();
    }
}

#[test]
#[ignore = "requer Docker e a imagem postgres:16 local"]
fn restores_backup_and_generates_signed_report() {
    assert_docker_and_image();

    let temp = tempfile::tempdir().expect("falha ao criar diretório temporário");
    let backups_dir = temp.path().join("backups");
    let tests_dir = temp.path().join("tests");
    let config_dir = temp.path().join("config");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&backups_dir).expect("criar diretório de backups");
    fs::create_dir_all(&tests_dir).expect("criar diretório de testes");

    fs::write(
        tests_dir.join("basics.yaml"),
        r#"
tests:
  - name: users_exist
    sql: SELECT count(*) FROM users
    expect: { op: gte, value: 2 }
  - name: users_have_email
    sql: SELECT bool_and(email IS NOT NULL) FROM users
    expect: { op: is_true }
  - name: emails_are_unique
    sql: SELECT count(DISTINCT email) = count(*) FROM users
    expect: { op: is_true }
"#,
    )
    .expect("escrever testes");

    let source = SourceContainer::start();
    source.exec(&[
        "psql",
        "-U",
        "postgres",
        "-d",
        "sentinela",
        "-v",
        "ON_ERROR_STOP=1",
        "-c",
        "CREATE TABLE users (id serial PRIMARY KEY, email text NOT NULL UNIQUE);",
        "-c",
        "INSERT INTO users (email) VALUES ('alice@example.com'), ('bob@example.com');",
    ]);

    let dump = source.exec(&["pg_dump", "-U", "postgres", "-d", "sentinela", "-Fc"]);
    let backup_path = backups_dir.join("e2e.dump");
    fs::write(&backup_path, &dump.stdout).expect("escrever backup de teste");
    assert!(!dump.stdout.is_empty(), "pg_dump produziu arquivo vazio");

    let config = config_dir.to_string_lossy().into_owned();
    let data = data_dir.to_string_lossy().into_owned();

    assert_success(
        cli(
            &config,
            &data,
            &[
                "env",
                "add",
                "e2e",
                "--backup-dir",
                &backups_dir.to_string_lossy(),
                "--tests-dir",
                &tests_dir.to_string_lossy(),
                "--postgres-image",
                IMAGE,
                "--postgres-user",
                "postgres",
                "--database",
                "sentinela",
                "--readiness-timeout-secs",
                "120",
                "--restore-timeout-secs",
                "120",
            ],
        ),
        "env add",
    );

    assert_success(
        cli(&config, &data, &["backups", "list", "--env", "e2e"]),
        "backups list",
    );
    assert_success(cli(&config, &data, &["run", "--env", "e2e"]), "run");

    let history = cli(&config, &data, &["history", "--env", "e2e"]);
    assert_success(history.clone(), "history");
    assert!(
        String::from_utf8_lossy(&history.stdout).contains("ok"),
        "histórico deveria exibir a run com status ok"
    );

    let report_path = single_report(&data_dir);
    let run_id = report_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("run_id inválido")
        .to_string();

    let shown = cli(&config, &data, &["report", "show", &run_id]);
    assert_success(shown.clone(), "report show");
    let shown_stdout = String::from_utf8_lossy(&shown.stdout);
    assert!(
        shown_stdout.contains("assinatura: ok"),
        "relatório deveria estar assinado: {shown_stdout}"
    );

    let json = cli(&config, &data, &["report", "show", &run_id, "--json"]);
    assert_success(json.clone(), "report show --json");
    let parsed: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("JSON do relatório inválido");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["summary"]["passed"], 3);
    assert!(parsed["tests"]
        .as_array()
        .is_some_and(|tests| tests.len() == 3));

    for extension in ["json", "html", "sha256", "sig", "pub"] {
        let path = report_path.with_extension(extension);
        assert!(path.exists(), "artefato ausente: {}", path.display());
    }

    // Alterar o JSON deve invalidar hash e assinatura, mesmo com sidecars presentes.
    let mut tampered = fs::read(&report_path).expect("ler relatório");
    tampered.extend_from_slice(b" ");
    fs::write(&report_path, tampered).expect("adulterar relatório");

    let tampered_output = cli(&config, &data, &["report", "show", &run_id]);
    assert!(
        !tampered_output.status.success(),
        "report show deveria falhar com relatório adulterado"
    );
    assert_eq!(tampered_output.status.code(), Some(3));
    let tampered_stderr = String::from_utf8_lossy(&tampered_output.stderr);
    assert!(
        tampered_stderr.contains("SHA-256") || tampered_stderr.contains("assinatura"),
        "erro inesperado: {tampered_stderr}"
    );

    assert_success(cli(&config, &data, &["cleanup"]), "cleanup");
}

fn assert_docker_and_image() {
    let docker = Command::new("docker")
        .arg("info")
        .output()
        .expect("Docker não encontrado no PATH");
    assert!(
        docker.status.success(),
        "Docker não está acessível: {}",
        String::from_utf8_lossy(&docker.stderr)
    );

    let image = Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .output()
        .expect("falha ao consultar a imagem");
    assert!(
        image.status.success(),
        "imagem {IMAGE} não existe localmente; baixe-a antes do teste (o Sentinela não faz pull)"
    );
}

fn cli(config_dir: &str, data_dir: &str, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sentinela"))
        .env("SENTINELA_CONFIG_DIR", config_dir)
        .env("SENTINELA_DATA_DIR", data_dir)
        .args(arguments)
        .output()
        .expect("falha ao executar a CLI")
}

fn assert_success(output: Output, context: &str) {
    if output.status.success() {
        return;
    }
    let mut stderr = std::io::stderr();
    let _ = writeln!(
        stderr,
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let _ = writeln!(
        stderr,
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    panic!("{context} falhou com código {:?}", output.status.code());
}

fn single_report(data_dir: &Path) -> PathBuf {
    let reports = data_dir.join("reports");
    let mut files = fs::read_dir(&reports)
        .expect("listar relatórios")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(files.len(), 1, "esperado exatamente um relatório JSON");
    files.remove(0)
}
