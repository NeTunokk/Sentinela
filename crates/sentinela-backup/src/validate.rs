use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Executa `pg_restore --list` no host, se a ferramenta estiver disponível.
///
/// Retorna `None` quando o binário não existe ou não pôde ser executado. Nesse
/// caso a validação é feita dentro do container antes do restore. A validação
/// nunca faz o conteúdo do dump trafegar para fora da máquina.
pub async fn host_restore_list(path: &Path) -> Option<bool> {
    let mut command = Command::new("pg_restore");
    command
        .arg("--list")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            warn!("pg_restore não encontrado no host; validação será feita no container");
            return None;
        }
        Err(error) => {
            warn!(error = %error, "não foi possível executar pg_restore no host");
            return None;
        }
    };

    match timeout(VALIDATION_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => Some(output.status.success()),
        Ok(Err(error)) => {
            warn!(error = %error, "falha ao aguardar pg_restore --list");
            None
        }
        Err(_) => {
            warn!("timeout ao validar backup com pg_restore --list no host");
            Some(false)
        }
    }
}
