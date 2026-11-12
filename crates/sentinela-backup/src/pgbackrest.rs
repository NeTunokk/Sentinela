use async_trait::async_trait;
use sentinela_core::config::EnvironmentConfig;
use sentinela_core::model::BackupInfo;
use sentinela_core::Result;

use crate::provider::BackupProvider;

/// Stub deliberado do provider PgBackRest.
///
/// O Sentinela 0.1 implementa apenas dumps lógicos (`pg_dump -Fc`). O suporte
/// a repositórios PgBackRest exige integração com metadados/manifestos e
/// restauração própria, previsto para uma versão futura.
///
/// Configurar `backup.provider: pgbackrest` hoje falha imediatamente no
/// momento da descoberta, sem subir container e sem tocar em nenhum backup.
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
        todo!("provider PgBackRest ainda não implementado no Sentinela 0.1")
    }
}
