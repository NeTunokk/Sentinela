//! Controlador da interface: uma única implementação usada pelo painel legado
//! e pelo app desktop. Toda a E/S de interface (HTTP ou Tauri) fica nas camadas
//! externas; aqui há apenas lógica de domínio e formatos de dados.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use sentinela_backup::provider_for;
use sentinela_core::config::EnvironmentConfig;
use sentinela_core::model::Report;
use sentinela_core::paths::{validate_env_name, Paths};
use sentinela_core::report::{self as report_core, StoredReportRef, VerifiedReport};
use sentinela_core::{Result, SentinelaError};
use sentinela_runner::{
    cleanup_orphans, execute_run, list_orphans, CancellationToken, OrphanResource, ProgressSender,
    RunRequest,
};
use sentinela_store::Store;

use crate::dto::{
    report_dto, run_dto, run_dto_active, test_dto, BackupDto, BackupReportDto, BackupsPayload,
    CleanupPayload, EnvironmentDto, EnvironmentsPayload, RemovedEnvironmentPayload,
    ResourcesPayload, RunDetailPayload, RunsPayload, SavedEnvironmentPayload, StartRunPayload,
    SummaryDto, TimingsDto,
};

/// Estado de uma execução iniciada pela interface e ainda não persistida.
#[derive(Clone, Debug)]
pub struct ActiveRun {
    pub environment: String,
    pub started_at: DateTime<Utc>,
    pub state: ActiveRunState,
}

#[derive(Clone, Debug)]
pub enum ActiveRunState {
    Running,
    Failed(String),
}

/// Estado compartilhado por todas as chamadas da interface.
#[derive(Clone)]
pub struct UiState {
    pub paths: Paths,
    active: Arc<Mutex<HashMap<String, ActiveRun>>>,
    cancel: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl UiState {
    #[must_use]
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            active: Arc::new(Mutex::new(HashMap::new())),
            cancel: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Lista os ambientes e seus erros de carregamento, sem expor dados de linhas.
    pub fn list_environments(&self) -> Result<EnvironmentsPayload> {
        let mut files = fs::read_dir(self.paths.envs_dir())?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| is_yaml(path))
            .collect::<Vec<_>>();
        files.sort();

        let mut environments = Vec::with_capacity(files.len());
        for file in files {
            let name = file
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("?")
                .to_string();
            let (config, error) = match EnvironmentConfig::load(&file) {
                Ok(config) => (Some(config), None),
                Err(error) => (None, Some(error.to_string())),
            };
            environments.push(EnvironmentDto::new(&name, config.as_ref(), error));
        }
        environments.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(EnvironmentsPayload { environments })
    }

    pub fn get_environment(&self, name: &str) -> Result<EnvironmentConfig> {
        validate_env_name(name)?;
        let path = self.paths.env_file(name);
        if !path.exists() {
            return Err(not_found("ambiente", name));
        }
        EnvironmentConfig::load(&path)
    }

    pub fn save_environment(
        &self,
        name: &str,
        config: &EnvironmentConfig,
    ) -> Result<SavedEnvironmentPayload> {
        validate_env_name(name)?;
        if config.name != name {
            return Err(SentinelaError::Config(format!(
                "o nome do ambiente no corpo ('{}') precisa ser igual ao da URL ('{}')",
                config.name, name
            )));
        }
        config.save(&self.paths.env_file(name))?;
        let store = Store::open(&self.paths.database_path())?;
        let config_json = serde_json::to_string(config)
            .map_err(|error| SentinelaError::Store(format!("serializar ambiente: {error}")))?;
        store.upsert_environment(&config.name, &config_json)?;
        Ok(SavedEnvironmentPayload {
            saved: true,
            name: name.to_string(),
        })
    }

    pub fn delete_environment(&self, name: &str) -> Result<RemovedEnvironmentPayload> {
        validate_env_name(name)?;
        let path = self.paths.env_file(name);
        let file_removed = if path.exists() {
            fs::remove_file(&path).map_err(SentinelaError::Io)?;
            true
        } else {
            false
        };

        let store = Store::open(&self.paths.database_path())?;
        let row_removed = store.remove_environment(name)?;
        if !file_removed && !row_removed {
            return Err(not_found("ambiente", name));
        }

        Ok(RemovedEnvironmentPayload {
            removed: true,
            name: name.to_string(),
            history_preserved: true,
        })
    }

