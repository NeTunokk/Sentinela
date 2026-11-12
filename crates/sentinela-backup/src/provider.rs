use async_trait::async_trait;

use sentinela_core::config::{EnvironmentConfig, ProviderKind};
use sentinela_core::model::BackupInfo;
use sentinela_core::{Result, SentinelaError};

use crate::logical::LogicalDumpProvider;
use crate::pgbackrest::PgBackRestProvider;

/// Contrato de descoberta de backups.
#[async_trait]
pub trait BackupProvider: Send + Sync {
    /// Identificador estável do provider.
    fn name(&self) -> &'static str;

    /// Lista backups com metadados e SHA-256 calculado em streaming.
    async fn discover(&self, config: &EnvironmentConfig) -> Result<Vec<BackupInfo>>;
}

/// Escolhe a implementação a partir da configuração do ambiente.
#[must_use]
pub fn provider_for(kind: ProviderKind) -> Box<dyn BackupProvider> {
    match kind {
        ProviderKind::Logical => Box::new(LogicalDumpProvider),
        ProviderKind::PgBackRest => Box::new(PgBackRestProvider),
    }
}

/// Seleciona o backup mais recente ou o informado por id/prefixo único.
pub fn select_backup(backups: &[BackupInfo], filter: Option<&str>) -> Result<BackupInfo> {
    if backups.is_empty() {
        return Err(SentinelaError::Backup(
            "nenhum backup encontrado no diretório configurado".into(),
        ));
    }

    match filter {
        None => backups
            .iter()
            .max_by(|left, right| {
                left.modified_at
                    .cmp(&right.modified_at)
                    .then_with(|| left.id.cmp(&right.id))
            })
            .cloned()
            .ok_or_else(|| SentinelaError::Backup("lista de backups vazia".into())),
        Some(filter) => {
            if let Some(exact) = backups.iter().find(|backup| backup.id == filter) {
                return Ok(exact.clone());
            }

            let matches = backups
                .iter()
                .filter(|backup| backup.id.starts_with(filter))
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => Err(SentinelaError::Config(format!(
                    "backup '{}' não encontrado",
                    filter
                ))),
                [only] => Ok((*only).clone()),
                _ => Err(SentinelaError::Config(format!(
                    "backup '{}' é ambíguo ({} correspondências)",
                    filter,
                    matches.len()
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sentinela_core::model::BackupInfo;

    use super::*;

    fn backup(id: &str, day: u32) -> BackupInfo {
        BackupInfo {
            id: id.into(),
            path: format!("/tmp/{id}").into(),
            provider: "logical".into(),
            size_bytes: 1,
            modified_at: chrono::DateTime::parse_from_rfc3339(&format!(
                "2026-01-{day:02}T00:00:00Z"
            ))
            .unwrap()
            .with_timezone(&Utc),
            sha256: "00".into(),
            host_restore_list_ok: None,
        }
    }

    #[test]
    fn selects_most_recent_by_default() {
        let backups = vec![backup("old.dump", 1), backup("new.dump", 2)];
        assert_eq!(select_backup(&backups, None).unwrap().id, "new.dump");
    }

    #[test]
    fn selects_by_unique_prefix() {
        let backups = vec![backup("prod-01.dump", 1), backup("prod-02.dump", 2)];
        assert_eq!(
            select_backup(&backups, Some("prod-02")).unwrap().id,
            "prod-02.dump"
        );
        assert!(select_backup(&backups, Some("prod-0")).is_err());
        assert!(select_backup(&backups, Some("nope")).is_err());
    }
}
