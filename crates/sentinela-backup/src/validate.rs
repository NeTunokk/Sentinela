use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sentinela_core::hash::sha256_file;
use sentinela_core::{Result, SentinelaError};
use tokio::process::Command;
use tokio::task;
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

/// Confere, imediatamente antes do restore, que o backup ainda é o mesmo
/// arquivo que foi descoberto.
///
/// O SHA-256 calculado na descoberta é rastreabilidade; esta checagem é a
/// barreira. Se o arquivo mudou depois da descoberta (cópia em andamento,
/// quarentena de antivírus, rotação sobrescrevendo o mesmo nome, share de rede
/// reconectado), a execução é abortada antes de montar o arquivo no container e
/// antes de qualquer restore.
///
/// Quando existe um arquivo de checksum ao lado do dump (`<dump>.sha256` ou
/// `<dump sem extensão>.sha256`), ele também precisa concordar com o hash da
/// descoberta.
pub async fn verify_unchanged(path: &Path, expected_sha256: &str) -> Result<()> {
    let expected = normalize_hash(expected_sha256).ok_or_else(|| {
        SentinelaError::Backup(format!(
            "hash esperado inválido para '{}': {expected_sha256:?}",
            path.display()
        ))
    })?;

    for sidecar in existing_sidecars(path) {
        let declared = read_sidecar(&sidecar)?;
        if declared != expected {
            return Err(SentinelaError::Backup(format!(
                "checksum '{}' não confere com o backup descoberto: arquivo diz {declared}, descoberta diz {expected}",
                sidecar.display()
            )));
        }
    }

    let inspect_path = path.to_path_buf();
    let actual = task::spawn_blocking(move || sha256_file(&inspect_path))
        .await
        .map_err(|error| {
            SentinelaError::Backup(format!("falha ao recalcular o SHA-256 do backup: {error}"))
        })??;

    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(SentinelaError::Backup(format!(
            "backup '{}' mudou depois da descoberta: esperado {expected}, calculado {actual}",
            path.display()
        )));
    }

    Ok(())
}

fn existing_sidecars(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.with_extension("sha256"), append_sha256(path)];
    candidates.sort();
    candidates.dedup();
    candidates.retain(|candidate| candidate.is_file());
    candidates
}

fn append_sha256(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(std::ffi::OsString::new, std::ffi::OsStr::to_os_string);
    name.push(".sha256");
    path.with_file_name(name)
}

fn read_sidecar(path: &Path) -> Result<String> {
    let raw = fs::read_to_string(path).map_err(|error| {
        SentinelaError::Backup(format!(
            "não foi possível ler '{}': {error}",
            path.display()
        ))
    })?;
    let token = raw.split_whitespace().next().unwrap_or_default();
    normalize_hash(token).ok_or_else(|| {
        SentinelaError::Backup(format!(
            "checksum inválido em '{}': esperado SHA-256 em hexadecimal",
            path.display()
        ))
    })
}

fn normalize_hash(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn write_dump(directory: &Path, contents: &[u8]) -> (PathBuf, String) {
        let path = directory.join("prod-01.dump");
        fs::write(&path, contents).expect("escrever dump");
        let sha256 = sha256_file(&path).expect("calcular hash");
        (path, sha256)
    }

    #[tokio::test]
    async fn accepts_backup_that_did_not_change() {
        let directory = tempfile::tempdir().expect("diretório temporário");
        let (path, sha256) = write_dump(directory.path(), b"dump intacto").await;

        verify_unchanged(&path, &sha256)
            .await
            .expect("backup intacto deve ser aceito");
    }

    #[tokio::test]
    async fn rejects_backup_that_changed_after_discovery() {
        let directory = tempfile::tempdir().expect("diretório temporário");
        let (path, sha256) = write_dump(directory.path(), b"dump original").await;
        fs::write(&path, b"dump sobrescrito").expect("sobrescrever dump");

        let error = verify_unchanged(&path, &sha256)
            .await
            .expect_err("backup alterado deve ser recusado");
        assert!(error.to_string().contains("mudou depois da descoberta"));
    }

    #[tokio::test]
    async fn rejects_sidecar_that_disagrees() {
        let directory = tempfile::tempdir().expect("diretório temporário");
        let (path, sha256) = write_dump(directory.path(), b"dump intacto").await;
        let sidecar = path.with_extension("sha256");
        fs::write(&sidecar, format!("{}\n", "a".repeat(64))).expect("escrever sidecar");

        let error = verify_unchanged(&path, &sha256)
            .await
            .expect_err("sidecar divergente deve ser recusado");
        assert!(error.to_string().contains("não confere"));
    }

    #[tokio::test]
    async fn rejects_invalid_sidecar() {
        let directory = tempfile::tempdir().expect("diretório temporário");
        let (path, sha256) = write_dump(directory.path(), b"dump intacto").await;
        fs::write(append_sha256(&path), "nao-e-um-hash\n").expect("escrever sidecar");

        let error = verify_unchanged(&path, &sha256)
            .await
            .expect_err("sidecar inválido deve ser recusado");
        assert!(error.to_string().contains("checksum inválido"));
    }

    #[test]
    fn rejects_malformed_expected_hash() {
        assert!(normalize_hash("00").is_none());
        assert!(normalize_hash(&"z".repeat(64)).is_none());
        assert_eq!(
            normalize_hash(&"A".repeat(64)),
            Some("a".repeat(64)),
            "hash hexadecimal deve ser normalizado para minúsculas"
        );
    }
}