    pub async fn list_backups(&self, name: &str) -> Result<BackupsPayload> {
        validate_env_name(name)?;
        let path = self.paths.env_file(name);
        if !path.exists() {
            return Err(not_found("ambiente", name));
        }
        let config = EnvironmentConfig::load(&path)?;
        let provider = provider_for(config.backup.provider);
        let backups = provider.discover(&config).await?;
        let backups = backups
            .into_iter()
            .map(|backup| BackupDto {
                id: backup.id,
                provider: backup.provider,
                size_bytes: backup.size_bytes,
                modified_at: backup.modified_at.to_rfc3339(),
                sha256: backup.sha256,
                host_restore_list_ok: backup.host_restore_list_ok,
            })
            .collect();
        Ok(BackupsPayload { backups })
    }

    pub fn list_runs(
        &self,
        environment: Option<&str>,
        status: Option<&str>,
    ) -> Result<RunsPayload> {
        let environment = environment.filter(|value| !value.is_empty());
        if let Some(environment) = environment {
            validate_env_name(environment)?;
        }
        let status = status.filter(|value| !value.is_empty());
        let store = Store::open(&self.paths.database_path())?;
        let runs = store.list_runs(environment, status)?;
        let mut items = Vec::with_capacity(runs.len());
        for run in &runs {
            let detail = store.get_run(&run.id)?;
            items.push(run_dto(run, detail.as_ref()));
        }
        Ok(RunsPayload { runs: items })
    }

    /// Detalhe da execução. Só inclui relatório/timings quando a verificação
    /// criptográfica passou; caso contrário `report.verified` é `false`.
    pub async fn get_run(&self, id: &str) -> Result<RunDetailPayload> {
        report_core::validate_run_id(id)?;

        let detail = {
            let store = Store::open(&self.paths.database_path())?;
            store.get_run(id)?
        };

        if let Some(detail) = detail {
            let stored = detail.report.as_ref().map(stored_report_ref);
            let verification = report_core::verify_stored_report(&self.paths, id, stored.as_ref());
            let report = report_dto(
                detail.report.as_ref(),
                verification
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .as_deref(),
            );
            let parsed = verification
                .as_ref()
                .ok()
                .and_then(|verified| serde_json::from_slice::<Report>(&verified.bytes).ok());
            return Ok(RunDetailPayload {
                pending: false,
                run: run_dto(&detail.run, Some(&detail)),
                tests: detail.tests.iter().map(test_dto).collect(),
                report: Some(report),
                timings: parsed.as_ref().map(TimingsDto::from),
                backup: parsed.as_ref().map(BackupReportDto::from),
                summary: parsed.as_ref().map(SummaryDto::from),
            });
        }

        let active = {
            let active_runs = self.active.lock().await;
            active_runs.get(id).cloned()
        };
        let Some(active) = active else {
            return Err(not_found("execução", id));
        };

        let (status, error_message, pending) = match &active.state {
            ActiveRunState::Running => ("running", None, true),
            ActiveRunState::Failed(message) => ("infra_error", Some(message.clone()), false),
        };

        Ok(RunDetailPayload {
            pending,
            run: run_dto_active(
                id,
                &active.environment,
                status,
                error_message,
                active.started_at,
            ),
            tests: Vec::new(),
            report: None,
            timings: None,
            backup: None,
            summary: None,
        })
    }

