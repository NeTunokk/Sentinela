//! Núcleo reutilizável da CLI: comandos, relatórios e assinatura.
//!
//! O binário `sentinela` e o servidor local do painel usam esta biblioteca para
//! compartilhar as mesmas regras de configuração, execução e verificação.

pub mod cli;
pub mod commands;
pub mod report;
pub mod signals;
