use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::expectation::Expectation;
use crate::paths::{expand_tilde, validate_env_name};
use crate::{Result, SentinelaError};

fn default_glob() -> String {
    "*.dump".to_string()
}
fn default_provider() -> ProviderKind {
    ProviderKind::Logical
}
fn default_postgres_version() -> String {
    "16".to_string()
}
fn default_postgres_image() -> String {
    "postgres".to_string()
}
fn default_postgres_user() -> String {
    "postgres".to_string()
}
fn default_database() -> String {
    "sentinela".to_string()
}
fn default_memory_mb() -> u64 {
    512
}
fn default_cpus() -> f64 {
    1.0
}
fn default_readiness_timeout_secs() -> u64 {
    60
}
fn default_restore_timeout_secs() -> u64 {
    600
}
fn default_tmpfs_mb() -> u64 {
    1024
}
fn default_tests_dir() -> PathBuf {
    PathBuf::from("tests")
}
fn default_statement_timeout_ms() -> u64 {
    5_000
}

/// Provedor de descoberta de backups.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Logical,
    PgBackRest,
}

impl ProviderKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Logical => "logical",
            Self::PgBackRest => "pgbackrest",
        }
    }
}

/// Configuração de descoberta do provider lógico (`pg_dump -Fc`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupConfig {
    #[serde(default = "default_provider")]
    pub provider: ProviderKind,
    #[serde(default)]
    pub directory: PathBuf,
    #[serde(default = "default_glob")]
    pub glob: String,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            directory: PathBuf::new(),
            glob: default_glob(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresConfig {
    #[serde(default = "default_postgres_version")]
    pub version: String,
    #[serde(default = "default_postgres_image")]
    pub image: String,
    #[serde(default = "default_postgres_user")]
    pub user: String,
    #[serde(default = "default_database")]
    pub database: String,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            version: default_postgres_version(),
            image: default_postgres_image(),
            user: default_postgres_user(),
            database: default_database(),
        }
    }
}

impl PostgresConfig {
    /// Imagem completa. Se o último segmento de `image` já contiver uma tag,
    /// ela é respeitada (inclusive em registries com porta, como
    /// `localhost:5000/postgres`).
    #[must_use]
    pub fn docker_image(&self) -> String {
        let has_tag = self
            .image
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.contains(':'));
        if has_tag {
            self.image.clone()
        } else {
            format!("{}:{}", self.image, self.version)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerConfig {
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u64,
    #[serde(default = "default_cpus")]
    pub cpus: f64,
    #[serde(default = "default_readiness_timeout_secs")]
    pub readiness_timeout_secs: u64,
    #[serde(default = "default_restore_timeout_secs")]
    pub restore_timeout_secs: u64,
    #[serde(default = "default_tmpfs_mb")]
    pub data_tmpfs_mb: u64,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            memory_mb: default_memory_mb(),
            cpus: default_cpus(),
            readiness_timeout_secs: default_readiness_timeout_secs(),
            restore_timeout_secs: default_restore_timeout_secs(),
            data_tmpfs_mb: default_tmpfs_mb(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestsConfig {
    #[serde(default = "default_tests_dir")]
    pub directory: PathBuf,
    #[serde(default = "default_statement_timeout_ms")]
    pub statement_timeout_ms: u64,
}

impl Default for TestsConfig {
    fn default() -> Self {
        Self {
            directory: default_tests_dir(),
            statement_timeout_ms: default_statement_timeout_ms(),
        }
    }
}

/// Configuração completa de um ambiente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentConfig {
    pub name: String,
    #[serde(default)]
    pub backup: BackupConfig,
    #[serde(default)]
    pub postgres: PostgresConfig,
    #[serde(default)]
    pub docker: DockerConfig,
    #[serde(default)]
    pub tests: TestsConfig,
}

impl EnvironmentConfig {
    /// Carrega e valida um YAML. Caminhos relativos são resolvidos a partir do
    /// diretório do arquivo de ambiente.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path).map_err(|error| {
            SentinelaError::Config(format!(
                "não foi possível ler o ambiente {}: {error}",
                path.display()
            ))
        })?;
        let mut config: Self = serde_yaml_ng::from_str(&raw).map_err(|error| {
            SentinelaError::Config(format!("YAML inválido em {}: {error}", path.display()))
        })?;

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        config.backup.directory = resolve_path(base, &config.backup.directory);
        config.tests.directory = resolve_path(base, &config.tests.directory);
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let yaml = serde_yaml_ng::to_string(self)?;
        crate::paths::write_private_file(path, yaml.as_bytes())
    }

