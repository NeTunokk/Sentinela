//! Descoberta e validação de backups PostgreSQL.
//!
//! O único provider implementado no MVP é o dump lógico (`pg_dump -Fc`).
//! Nenhum byte do backup sai da máquina local.

pub mod logical;
pub mod pgbackrest;
pub mod provider;
mod validate;

pub use logical::LogicalDumpProvider;
pub use pgbackrest::PgBackRestProvider;
pub use provider::{provider_for, select_backup, BackupProvider};
