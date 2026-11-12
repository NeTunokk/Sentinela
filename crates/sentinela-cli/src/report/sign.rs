use std::fs;
use std::path::Path;

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::Rng;
use sentinela_core::paths::{create_private_dir, write_private_file, Paths};
use sentinela_core::{Result, SentinelaError};

const KEY_LEN: usize = 32;
const SIGNATURE_LEN: usize = 64;
const PUBLIC_KEY_LEN: usize = 32;

/// Chave Ed25519 local usada para assinar o SHA-256 dos relatórios.
///
/// A chave privada fica em `<data>/keys/report-signing.key` com permissão
/// `0600`; a pública é derivada e salva ao lado.
pub struct ReportSigner {
    signing_key: SigningKey,
    public_key_base64: String,
}

impl ReportSigner {
    pub fn load_or_create(paths: &Paths) -> Result<Self> {
        create_private_dir(&paths.keys_dir())?;
        let key_path = paths.signing_key_path();
        let secret = if key_path.exists() {
            let bytes = fs::read(&key_path)?;
            if bytes.len() != KEY_LEN {
                return Err(SentinelaError::Report(format!(
                    "chave de assinatura inválida em {}: esperado {} bytes, encontrado {}",
                    key_path.display(),
                    KEY_LEN,
                    bytes.len()
                )));
            }
            let mut secret = [0_u8; KEY_LEN];
            secret.copy_from_slice(&bytes);
            set_key_permissions(&key_path)?;
            secret
        } else {
            let mut secret = [0_u8; KEY_LEN];
            rand::rng().fill_bytes(&mut secret);
            write_private_file(&key_path, &secret)?;
            secret
        };

        let signing_key = SigningKey::from_bytes(&secret);
        let public_key_base64 = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        let public_path = paths.verifying_key_path();
        let current = fs::read_to_string(&public_path).ok();
        if current.as_deref().map(str::trim) != Some(public_key_base64.as_str()) {
            write_private_file(&public_path, public_key_base64.as_bytes())?;
        }

        Ok(Self {
            signing_key,
            public_key_base64,
        })
    }

    #[must_use]
    pub fn public_key_base64(&self) -> &str {
        &self.public_key_base64
    }

    /// Assina o digest SHA-256 (32 bytes) e retorna a assinatura em base64.
    pub fn sign_digest(&self, digest: &[u8; 32]) -> String {
        let signature = self.signing_key.sign(digest);
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
    }
}

/// Verifica uma assinatura Ed25519 de um digest SHA-256.
pub fn verify_digest(
    digest: &[u8; 32],
    signature_base64: &str,
    public_key_base64: &str,
) -> Result<()> {
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_base64.trim())
        .map_err(|error| SentinelaError::Report(format!("assinatura base64 inválida: {error}")))?;
    let signature_array: [u8; SIGNATURE_LEN] = signature_bytes
        .as_slice()
        .try_into()
        .map_err(|_| SentinelaError::Report("tamanho de assinatura inválido".into()))?;

    let public_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key_base64.trim())
        .map_err(|error| {
            SentinelaError::Report(format!("chave pública base64 inválida: {error}"))
        })?;
    let public_array: [u8; PUBLIC_KEY_LEN] = public_bytes
        .as_slice()
        .try_into()
        .map_err(|_| SentinelaError::Report("tamanho de chave pública inválido".into()))?;

    let verifying_key = VerifyingKey::from_bytes(&public_array).map_err(|error| {
        SentinelaError::Report(format!("chave pública Ed25519 inválida: {error}"))
    })?;
    let signature = Signature::from_bytes(&signature_array);

    verifying_key
        .verify_strict(digest, &signature)
        .map_err(|error| SentinelaError::Report(format!("assinatura não confere: {error}")))
}

fn set_key_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sentinela_core::hash::sha256_digest;

    use super::*;

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let directory = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: directory.path().join("config"),
            data_dir: directory.path().join("data"),
        };
        paths.ensure().unwrap();
        (directory, paths)
    }

    #[test]
    fn signs_and_verifies_digest() {
        let (_temp, paths) = temp_paths();
        let signer = ReportSigner::load_or_create(&paths).unwrap();
        let digest = sha256_digest(b"relatorio");
        let signature = signer.sign_digest(&digest);
        verify_digest(&digest, &signature, signer.public_key_base64()).unwrap();

        let other = sha256_digest(b"outro");
        assert!(verify_digest(&other, &signature, signer.public_key_base64()).is_err());
    }

    #[test]
    fn reuses_key_and_keeps_private_permissions() {
        let (_temp, paths) = temp_paths();
        let first = ReportSigner::load_or_create(&paths).unwrap();
        let second = ReportSigner::load_or_create(&paths).unwrap();
        assert_eq!(first.public_key_base64(), second.public_key_base64());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(paths.signing_key_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn rejects_invalid_signature_size() {
        let digest = sha256_digest(b"x");
        assert!(verify_digest(&digest, "abc", "abc").is_err());
    }

    #[test]
    fn exposes_paths_for_debugging() {
        let (_temp, paths) = temp_paths();
        assert!(paths
            .signing_key_path()
            .ends_with(PathBuf::from("report-signing.key")));
    }
}
