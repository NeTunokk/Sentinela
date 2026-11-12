use thiserror::Error;

/// Erros de domínio do Sentinela.
///
/// O código de saída é derivado exclusivamente do tipo do erro:
/// `1` para testes que falharam, `2` para restauração que falhou e `3` para
/// qualquer falha de infraestrutura/configuração.
#[derive(Debug, Error)]
pub enum SentinelaError {
    #[error("erro de configuração: {0}")]
    Config(String),

    #[error("erro de backup: {0}")]
    Backup(String),

    #[error("backup inválido: {0}")]
    BackupInvalid(String),

    #[error("erro de E/S: {0}")]
    Io(#[from] std::io::Error),

    #[error("erro de infraestrutura Docker: {0}")]
    Docker(String),

    #[error("falha na restauração: {0}")]
    Restore(String),

    #[error("testes SQL falharam: {0}")]
    TestsFailed(String),

    #[error("erro de persistência: {0}")]
    Store(String),

    #[error("erro ao gerar/validar relatório: {0}")]
    Report(String),

    #[error("operação interrompida por sinal")]
    Interrupted,
}

impl SentinelaError {
    /// Código de saída documentado para a CLI.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::TestsFailed(_) => 1,
            Self::Restore(_) | Self::BackupInvalid(_) => 2,
            Self::Config(_)
            | Self::Backup(_)
            | Self::Io(_)
            | Self::Docker(_)
            | Self::Store(_)
            | Self::Report(_)
            | Self::Interrupted => 3,
        }
    }
}

impl From<serde_yaml_ng::Error> for SentinelaError {
    fn from(value: serde_yaml_ng::Error) -> Self {
        Self::Config(value.to_string())
    }
}

impl From<serde_json::Error> for SentinelaError {
    fn from(value: serde_json::Error) -> Self {
        Self::Report(value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SentinelaError>;
