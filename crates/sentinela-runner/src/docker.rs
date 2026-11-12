use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine as _;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, HostConfig, NetworkCreateRequest, PortBinding,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, ListContainersOptionsBuilder, ListNetworksOptionsBuilder,
    ListVolumesOptionsBuilder, RemoveContainerOptionsBuilder, RemoveVolumeOptionsBuilder,
    StartContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use rand::Rng;
use sentinela_core::config::EnvironmentConfig;
use sentinela_core::model::{RestoreReport, RunStatus, TestOutcome};
use sentinela_core::{Result, SentinelaError};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

use crate::sql_tests::run_sql_tests;

const LABEL_SENTINELA: &str = "sentinela";
const LABEL_RUN_ID: &str = "sentinela.run-id";
const LABEL_ROLE: &str = "sentinela.role";
const CONTAINER_BACKUP_PATH: &str = "/backup/input.dump";
const CONTAINER_DATA_PATH: &str = "/var/lib/postgresql/data";
const CONTAINER_PORT: &str = "5432/tcp";
const REPORT_STDERR_LIMIT: usize = 16 * 1024;

/// Especificação imutável do sandbox de uma execução.
///
/// A senha é gerada aqui e nunca é impressa em logs, relatórios ou erros.
pub struct SandboxSpec {
    pub run_id: String,
    pub image: String,
    pub postgres_user: String,
    pub database: String,
    pub backup_path: PathBuf,
    pub memory_mb: u64,
    pub cpus: f64,
    pub data_tmpfs_mb: u64,
    pub readiness_timeout: Duration,
    pub restore_timeout: Duration,
    password: String,
}

impl SandboxSpec {
    #[must_use]
    pub fn from_environment(
        config: &EnvironmentConfig,
        run_id: String,
        backup_path: PathBuf,
    ) -> Self {
        Self {
            run_id,
            image: config.postgres.docker_image(),
            postgres_user: config.postgres.user.clone(),
            database: config.postgres.database.clone(),
            backup_path,
            memory_mb: config.docker.memory_mb,
            cpus: config.docker.cpus,
            data_tmpfs_mb: config.docker.data_tmpfs_mb,
            readiness_timeout: Duration::from_secs(config.docker.readiness_timeout_secs),
            restore_timeout: Duration::from_secs(config.docker.restore_timeout_secs),
            password: random_password(),
        }
    }

    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }
}