    /// Valida campos que não podem ser verificados apenas pelo serde.
    pub fn validate(&self) -> Result<()> {
        validate_env_name(&self.name)?;

        if self.backup.directory.as_os_str().is_empty() {
            return Err(SentinelaError::Config(
                "backup.directory é obrigatório".into(),
            ));
        }
        if self.backup.glob.trim().is_empty() {
            return Err(SentinelaError::Config(
                "backup.glob não pode ser vazio".into(),
            ));
        }
        let glob_path = Path::new(&self.backup.glob);
        if glob_path.is_absolute()
            || self.backup.glob.contains("..")
            || self.backup.glob.contains('/')
        {
            return Err(SentinelaError::Config(
                "backup.glob deve ser um padrão simples, relativo ao diretório de backups".into(),
            ));
        }

        if self.postgres.version.trim().is_empty() {
            return Err(SentinelaError::Config(
                "postgres.version é obrigatório".into(),
            ));
        }
        if self.postgres.image.trim().is_empty() {
            return Err(SentinelaError::Config(
                "postgres.image é obrigatório".into(),
            ));
        }
        if self.postgres.user.trim().is_empty() {
            return Err(SentinelaError::Config("postgres.user é obrigatório".into()));
        }
        if self.postgres.database.trim().is_empty() {
            return Err(SentinelaError::Config(
                "postgres.database é obrigatório".into(),
            ));
        }

        if self.docker.memory_mb == 0 {
            return Err(SentinelaError::Config(
                "docker.memory_mb deve ser maior que zero".into(),
            ));
        }
        if !self.docker.cpus.is_finite() || self.docker.cpus <= 0.0 {
            return Err(SentinelaError::Config(
                "docker.cpus deve ser um número positivo".into(),
            ));
        }
        if self.docker.readiness_timeout_secs == 0 {
            return Err(SentinelaError::Config(
                "docker.readiness_timeout_secs deve ser maior que zero".into(),
            ));
        }
        if self.docker.restore_timeout_secs == 0 {
            return Err(SentinelaError::Config(
                "docker.restore_timeout_secs deve ser maior que zero".into(),
            ));
        }
        if self.docker.data_tmpfs_mb == 0 {
            return Err(SentinelaError::Config(
                "docker.data_tmpfs_mb deve ser maior que zero".into(),
            ));
        }

        if self.tests.directory.as_os_str().is_empty() {
            return Err(SentinelaError::Config(
                "tests.directory não pode ser vazio".into(),
            ));
        }
        if self.tests.statement_timeout_ms == 0 {
            return Err(SentinelaError::Config(
                "tests.statement_timeout_ms deve ser maior que zero".into(),
            ));
        }

        Ok(())
    }
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    let expanded = expand_tilde(&path.to_string_lossy());
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

/// Defaults globais usados por `env add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDefaults {
    #[serde(default = "default_postgres_version")]
    pub postgres_version: String,
    #[serde(default = "default_postgres_image")]
    pub postgres_image: String,
    #[serde(default = "default_postgres_user")]
    pub postgres_user: String,
    #[serde(default = "default_database")]
    pub database: String,
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u64,
    #[serde(default = "default_cpus")]
    pub cpus: f64,
    #[serde(default = "default_readiness_timeout_secs")]
    pub readiness_timeout_secs: u64,
    #[serde(default = "default_restore_timeout_secs")]
    pub restore_timeout_secs: u64,
}

impl Default for EnvironmentDefaults {
    fn default() -> Self {
        Self {
            postgres_version: default_postgres_version(),
            postgres_image: default_postgres_image(),
            postgres_user: default_postgres_user(),
            database: default_database(),
            memory_mb: default_memory_mb(),
            cpus: default_cpus(),
            readiness_timeout_secs: default_readiness_timeout_secs(),
            restore_timeout_secs: default_restore_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    #[serde(default)]
    pub defaults: EnvironmentDefaults,
}

impl GlobalConfig {
    /// Lê o arquivo global; ausência do arquivo significa defaults.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path).map_err(|error| {
            SentinelaError::Config(format!("não foi possível ler {}: {error}", path.display()))
        })?;
        serde_yaml_ng::from_str(&raw).map_err(|error| {
            SentinelaError::Config(format!("YAML inválido em {}: {error}", path.display()))
        })
    }
}

