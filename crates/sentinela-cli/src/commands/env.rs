use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sentinela_core::config::{
    BackupConfig, DockerConfig, EnvironmentConfig, GlobalConfig, PostgresConfig, ProviderKind,
    TestsConfig,
};
use sentinela_core::paths::{expand_tilde, validate_env_name, Paths};
use sentinela_core::SentinelaError;
use sentinela_store::Store;

use crate::cli::{EnvAddArgs, EnvCommand, ProviderArg};

pub fn execute(command: EnvCommand, paths: &Paths) -> Result<i32> {
    match command {
        EnvCommand::Add(args) => add(*args, paths),
        EnvCommand::List => list(paths),
        EnvCommand::Remove { name } => remove(&name, paths),
    }
}

fn add(args: EnvAddArgs, paths: &Paths) -> Result<i32> {
    validate_env_name(&args.name)?;
    let path = paths.env_file(&args.name);
    if path.exists() && !args.force {
        return Err(SentinelaError::Config(format!(
            "ambiente '{}' já existe em {} (use --force para sobrescrever)",
            args.name,
            path.display()
        ))
        .into());
    }

    let global = GlobalConfig::load(&paths.global_config_file())?;
    let defaults = global.defaults;
    let cwd = env::current_dir().map_err(|error| {
        SentinelaError::Config(format!("não foi possível obter o diretório atual: {error}"))
    })?;
    let backup_dir = resolve_path(&args.backup_dir, &cwd);
    let tests_dir = args
        .tests_dir
        .as_deref()
        .map_or_else(|| cwd.join("tests"), |value| resolve_path(value, &cwd));

    let provider = match args.provider {
        ProviderArg::Logical => ProviderKind::Logical,
        ProviderArg::PgBackRest => ProviderKind::PgBackRest,
    };

    let config = EnvironmentConfig {
        name: args.name.clone(),
        backup: BackupConfig {
            provider,
            directory: backup_dir,
            glob: args.glob,
        },
        postgres: PostgresConfig {
            version: args.postgres_version.unwrap_or(defaults.postgres_version),
            image: args.postgres_image.unwrap_or(defaults.postgres_image),
            user: args.postgres_user.unwrap_or(defaults.postgres_user),
            database: args.database.unwrap_or(defaults.database),
        },
        docker: DockerConfig {
            memory_mb: args.memory_mb.unwrap_or(defaults.memory_mb),
            cpus: args.cpus.unwrap_or(defaults.cpus),
            readiness_timeout_secs: args
                .readiness_timeout_secs
                .unwrap_or(defaults.readiness_timeout_secs),
            restore_timeout_secs: args
                .restore_timeout_secs
                .unwrap_or(defaults.restore_timeout_secs),
            data_tmpfs_mb: 1024,
        },
        tests: TestsConfig {
            directory: tests_dir,
            statement_timeout_ms: args.statement_timeout_ms.unwrap_or(5_000),
        },
    };

    config.save(&path)?;

    let store = Store::open(&paths.database_path())?;
    let config_json = serde_json::to_string(&config).map_err(|error| {
        SentinelaError::Store(format!("não foi possível serializar o ambiente: {error}"))
    })?;
    store.upsert_environment(&config.name, &config_json)?;

    println!("Ambiente '{}' salvo em {}", config.name, path.display());
    println!(
        "  backups: {} ({})",
        config.backup.directory.display(),
        config.backup.glob
    );
    println!("  imagem:  {}", config.postgres.docker_image());
    println!("  testes:  {}", config.tests.directory.display());
    Ok(0)
}

fn list(paths: &Paths) -> Result<i32> {
    let mut files = fs::read_dir(paths.envs_dir())
        .map_err(|error| {
            SentinelaError::Config(format!(
                "não foi possível listar {}: {error}",
                paths.envs_dir().display()
            ))
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
                })
        })
        .collect::<Vec<_>>();
    files.sort();

    if files.is_empty() {
        println!("Nenhum ambiente configurado. Use: sentinela env add <nome> --backup-dir <dir>");
        return Ok(0);
    }

    println!(
        "{:<20} {:<12} {:<42} {:<18} TESTES",
        "AMBIENTE", "PROVIDER", "BACKUPS", "IMAGEM"
    );
    for file in files {
        match EnvironmentConfig::load(&file) {
            Ok(config) => println!(
                "{:<20} {:<12} {:<42} {:<18} {}",
                config.name,
                config.backup.provider.as_str(),
                config.backup.directory.display().to_string(),
                config.postgres.docker_image(),
                config.tests.directory.display()
            ),
            Err(error) => println!(
                "{:<20} {:<12} {:<42} {:<18} -",
                file.file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("?"),
                "ERRO",
                error,
                "-"
            ),
        }
    }
    Ok(0)
}

fn remove(name: &str, paths: &Paths) -> Result<i32> {
    validate_env_name(name)?;
    let path = paths.env_file(name);
    if !path.exists() {
        return Err(SentinelaError::Config(format!(
            "ambiente '{}' não encontrado em {}",
            name,
            path.display()
        ))
        .into());
    }
    fs::remove_file(&path)?;
    let store = Store::open(&paths.database_path())?;
    store.remove_environment(name)?;
    println!(
        "Ambiente '{}' removido. O histórico de execuções foi preservado.",
        name
    );
    Ok(0)
}

fn resolve_path(path: &Path, cwd: &Path) -> PathBuf {
    let expanded = expand_tilde(&path.to_string_lossy());
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}