/// Senha aleatória (32 caracteres URL-safe) usada apenas dentro do sandbox.
#[must_use]
pub fn random_password() -> String {
    let mut bytes = [0_u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Resultado agregado de restauração + testes.
pub struct RunExecution {
    pub restore: RestoreReport,
    pub tests: Vec<TestOutcome>,
    pub status: RunStatus,
    pub error: Option<String>,
    pub tests_duration_ms: u64,
}

/// Recursos removidos pela limpeza.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub containers_removed: usize,
    pub networks_removed: usize,
    pub volumes_removed: usize,
    pub errors: Vec<String>,
}

struct ExecOutput {
    exit_code: Option<i32>,
    stderr: String,
}

#[derive(Clone, Copy)]
struct OutputLimits {
    stdout: usize,
    stderr: usize,
}

impl OutputLimits {
    const NONE: Self = Self {
        stdout: 0,
        stderr: 0,
    };
}

/// Sessão Docker de uma execução. `teardown` deve ser chamado em todos os
/// caminhos, inclusive erro e sinal.
pub struct DockerSession {
    docker: Docker,
    run_id: String,
    container_name: String,
    network_name: String,
    container_attempted: bool,
    network_attempted: bool,
}

impl DockerSession {
    pub fn connect(run_id: &str) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults().map_err(|error| {
            SentinelaError::Docker(format!(
                "não foi possível conectar ao Docker local: {error}"
            ))
        })?;
        let safe_run_id = run_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        Ok(Self {
            docker,
            run_id: run_id.to_string(),
            container_name: format!("sentinela-run-{safe_run_id}"),
            network_name: format!("sentinela-net-{safe_run_id}"),
            container_attempted: false,
            network_attempted: false,
        })
    }

    #[must_use]
    pub fn container_name(&self) -> &str {
        &self.container_name
    }

    #[must_use]
    pub fn network_name(&self) -> &str {
        &self.network_name
    }

    /// Sobe rede, container e aguarda readiness.
    pub async fn prepare(&mut self, spec: &SandboxSpec) -> Result<()> {
        self.ensure_image(&spec.image).await?;
        self.create_network().await?;
        self.create_container(spec).await?;
        self.start_container().await?;
        self.wait_until_ready(spec).await?;
        Ok(())
    }

    async fn ensure_image(&self, image: &str) -> Result<()> {
        self.docker
            .inspect_image(image)
            .await
            .map(|_| ())
            .map_err(|error| {
                SentinelaError::Docker(format!(
                    "imagem '{image}' não está disponível localmente; o Sentinela não faz pull \
                 para garantir zero acesso à rede. Baixe-a manualmente e tente novamente. \
                 Detalhe: {error}"
                ))
            })
    }

    async fn create_network(&mut self) -> Result<()> {
        self.network_attempted = true;
        let labels = HashMap::from([
            (LABEL_SENTINELA.to_string(), "true".to_string()),
            (LABEL_RUN_ID.to_string(), self.run_id.clone()),
            (LABEL_ROLE.to_string(), "network".to_string()),
        ]);
        let request = NetworkCreateRequest {
            name: self.network_name.clone(),
            driver: Some("bridge".to_string()),
            internal: Some(true),
            labels: Some(labels),
            ..NetworkCreateRequest::default()
        };
        self.docker
            .create_network(request)
            .await
            .map(|_| ())
            .map_err(|error| {
                SentinelaError::Docker(format!(
                    "falha ao criar rede interna '{}': {error}",
                    self.network_name
                ))
            })
    }

    async fn create_container(&mut self, spec: &SandboxSpec) -> Result<()> {
        let backup_path = spec.backup_path.to_string_lossy().into_owned();
        if backup_path.contains(':') {
            return Err(SentinelaError::Docker(
                "o caminho do backup contém ':', não suportado no bind mount do MVP".into(),
            ));
        }

        let mut labels = HashMap::from([
            (LABEL_SENTINELA.to_string(), "true".to_string()),
            (LABEL_RUN_ID.to_string(), self.run_id.clone()),
            (LABEL_ROLE.to_string(), "postgres".to_string()),
        ]);
        labels.insert(
            "sentinela.backup.id".to_string(),
            spec.backup_path.display().to_string(),
        );

        let memory_bytes = spec
            .memory_mb
            .saturating_mul(1024 * 1024)
            .min(i64::MAX as u64) as i64;
        let nano_cpus = (spec.cpus * 1_000_000_000.0)
            .round()
            .clamp(1.0, i64::MAX as f64) as i64;
        let tmpfs_options = format!("rw,noexec,nosuid,size={}m", spec.data_tmpfs_mb.max(1));

        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            CONTAINER_PORT.to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: None,
            }]),
        );

        let host_config = HostConfig {
            network_mode: Some(self.network_name.clone()),
            binds: Some(vec![format!("{backup_path}:{CONTAINER_BACKUP_PATH}:ro")]),
            tmpfs: Some(HashMap::from([(
                CONTAINER_DATA_PATH.to_string(),
                tmpfs_options,
            )])),
            memory: Some(memory_bytes),
            nano_cpus: Some(nano_cpus),
            port_bindings: Some(port_bindings),
            auto_remove: Some(false),
            ..HostConfig::default()
        };

        let body = ContainerCreateBody {
            image: Some(spec.image.clone()),
            env: Some(vec![
                format!("POSTGRES_PASSWORD={}", spec.password),
                format!("POSTGRES_USER={}", spec.postgres_user),
                format!("POSTGRES_DB={}", spec.database),
                "PGTZ=UTC".to_string(),
                "POSTGRES_INITDB_ARGS=--data-checksums".to_string(),
            ]),
            exposed_ports: Some(vec![CONTAINER_PORT.to_string()]),
            labels: Some(labels),
            host_config: Some(host_config),
            ..ContainerCreateBody::default()
        };

        let options = CreateContainerOptionsBuilder::new()
            .name(&self.container_name)
            .build();

        self.container_attempted = true;
        self.docker
            .create_container(Some(options), body)
            .await
            .map(|_| ())
            .map_err(|error| {
                SentinelaError::Docker(format!(
                    "falha ao criar container '{}': {error}",
                    self.container_name
                ))
            })
    }

    async fn start_container(&self) -> Result<()> {
        self.docker
            .start_container(&self.container_name, None::<StartContainerOptions>)
            .await
            .map_err(|error| {
                SentinelaError::Docker(format!(
                    "falha ao iniciar container '{}': {error}",
                    self.container_name
                ))
            })
    }

    async fn wait_until_ready(&self, spec: &SandboxSpec) -> Result<()> {
        let started = Instant::now();
        let mut attempts = 0_u64;
        let mut last_detail: String;

        loop {
            attempts += 1;
            let command = [
                "pg_isready".to_string(),
                "-U".to_string(),
                spec.postgres_user.clone(),
                "-d".to_string(),
                spec.database.clone(),
            ];
            match timeout(
                Duration::from_secs(5),
                self.exec_capture(&command, OutputLimits::NONE),
            )
            .await
            {
                Ok(Ok(output)) if output.exit_code == Some(0) => return Ok(()),
                Ok(Ok(output)) => {
                    last_detail = format!("pg_isready exit={:?}", output.exit_code);
                }
                Ok(Err(error)) => {
                    last_detail = error.to_string();
                }
                Err(_) => {
                    last_detail = "timeout no pg_isready".to_string();
                }
            }

            if started.elapsed() >= spec.readiness_timeout {
                return Err(SentinelaError::Docker(format!(
                    "timeout de {}s aguardando PostgreSQL ficar pronto ({} tentativas). Último detalhe: {}",
                    spec.readiness_timeout.as_secs(),
                    attempts,
                    last_detail
                )));
            }
            sleep(Duration::from_millis(500)).await;
        }
    }

    /// Executa a validação do dump, o restore e os testes SQL.
    pub async fn execute(
        &self,
        spec: &SandboxSpec,
        tests: &[sentinela_core::config::TestDefinition],
        statement_timeout_ms: u64,
    ) -> Result<RunExecution> {
        let restore = self.restore(spec).await?;
        if !restore.list_ok || restore.exit_code != Some(0) {
            return Ok(RunExecution {
                error: Some(if restore.list_ok {
                    format!("pg_restore terminou com código {:?}", restore.exit_code)
                } else {
                    "pg_restore --list recusou o dump".to_string()
                }),
                restore,
                tests: Vec::new(),
                status: RunStatus::RestoreFailed,
                tests_duration_ms: 0,
            });
        }

        let tests_started = Instant::now();
        let outcomes = if tests.is_empty() {
            Vec::new()
        } else {
            let port = self.host_port().await?;
            run_sql_tests(spec, port, tests, statement_timeout_ms).await?
        };
        let tests_duration_ms = millis(tests_started.elapsed());

        let failed = outcomes.iter().filter(|outcome| !outcome.passed()).count();
        let (status, error) = if failed == 0 {
            (RunStatus::Ok, None)
        } else {
            (
                RunStatus::TestsFailed,
                Some(format!(
                    "{failed} de {} testes não passaram",
                    outcomes.len()
                )),
            )
        };

        Ok(RunExecution {
            restore,
            tests: outcomes,
            status,
            error,
            tests_duration_ms,
        })
    }

    async fn restore(&self, spec: &SandboxSpec) -> Result<RestoreReport> {
        let started = Instant::now();
        let list_command = [
            "pg_restore".to_string(),
            "--list".to_string(),
            CONTAINER_BACKUP_PATH.to_string(),
        ];
        let list_result = timeout(
            spec.restore_timeout,
            self.exec_capture(&list_command, OutputLimits::NONE),
        )
        .await;

        let list_ok = match list_result {
            Ok(Ok(output)) => {
                if output.exit_code != Some(0) {
                    return Ok(RestoreReport {
                        duration_ms: millis(started.elapsed()),
                        list_ok: false,
                        exit_code: None,
                        stderr_tail: Some(tail(&output.stderr, REPORT_STDERR_LIMIT)),
                    });
                }
                true
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Ok(RestoreReport {
                    duration_ms: millis(started.elapsed()),
                    list_ok: false,
                    exit_code: None,
                    stderr_tail: Some("timeout na validação com pg_restore --list".into()),
                });
            }
        };

        let restore_command = [
            "pg_restore".to_string(),
            "-U".to_string(),
            spec.postgres_user.clone(),
            "-d".to_string(),
            spec.database.clone(),
            "--no-owner".to_string(),
            CONTAINER_BACKUP_PATH.to_string(),
        ];
        let limits = OutputLimits {
            stdout: 16 * 1024,
            stderr: 64 * 1024,
        };

        let restore_result = timeout(
            spec.restore_timeout,
            self.exec_capture(&restore_command, limits),
        )
        .await;

        match restore_result {
            Ok(Ok(output)) => Ok(RestoreReport {
                duration_ms: millis(started.elapsed()),
                list_ok,
                exit_code: output.exit_code,
                stderr_tail: if output.stderr.is_empty() {
                    None
                } else {
                    Some(tail(&output.stderr, REPORT_STDERR_LIMIT))
                },
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Ok(RestoreReport {
                duration_ms: millis(started.elapsed()),
                list_ok,
                exit_code: None,
                stderr_tail: Some(format!(
                    "timeout de {}s durante pg_restore",
                    spec.restore_timeout.as_secs()
                )),
            }),
        }
    }

    async fn host_port(&self) -> Result<u16> {
        let inspect = self
            .docker
            .inspect_container(&self.container_name, None)
            .await
            .map_err(|error| {
                SentinelaError::Docker(format!(
                    "falha ao inspecionar container '{}': {error}",
                    self.container_name
                ))
            })?;
        extract_host_port(&inspect).ok_or_else(|| {
            SentinelaError::Docker(format!(
                "container '{}' não expôs a porta {} em 127.0.0.1",
                self.container_name, CONTAINER_PORT
            ))
        })
    }

    async fn exec_capture(&self, command: &[String], limits: OutputLimits) -> Result<ExecOutput> {
        let options = CreateExecOptions::<String> {
            cmd: Some(command.to_vec()),
            attach_stdout: Some(limits.stdout > 0),
            attach_stderr: Some(limits.stderr > 0),
            attach_stdin: Some(false),
            tty: Some(false),
            ..CreateExecOptions::default()
        };
        let created = self
            .docker
            .create_exec(&self.container_name, options)
            .await
            .map_err(|error| {
                SentinelaError::Docker(format!(
                    "falha ao criar exec '{}' no container '{}': {error}",
                    command.first().map_or("?", String::as_str),
                    self.container_name
                ))
            })?;

        let started = self
            .docker
            .start_exec(&created.id, None::<StartExecOptions>)
            .await
            .map_err(|error| {
                SentinelaError::Docker(format!(
                    "falha ao executar '{}' no container '{}': {error}",
                    command.first().map_or("?", String::as_str),
                    self.container_name
                ))
            })?;

        let mut stdout = String::new();
        let mut stderr = String::new();
        match started {
            StartExecResults::Attached { output, input } => {
                drop(input);
                let mut stream = output;
                while let Some(item) = stream.next().await {
                    let item = item.map_err(|error| {
                        SentinelaError::Docker(format!(
                            "falha ao ler saída de '{}': {error}",
                            command.first().map_or("?", String::as_str)
                        ))
                    })?;
                    match item {
                        LogOutput::StdOut { message } | LogOutput::Console { message } => {
                            append_bounded(&mut stdout, &message, limits.stdout);
                        }
                        LogOutput::StdErr { message } => {
                            append_bounded(&mut stderr, &message, limits.stderr);
                        }
                        LogOutput::StdIn { .. } => {}
                    }
                }
            }
            StartExecResults::Detached => {
                return Err(SentinelaError::Docker(
                    "exec inesperadamente destacado".into(),
                ));
            }
        }

        let exit_code = self.inspect_exit_code(&created.id).await?;
        Ok(ExecOutput { exit_code, stderr })
    }

    async fn inspect_exit_code(&self, exec_id: &str) -> Result<Option<i32>> {
        for _ in 0..40 {
            let inspect = self.docker.inspect_exec(exec_id).await.map_err(|error| {
                SentinelaError::Docker(format!("falha ao inspecionar exec {exec_id}: {error}"))
            })?;
            if inspect.running == Some(false) {
                return Ok(inspect.exit_code.and_then(|code| i32::try_from(code).ok()));
            }
            sleep(Duration::from_millis(25)).await;
        }
        Ok(None)
    }

    /// Remove container e rede. É idempotente e nunca interrompe a tentativa de
    /// remover o segundo recurso quando o primeiro falha.
    pub async fn teardown(&mut self, keep: bool) -> Result<()> {
        if keep {
            info!(
                container = %self.container_name,
                network = %self.network_name,
                "limpeza ignorada por --keep"
            );
            return Ok(());
        }

        let mut first_error = None;
        if self.container_attempted {
            let options = RemoveContainerOptionsBuilder::new()
                .force(true)
                .v(true)
                .build();
            if let Err(error) = self
                .docker
                .remove_container(&self.container_name, Some(options))
                .await
            {
                if is_not_found(&error) {
                    warn!(container = %self.container_name, "container já não existia no teardown");
                } else {
                    warn!(container = %self.container_name, error = %error, "falha ao remover container");
                    first_error.get_or_insert_with(|| {
                        SentinelaError::Docker(format!("falha ao remover container: {error}"))
                    });
                }
            }
            self.container_attempted = false;
        }

        if self.network_attempted {
            if let Err(error) = self.docker.remove_network(&self.network_name).await {
                if is_not_found(&error) {
                    warn!(network = %self.network_name, "rede já não existia no teardown");
                } else {
                    warn!(network = %self.network_name, error = %error, "falha ao remover rede");
                    first_error.get_or_insert_with(|| {
                        SentinelaError::Docker(format!("falha ao remover rede: {error}"))
                    });
                }
            }
            self.network_attempted = false;
        }

        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for DockerSession {
    fn drop(&mut self) {
        if self.container_attempted || self.network_attempted {
            warn!(
                container = %self.container_name,
                network = %self.network_name,
                "sessão Docker descartada sem teardown explícito; execute 'sentinela cleanup'"
            );
        }
    }
}

fn extract_host_port(inspect: &ContainerInspectResponse) -> Option<u16> {
    inspect
        .network_settings
        .as_ref()?
        .ports
        .as_ref()?
        .get(CONTAINER_PORT)?
        .as_ref()?
        .iter()
        .find_map(|binding| binding.host_port.as_deref()?.parse::<u16>().ok())
}

fn append_bounded(output: &mut String, chunk: &[u8], limit: usize) {
    if limit == 0 || output.len() >= limit {
        return;
    }
    let text = String::from_utf8_lossy(chunk);
    for character in text.chars() {
        if output.len() + character.len_utf8() > limit {
            break;
        }
        output.push(character);
    }
}

fn tail(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    format!("...[truncado] {}", &value[start..])
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn is_not_found(error: &bollard::errors::Error) -> bool {
    matches!(
        error,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn docker_error(error: bollard::errors::Error) -> String {
    error.to_string()
}

/// Conecta ao Docker local e remove containers, redes e volumes com
/// `label=sentinela=true`.
pub async fn cleanup_orphans() -> Result<CleanupReport> {
    let docker = Docker::connect_with_local_defaults().map_err(|error| {
        SentinelaError::Docker(format!(
            "não foi possível conectar ao Docker local: {error}"
        ))
    })?;
    cleanup_orphans_with(&docker).await
}

async fn cleanup_orphans_with(docker: &Docker) -> Result<CleanupReport> {
    let mut report = CleanupReport::default();
    let mut filters = HashMap::new();
    filters.insert("label", vec!["sentinela=true"]);

    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::new()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await
        .map_err(|error| {
            SentinelaError::Docker(format!("falha ao listar containers órfãos: {error}"))
        })?;
    for container in containers {
        let Some(id) = container.id else { continue };
        let options = RemoveContainerOptionsBuilder::new()
            .force(true)
            .v(true)
            .build();
        match docker.remove_container(&id, Some(options)).await {
            Ok(()) => report.containers_removed += 1,
            Err(error) => report
                .errors
                .push(format!("container {id}: {}", docker_error(error))),
        }
    }

    let networks = docker
        .list_networks(Some(
            ListNetworksOptionsBuilder::new().filters(&filters).build(),
        ))
        .await
        .map_err(|error| SentinelaError::Docker(format!("falha ao listar redes órfãs: {error}")))?;
    for network in networks {
        let Some(name) = network.name else { continue };
        match docker.remove_network(&name).await {
            Ok(()) => report.networks_removed += 1,
            Err(error) => report
                .errors
                .push(format!("rede {name}: {}", docker_error(error))),
        }
    }

    let volumes = docker
        .list_volumes(Some(
            ListVolumesOptionsBuilder::new().filters(&filters).build(),
        ))
        .await
        .map_err(|error| {
            SentinelaError::Docker(format!("falha ao listar volumes órfãos: {error}"))
        })?;
    for volume in volumes.volumes.unwrap_or_default() {
        let options = RemoveVolumeOptionsBuilder::new().force(true).build();
        match docker.remove_volume(&volume.name, Some(options)).await {
            Ok(()) => report.volumes_removed += 1,
            Err(error) => {
                report
                    .errors
                    .push(format!("volume {}: {}", volume.name, docker_error(error)))
            }
        }
    }

    Ok(report)
}
