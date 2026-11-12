mod cli;
mod commands;
mod report;
mod signals;

use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::Parser;
use sentinela_core::SentinelaError;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let kind = error.kind();
            let code = match kind {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 3,
            };
            if let Err(print_error) = error.print() {
                eprintln!("erro ao imprimir ajuda: {print_error}");
            }
            return exit_code(code);
        }
    };

    init_tracing();

    match commands::dispatch(cli.command).await {
        Ok(code) => exit_code(code),
        Err(error) => {
            eprintln!("erro: {error:#}");
            let domain_code = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<SentinelaError>())
                .map_or(3, SentinelaError::exit_code);
            exit_code(domain_code)
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,sentinela_cli=info,sentinela_runner=info,sentinela_backup=info")
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(3))
}
