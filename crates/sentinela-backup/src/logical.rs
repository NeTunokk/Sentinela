use std::fs;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sentinela_core::config::EnvironmentConfig;
use sentinela_core::hash::sha256_file;
use sentinela_core::model::BackupInfo;
use sentinela_core::{Result, SentinelaError};
use tracing::{debug, warn};

use crate::provider::BackupProvider;
use crate::validate::host_restore_list;

/// Provider de dumps lógicos produzidos por `pg_dump -Fc`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogicalDumpProvider;

#[async_trait]
impl BackupProvider for LogicalDumpProvider {
    fn name(&self) -> &'static str {
        "logical"
    }

    async fn discover(&self, config: &EnvironmentConfig) -> Result<Vec<BackupInfo>> {
        let directory = &config.backup.directory;
        if !directory.exists() {
            return Err(SentinelaError::Backup(format!(
                "diretório de backups não existe: {}",
                directory.display()
            )));
        }
        if !directory.is_dir() {
            return Err(SentinelaError::Backup(format!(
                "caminho de backups não é um diretório: {}",
                directory.display()
            )));
        }

        let pattern = directory.join(&config.backup.glob);
        let pattern = pattern.to_string_lossy().into_owned();
        let entries = glob::glob(&pattern).map_err(|error| {
            SentinelaError::Backup(format!("padrão de backup inválido '{pattern}': {error}"))
        })?;

        let mut paths = entries
            .filter_map(|entry| match entry {
                Ok(path) => Some(path),
                Err(error) => {
                    warn!(error = %error, "entrada de backup ignorada");
                    None
                }
            })
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        paths.sort();

        let mut backups = Vec::with_capacity(paths.len());
        for path in paths {
            let host_restore_list_ok = host_restore_list(&path).await;
            let inspect_path = path.clone();
            let inspected = tokio::task::spawn_blocking(move || inspect(&inspect_path))
                .await
                .map_err(|error| {
                    SentinelaError::Backup(format!("falha ao inspecionar backup: {error}"))
                })??;
            debug!(path = %path.display(), sha256 = %inspected.sha256, "backup descoberto");
            backups.push(BackupInfo {
                id: inspected.id,
                path,
                provider: self.name().to_string(),
                size_bytes: inspected.size_bytes,
                modified_at: inspected.modified_at,
                sha256: inspected.sha256,
                host_restore_list_ok,
            });
        }

        backups.sort_by(|left, right| {
            right
                .modified_at
                .cmp(&left.modified_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(backups)
    }
}

struct Inspected {
    id: String,
    size_bytes: u64,
    modified_at: DateTime<Utc>,
    sha256: String,
}

fn inspect(path: &Path) -> Result<Inspected> {
    let metadata = fs::metadata(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SentinelaError::Backup(format!("nome inválido: {}", path.display())))?;
    let modified = metadata.modified().map_err(|error| {
        SentinelaError::Backup(format!(
            "não foi possível ler mtime de {}: {error}",
            path.display()
        ))
    })?;
    let sha256 = sha256_file(path).map_err(|error| {
        SentinelaError::Backup(format!(
            "falha ao calcular SHA-256 de {}: {error}",
            path.display()
        ))
    })?;
    Ok(Inspected {
        id: file_name.to_string(),
        size_bytes: metadata.len(),
        modified_at: DateTime::<Utc>::from(modified),
        sha256,
    })
}
