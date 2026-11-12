use askama::Template;
use sentinela_core::model::Report;
use sentinela_core::{Result, SentinelaError};

/// Relatório HTML autocontido: CSS inline, sem JavaScript, fontes ou CDNs.
#[derive(Template)]
#[template(path = "report.html")]
struct ReportTemplate {
    run_id: String,
    environment: String,
    status: String,
    exit_code: i32,
    started_at: String,
    finished_at: String,
    backup_id: String,
    backup_path: String,
    backup_provider: String,
    backup_size: String,
    backup_modified: String,
    backup_sha256: String,
    host_restore_list: String,
    restore_list_ok: String,
    discovery_ms: u64,
    startup_ms: u64,
    restore_ms: u64,
    tests_ms: u64,
    total_ms: u64,
    has_restore: bool,
    restore_exit: String,
    restore_duration: String,
    restore_list: String,
    tests: Vec<TestRowView>,
    summary_total: usize,
    summary_passed: usize,
    summary_failed: usize,
    summary_errors: usize,
    has_error: bool,
    error: String,
    sha256: String,
    signature: String,
    public_key: String,
    generated_at: String,
}

struct TestRowView {
    name: String,
    status: String,
    value: String,
    duration_ms: u64,
    message: String,
}

pub fn render(report: &Report, sha256: &str, signature: &str, public_key: &str) -> Result<String> {
    let restore = report.restore.as_ref();
    let template = ReportTemplate {
        run_id: report.run_id.clone(),
        environment: report.environment.clone(),
        status: report.status.as_str().to_string(),
        exit_code: report.exit_code,
        started_at: report.started_at.to_rfc3339(),
        finished_at: report.finished_at.to_rfc3339(),
        backup_id: report.backup.id.clone(),
        backup_path: report.backup.path.clone(),
        backup_provider: report.backup.provider.clone(),
        backup_size: format_bytes(report.backup.size_bytes),
        backup_modified: report.backup.modified_at.to_rfc3339(),
        backup_sha256: report.backup.sha256.clone(),
        host_restore_list: validation_label(report.backup.host_restore_list_ok),
        restore_list_ok: validation_label(report.backup.restore_list_ok),
        discovery_ms: report.timings_ms.discovery_ms,
        startup_ms: report.timings_ms.startup_ms,
        restore_ms: report.timings_ms.restore_ms,
        tests_ms: report.timings_ms.tests_ms,
        total_ms: report.timings_ms.total_ms,
        has_restore: restore.is_some(),
        restore_exit: restore
            .and_then(|restore| restore.exit_code)
            .map_or_else(|| "—".to_string(), |code| code.to_string()),
        restore_duration: restore.map_or_else(
            || "—".to_string(),
            |restore| format!("{} ms", restore.duration_ms),
        ),
        restore_list: restore.map_or_else(
            || "—".to_string(),
            |restore| if restore.list_ok { "ok" } else { "falhou" }.to_string(),
        ),
        tests: report
            .tests
            .iter()
            .map(|test| TestRowView {
                name: test.name.clone(),
                status: test.status.as_str().to_string(),
                value: test
                    .value
                    .map_or_else(|| "—".to_string(), |value| value.display()),
                duration_ms: test.duration_ms,
                message: test.message.clone().unwrap_or_default(),
            })
            .collect(),
        summary_total: report.summary.total,
        summary_passed: report.summary.passed,
        summary_failed: report.summary.failed,
        summary_errors: report.summary.errors,
        has_error: report.error.is_some(),
        error: report.error.clone().unwrap_or_default(),
        sha256: sha256.to_string(),
        signature: signature.to_string(),
        public_key: public_key.to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    };

    template
        .render()
        .map_err(|error| SentinelaError::Report(format!("falha ao renderizar HTML: {error}")))
}

fn validation_label(value: Option<bool>) -> String {
    match value {
        Some(true) => "ok".to_string(),
        Some(false) => "falhou".to_string(),
        None => "não executada no host".to_string(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
