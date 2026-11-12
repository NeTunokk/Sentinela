use async_trait::async_trait;
use sentinela_core::config::EnvironmentConfig;
use sentinela_core::model::BackupInfo;
use sentinela_core::{Result, SentinelaError};

use crate::provider::BackupProvider;

/// Stub deliberado do provider PgBackRest.
///
/// O Sentinela 0.1 implementa apenas dumps lógicos (`pg_dump -Fc`). O suporte
/// a repositórios PgBackRest exige integração com metadados/manifestos e
/// restauração própria, previsto para uma versão futura.
///
/// Configurar `backup.provider: pgbackrest` hoje falha na descoberta, com erro
/// de configuração, sem subir container e sem tocar em nenhum backup.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgBackRestProvider;

#[async_trait]
impl BackupProvider for PgBackRestProvider {
    fn name(&self) -> &'static str {
        "pgbackrest"
    }

    async fn discover(&self, _config: &EnvironmentConfig) -> Result<Vec<BackupInfo>> {
        // TODO(0.2): descobrir backups via `pgbackrest info --output=json`,
        // validar o manifesto/stanza e restaurar em sandbox isolado.
        Err(SentinelaError::Config(
            "provider 'pgbackrest' não é suportado no Sentinela 0.1; use 'logical' (pg_dump -Fc)"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use sentinela_core::config::{BackupConfig, DockerConfig, PostgresConfig, TestsConfig};

    use super::*;

    fn config() -> EnvironmentConfig {
        EnvironmentConfig {
            name: "teste".into(),
            backup: BackupConfig::default(),
            postgres: PostgresConfig::default(),
            docker: DockerConfig::default(),
            tests: TestsConfig::default(),
        }
    }

    #[tokio::test]
    async fn rejects_pgbackrest_with_config_error_instead_of_panicking() {
        let error = PgBackRestProvider
            .discover(&config())
            .await
            .expect_err("pgbackrest deve falhar com erro de configuração");

        assert!(matches!(error, SentinelaError::Config(_)));
        assert!(error.to_string().contains("pgbackrest"));
    }
}
