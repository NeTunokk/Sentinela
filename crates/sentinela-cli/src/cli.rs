use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "sentinela",
    version,
    about = "Prova localmente que um backup PostgreSQL é restaurável",
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Gerencia ambientes.
    Env {
        #[command(subcommand)]
        command: Box<EnvCommand>,
    },
    /// Lista backups descobertos.
    Backups {
        #[command(subcommand)]
        command: BackupsCommand,
    },
    /// Executa restauração e testes em sandbox descartável.
    Run(RunArgs),
    /// Mostra o histórico local de execuções.
    History(HistoryArgs),
    /// Mostra e verifica um relatório assinado.
    Report {
        #[command(subcommand)]
        command: ReportCommand,
    },
    /// Remove containers, redes e volumes órfãos do Sentinela.
    Cleanup,
}

#[derive(Debug, Subcommand)]
pub enum EnvCommand {
    /// Adiciona ou atualiza um ambiente.
    Add(Box<EnvAddArgs>),
    /// Lista ambientes configurados.
    List,
    /// Remove a configuração de um ambiente (o histórico é preservado).
    Remove { name: String },
}

#[derive(Debug, Args)]
pub struct EnvAddArgs {
    pub name: String,

    #[arg(long)]
    pub backup_dir: PathBuf,

    #[arg(long, value_enum, default_value_t = ProviderArg::Logical)]
    pub provider: ProviderArg,

    #[arg(long, default_value = "*.dump")]
    pub glob: String,

    #[arg(long)]
    pub tests_dir: Option<PathBuf>,

    #[arg(long)]
    pub postgres_version: Option<String>,

    #[arg(long)]
    pub postgres_image: Option<String>,

    #[arg(long)]
    pub postgres_user: Option<String>,

    #[arg(long)]
    pub database: Option<String>,

    #[arg(long)]
    pub memory_mb: Option<u64>,

    #[arg(long)]
    pub cpus: Option<f64>,

    #[arg(long)]
    pub readiness_timeout_secs: Option<u64>,

    #[arg(long)]
    pub restore_timeout_secs: Option<u64>,

    #[arg(long)]
    pub statement_timeout_ms: Option<u64>,

    /// Sobrescreve um ambiente existente.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProviderArg {
    Logical,
    PgBackRest,
}

#[derive(Debug, Subcommand)]
pub enum BackupsCommand {
    /// Lista backups do ambiente com SHA-256 e validação pg_restore.
    List {
        #[arg(long)]
        env: String,
    },
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub env: String,

    /// Nome ou prefixo único do backup.
    #[arg(long)]
    pub backup: Option<String>,

    /// Mantém container e rede para depuração.
    #[arg(long)]
    pub keep: bool,
}

#[derive(Debug, Args)]
pub struct HistoryArgs {
    #[arg(long)]
    pub env: Option<String>,

    #[arg(long, value_enum)]
    pub status: Option<HistoryStatus>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum HistoryStatus {
    Ok,
    Failed,
    TestsFailed,
    RestoreFailed,
    InfraError,
}

impl HistoryStatus {
    #[must_use]
    pub fn as_filter(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::TestsFailed => "tests_failed",
            Self::RestoreFailed => "restore_failed",
            Self::InfraError => "infra_error",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ReportCommand {
    /// Mostra o resumo e verifica a assinatura de um relatório.
    Show {
        run_id: String,
        /// Imprime o JSON bruto após verificar a assinatura.
        #[arg(long)]
        json: bool,
    },
}