    /// Enfileira uma execução. `progress`, quando informado, recebe os eventos
    /// de fase que a camada Tauri encaminha para o frontend.
    pub async fn start_run(
        &self,
        environment: &str,
        backup: Option<String>,
        progress: Option<ProgressSender>,
    ) -> Result<StartRunPayload> {
        validate_env_name(environment)?;
        let env_path = self.paths.env_file(environment);
        if !env_path.exists() {
            return Err(not_found("ambiente", environment));
        }
        EnvironmentConfig::load(&env_path)?;

        let run_id = Uuid::new_v4().to_string();
        let started_at = Utc::now();
        {
            let mut active = self.active.lock().await;
            active.retain(|_, run| matches!(run.state, ActiveRunState::Running));
            if let Some((existing, _)) = active
                .iter()
                .find(|(_, run)| matches!(run.state, ActiveRunState::Running))
            {
                return Err(SentinelaError::Config(format!(
                    "já existe uma execução em andamento ({existing})"
                )));
            }
            active.insert(
                run_id.clone(),
                ActiveRun {
                    environment: environment.to_string(),
                    started_at,
                    state: ActiveRunState::Running,
                },
            );
        }

        let cancel = CancellationToken::new();
        {
            let mut tokens = self.cancel.lock().await;
            tokens.insert(run_id.clone(), cancel.clone());
        }

        let state = self.clone();
        let task_run_id = run_id.clone();
        let task_env = environment.to_string();
        tokio::spawn(async move {
            let outcome = execute_run(RunRequest {
                run_id: task_run_id.clone(),
                environment: task_env.clone(),
                backup,
                keep: false,
                paths: state.paths.clone(),
                progress,
                cancel: cancel.clone(),
            })
            .await;

            let mut active = state.active.lock().await;
            match outcome {
                Ok(_) => {
                    active.remove(&task_run_id);
                }
                Err(error) => {
                    if !matches!(error, SentinelaError::Interrupted)
                        && matches!(
                            active.get(&task_run_id).map(|run| &run.state),
                            Some(ActiveRunState::Running)
                        )
                    {
                        active.insert(
                            task_run_id.clone(),
                            ActiveRun {
                                environment: task_env.clone(),
                                started_at,
                                state: ActiveRunState::Failed(error.to_string()),
                            },
                        );
                    }
                }
            }
            let mut tokens = state.cancel.lock().await;
            tokens.remove(&task_run_id);
        });

        Ok(StartRunPayload {
            run_id,
            status: "running",
        })
    }

    /// Cancela uma execução ativa. A limpeza é feita pela orquestração:
    /// `execute_run` remove container/rede e os órfãos antes de encerrar.
    pub async fn cancel_run(&self, id: &str) -> Result<bool> {
        report_core::validate_run_id(id)?;
        let token = {
            let tokens = self.cancel.lock().await;
            tokens.get(id).cloned()
        };
        let Some(token) = token else {
            return Ok(false);
        };
        token.cancel();

        let mut active = self.active.lock().await;
        if let Some(run) = active.get_mut(id) {
            run.state = ActiveRunState::Failed(
                "execução cancelada pelo usuário; recursos Docker removidos".to_string(),
            );
        }
        Ok(true)
    }

    /// Retorna os bytes canônicos do relatório somente após verificação.
    pub fn report_json(&self, id: &str) -> Result<Vec<u8>> {
        Ok(self.verify_report(id)?.bytes)
    }

    /// Gera o HTML a partir do JSON verificado: mesmo conteúdo de sempre, mas
    /// impossível de exibir um arquivo adulterado no disco.
    pub fn report_html(&self, id: &str) -> Result<String> {
        let verified = self.verify_report(id)?;
        let report: Report = serde_json::from_slice(&verified.bytes).map_err(|error| {
            SentinelaError::Report(format!("relatório inválido após verificação: {error}"))
        })?;
        sentinela_core::report::html::render(
            &report,
            &verified.sha256,
            &verified.signature,
            &verified.public_key,
        )
    }

    pub async fn list_resources(&self) -> Result<ResourcesPayload<OrphanResource>> {
        let resources = list_orphans().await?;
        Ok(ResourcesPayload { resources })
    }

    pub async fn cleanup(&self) -> Result<CleanupPayload> {
        let report = cleanup_orphans().await?;
        Ok(CleanupPayload {
            containers_removed: report.containers_removed,
            networks_removed: report.networks_removed,
            volumes_removed: report.volumes_removed,
            errors: report.errors,
        })
    }

    fn verify_report(&self, id: &str) -> Result<VerifiedReport> {
        report_core::validate_run_id(id)?;
        let store = Store::open(&self.paths.database_path())?;
        let detail = store.get_run(id)?;
        let detail = detail.ok_or_else(|| not_found("execução", id))?;
        let stored = detail.report.as_ref().map(stored_report_ref);
        report_core::verify_stored_report(&self.paths, id, stored.as_ref())
    }
}

fn stored_report_ref(report: &sentinela_store::ReportRow) -> StoredReportRef {
    StoredReportRef::new(
        &report.json_path,
        &report.sha256,
        &report.signature,
        &report.public_key,
    )
}

fn is_yaml(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        })
}

fn not_found(kind: &str, id: &str) -> SentinelaError {
    SentinelaError::Config(format!("{kind} '{id}' não encontrado"))
}
