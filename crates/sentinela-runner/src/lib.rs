//! Sandbox Docker efêmero, restauração e testes SQL.
//!
//! O container usa uma rede `internal` (sem rota para a internet), dados em
//! tmpfs, limites de memória/CPU e uma porta publicada apenas em `127.0.0.1`
//! para que os testes SQL possam ser executados pelo host com `tokio-postgres`.

pub mod docker;
pub mod sql_tests;

pub use docker::{
    cleanup_orphans, random_password, CleanupReport, DockerSession, RunExecution, SandboxSpec,
};
pub use sql_tests::run_sql_tests;
