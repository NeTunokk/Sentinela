use anyhow::Result;
use sentinela_core::paths::Paths;
use sentinela_store::Store;

use crate::cli::HistoryArgs;

pub fn execute(args: HistoryArgs, paths: &Paths) -> Result<i32> {
    let store = Store::open(&paths.database_path())?;
    let status = args.status.map(|status| status.as_filter());
    let runs = store.list_runs(args.env.as_deref(), status)?;

    if runs.is_empty() {
        println!("Nenhuma execução encontrada.");
        return Ok(0);
    }

    println!(
        "{:<38} {:<18} {:<15} {:>6} {:<26} {:>10}",
        "RUN", "AMBIENTE", "STATUS", "EXIT", "INÍCIO", "TOTAL"
    );
    for run in runs {
        let total = run
            .total_duration_ms
            .map_or_else(|| "-".to_string(), |value| format!("{value} ms"));
        println!(
            "{:<38} {:<18} {:<15} {:>6} {:<26} {:>10}",
            run.id, run.environment, run.status, run.exit_code, run.started_at, total
        );
    }
    Ok(0)
}
