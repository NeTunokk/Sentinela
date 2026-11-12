use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{Result, SentinelaError};

/// Diretórios de configuração e de dados do Sentinela.
///
/// Respeita `SENTINELA_CONFIG_DIR`/`SENTINELA_DATA_DIR` e, quando ausentes,
/// `XDG_CONFIG_HOME`/`XDG_DATA_HOME` ou `~/.config`/`~/.local/share`.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    /// Descobre os diretórios padrão.
    ///
    /// Usa `HOME`; no Windows, recorre a `USERPROFILE` quando `HOME` não está
    /// definido, porque o Windows não define `HOME` por padrão. Sem esse
    /// fallback o executável não inicia em uma máquina de cliente.
    pub fn discover() -> Result<Self> {
        let home = home_dir().ok_or_else(|| {
            SentinelaError::Config(
                "nem HOME nem USERPROFILE estão definidos; não é possível localizar os diretórios do Sentinela"
                    .into(),
            )
        })?;

        let config_dir = match env::var_os("SENTINELA_CONFIG_DIR") {
            Some(value) => PathBuf::from(value),
            None => match env::var_os("XDG_CONFIG_HOME") {
                Some(value) => PathBuf::from(value).join("sentinela"),
                None => home.join(".config").join("sentinela"),
            },
        };

        let data_dir = match env::var_os("SENTINELA_DATA_DIR") {
            Some(value) => PathBuf::from(value),
            None => match env::var_os("XDG_DATA_HOME") {
                Some(value) => PathBuf::from(value).join("sentinela"),
                None => home.join(".local").join("share").join("sentinela"),
            },
        };

        Ok(Self {
            config_dir,
            data_dir,
        })
    }

    #[must_use]
    pub fn envs_dir(&self) -> PathBuf {
        self.config_dir.join("envs")
    }

    #[must_use]
    pub fn env_file(&self, name: &str) -> PathBuf {
        self.envs_dir().join(format!("{name}.yaml"))
    }

    #[must_use]
    pub fn global_config_file(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("sentinela.db")
    }

    #[must_use]
    pub fn reports_dir(&self) -> PathBuf {
        self.data_dir.join("reports")
    }

    #[must_use]
    pub fn keys_dir(&self) -> PathBuf {
        self.data_dir.join("keys")
    }

    #[must_use]
    pub fn signing_key_path(&self) -> PathBuf {
        self.keys_dir().join("report-signing.key")
    }

    #[must_use]
    pub fn verifying_key_path(&self) -> PathBuf {
        self.keys_dir().join("report-signing.pub")
    }

    /// Cria os diretórios necessários. Diretórios com material sensível usam
    /// permissão `0700` em sistemas Unix.
    pub fn ensure(&self) -> Result<()> {
        create_private_dir(&self.config_dir)?;
        create_private_dir(&self.envs_dir())?;
        create_private_dir(&self.data_dir)?;
        create_private_dir(&self.reports_dir())?;
        create_private_dir(&self.keys_dir())
    }
}

/// Diretório home do usuário.
///
/// `HOME` tem precedência. No Windows, `USERPROFILE` é usado como fallback
/// porque o sistema não define `HOME` por padrão.
fn home_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME");
    #[cfg(windows)]
    let userprofile = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let userprofile = None;
    resolve_home(home.as_deref(), userprofile.as_deref())
}

fn resolve_home(home: Option<&OsStr>, userprofile: Option<&OsStr>) -> Option<PathBuf> {
    [home, userprofile]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Expande exclusivamente um `~` inicial. Não usa variáveis de ambiente.
pub fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

/// Valida o nome de ambiente usado como nome de arquivo.
pub fn validate_env_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid && name != "." && name != ".." && !name.starts_with('.') {
        Ok(())
    } else {
        Err(SentinelaError::Config(format!(
            "nome de ambiente inválido: {name:?} (use letras, números, '-', '_' ou '.')"
        )))
    }
}

pub fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// Escreve um arquivo de forma atômica e com permissão `0600` em Unix.
pub fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let temp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))?;
    fs::write(temp.path(), contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(temp.path())?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(temp.path(), permissions)?;
    }
    temp.persist(path)
        .map_err(|error| SentinelaError::Io(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_env_names() {
        assert!(validate_env_name("producao").is_ok());
        assert!(validate_env_name("prod-2026_01").is_ok());
        assert!(validate_env_name("../etc").is_err());
        assert!(validate_env_name("").is_err());
        assert!(validate_env_name(".oculto").is_err());
    }

    #[test]
    fn expands_home() {
        let expanded = expand_tilde("~/backups");
        assert!(expanded.is_absolute());
        assert!(expanded.ends_with("backups"));
    }

    #[test]
    fn falls_back_to_userprofile_when_home_is_missing() {
        let userprofile = OsStr::new(r"C:\Users\cliente");

        assert_eq!(
            resolve_home(None, Some(userprofile)),
            Some(PathBuf::from(userprofile))
        );
        assert_eq!(
            resolve_home(Some(OsStr::new("")), Some(userprofile)),
            Some(PathBuf::from(userprofile)),
            "HOME vazio deve ser ignorado"
        );
        assert_eq!(
            resolve_home(Some(OsStr::new("/home/maria")), Some(userprofile)),
            Some(PathBuf::from("/home/maria")),
            "HOME tem precedência sobre USERPROFILE"
        );
        assert_eq!(resolve_home(None, None), None);
    }
}