/// Definição de um teste SQL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestDefinition {
    pub name: String,
    pub sql: String,
    pub expect: Expectation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl TestDefinition {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(SentinelaError::Config("teste sem 'name'".into()));
        }
        if self.sql.trim().is_empty() {
            return Err(SentinelaError::Config(format!(
                "teste '{}' sem SQL",
                self.name
            )));
        }
        if self.timeout_ms == Some(0) {
            return Err(SentinelaError::Config(format!(
                "teste '{}' tem timeout_ms igual a zero",
                self.name
            )));
        }
        self.expect
            .validate()
            .map_err(|error| SentinelaError::Config(format!("teste '{}': {error}", self.name)))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TestsDocument {
    Wrapped { tests: Vec<TestDefinition> },
    List(Vec<TestDefinition>),
}

impl TestsDocument {
    fn into_tests(self) -> Vec<TestDefinition> {
        match self {
            Self::Wrapped { tests } => tests,
            Self::List(tests) => tests,
        }
    }
}

/// Carrega todos os arquivos `*.yaml`/`*.yml` de um diretório de testes.
///
/// Diretório ausente é tratado como "nenhum teste", permitindo um run que
/// apenas prova que o restore concluiu.
pub fn load_tests_from_dir(directory: &Path) -> Result<Vec<TestDefinition>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    if !directory.is_dir() {
        return Err(SentinelaError::Config(format!(
            "tests.directory não é um diretório: {}",
            directory.display()
        )));
    }

    let mut entries = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
                })
        })
        .collect::<Vec<_>>();
    entries.sort();

    let mut tests = Vec::new();
    let mut names = std::collections::HashSet::new();
    for path in entries {
        let raw = fs::read_to_string(&path)?;
        let document: TestsDocument = serde_yaml_ng::from_str(&raw).map_err(|error| {
            SentinelaError::Config(format!(
                "YAML de testes inválido em {}: {error}",
                path.display()
            ))
        })?;
        for test in document.into_tests() {
            test.validate()
                .map_err(|error| SentinelaError::Config(format!("{}: {error}", path.display())))?;
            if !names.insert(test.name.clone()) {
                return Err(SentinelaError::Config(format!(
                    "teste duplicado: '{}'",
                    test.name
                )));
            }
            tests.push(test);
        }
    }
    Ok(tests)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_yaml() -> &'static str {
        r#"
name: producao
backup:
  provider: logical
  directory: /var/backups/postgres
  glob: "*.dump"
postgres:
  version: "16"
  database: app
tests:
  directory: tests
  statement_timeout_ms: 2500
"#
    }

    #[test]
    fn parses_environment_yaml() {
        let config: EnvironmentConfig = serde_yaml_ng::from_str(valid_yaml()).unwrap();
        config.validate().unwrap();
        assert_eq!(config.name, "producao");
        assert_eq!(config.postgres.docker_image(), "postgres:16");
        assert_eq!(config.tests.statement_timeout_ms, 2500);
        assert_eq!(config.docker.memory_mb, 512);
    }

    #[test]
    fn builds_docker_image_with_version() {
        let mut config = PostgresConfig::default();
        assert_eq!(config.docker_image(), "postgres:16");

        config.image = "postgres:15".into();
        assert_eq!(config.docker_image(), "postgres:15");

        config.image = "localhost:5000/postgres".into();
        assert_eq!(config.docker_image(), "localhost:5000/postgres:16");
    }

    #[test]
    fn rejects_invalid_glob() {
        let mut config: EnvironmentConfig = serde_yaml_ng::from_str(valid_yaml()).unwrap();
        config.backup.glob = "../*.dump".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = format!("{}extra: true\n", valid_yaml());
        assert!(serde_yaml_ng::from_str::<EnvironmentConfig>(&yaml).is_err());
    }

    #[test]
    fn parses_tests_documents() {
        let wrapped: TestsDocument = serde_yaml_ng::from_str(
            r#"
tests:
  - name: users_exist
    sql: SELECT count(*) FROM users
    expect: { op: gt, value: 0 }
"#,
        )
        .unwrap();
        assert_eq!(wrapped.into_tests().len(), 1);

        let list: TestsDocument = serde_yaml_ng::from_str(
            r#"
- name: ok
  sql: SELECT true
  expect: { op: is_true }
"#,
        )
        .unwrap();
        assert_eq!(list.into_tests().len(), 1);
    }
}
