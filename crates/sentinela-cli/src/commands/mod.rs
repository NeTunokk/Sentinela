pub mod backups;
pub mod cleanup;
pub mod env;
pub mod history;
pub mod report;
pub mod run;

use anyhow::Result;
use sentinela_core::paths::Paths;

use crate::cli::Command;

pub async fn dispatch(command: Command) -> Result<i32> {
    let paths = Paths::discover()?;
    paths.ensure()?;

    match command {
        Command::Env { command } => env::execute(*command, &paths),
        Command::Backups { command } => backups::execute(command, &paths).await,
        Command::Run(args) => run::execute(args, &paths).await,
        Command::History(args) => history::execute(args, &paths),
        Command::Report { command } => report::execute(command, &paths),
        Command::Cleanup => cleanup::execute().await,
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
