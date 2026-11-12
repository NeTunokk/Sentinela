use anyhow::Result;
use sentinela_runner::cleanup_orphans;
use tracing::warn;

pub async fn execute() -> Result<i32> {
    let report = cleanup_orphans().await?;

    println!(
        "Recursos removidos: {} container(s), {} rede(s), {} volume(s).",
        report.containers_removed, report.networks_removed, report.volumes_removed
    );

    if report.errors.is_empty() {
        return Ok(0);
    }

    for error in &report.errors {
        warn!(error, "falha ao remover recurso órfão");
        eprintln!("falha ao remover recurso órfão: {error}");
    }
    eprintln!(
        "{} recurso(s) não puderam ser removidos.",
        report.errors.len()
    );
    Ok(3)
}
