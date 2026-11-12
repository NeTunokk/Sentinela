use anyhow::Result;
use sentinela_backup::provider_for;
use sentinela_core::config::EnvironmentConfig;
use sentinela_core::paths::Paths;

use crate::cli::BackupsCommand;

use super::format_bytes;

pub async fn execute(command: BackupsCommand, paths: &Paths) -> Result<i32> {
    match command {
        BackupsCommand::List { env } => list(&env, paths).await,
    }
}

async fn list(env: &str, paths: &Paths) -> Result<i32> {
    let config = EnvironmentConfig::load(&paths.env_file(env))?;
    let provider = provider_for(config.backup.provider);
    let backups = provider.discover(&config).await?;

    if backups.is_empty() {
        println!(
            "Nenhum backup encontrado em {} ({})",
            config.backup.directory.display(),
            config.backup.glob
        );
        return Ok(0);
    }

    println!(
        "{:<38} {:>10} {:<26} {:<8} SHA-256",
        "BACKUP", "TAMANHO", "MODIFICADO", "LIST"
    );
    for backup in &backups {
        let validation = match backup.host_restore_list_ok {
            Some(true) => "ok",
            Some(false) => "falhou",
            None => "n/d",
        };
        println!(
            "{:<38} {:>10} {:<26} {:<8} {}",
            backup.id,
            format_bytes(backup.size_bytes),
            backup.modified_at.to_rfc3339(),
            validation,
            backup.sha256
        );
    }
    println!(
        "\n{} backup(s) em {}",
        backups.len(),
        config.backup.directory.display()
    );
    Ok(0)
}
