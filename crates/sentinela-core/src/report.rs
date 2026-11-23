//! Assinatura e verificação de relatórios.
//!
//! Esta camada é compartilhada pela CLI, pelo painel legado e pela interface
//! desktop: o formato JSON/HTML e a assinatura Ed25519 permanecem idênticos.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::hash::{sha256_digest, sha256_hex};
use crate::model::Report;
use crate::paths::{create_private_dir, write_private_file, Paths};
use crate::{Result, SentinelaError};

#[path = "report_html.rs"]
pub mod html;
#[path = "report_json.rs"]
pub mod json;

const KEY_LEN: usize = 32;
const SIGNATURE_LEN: usize = 64;
const PUBLIC_KEY_LEN: usize = 32;

/// Artefatos assinados de um relatório gravados em disco.
#[derive(Clone, Debug)]
pub struct ReportArtifacts {
    pub json_path: PathBuf,
    pub html_path: PathBuf,
    pub sha256: String,
    pub signature: String,
    pub public_key: String,
}

/// Gera JSON, HTML, hash e assinatura, e grava todos os artefatos em disco.
///
/// O formato dos arquivos é idêntico ao da versão 0.1: nenhum campo foi
/// acrescentado ou removido.
pub fn write_reports(
    paths: &Paths,
    report: &Report,
    signer: &ReportSigner,
) -> Result<ReportArtifacts> {
    create_private_dir(&paths.reports_dir())?;

    let json_bytes = json::serialize(report)?;
    let digest = sha256_digest(&json_bytes);
    let sha256 = sha256_hex(&json_bytes);
    let signature = signer.sign_digest(&digest);
    let public_key = signer.public_key_base64().to_string();
    let html = html::render(report, &sha256, &signature, &public_key)?;

    let base = paths.reports_dir().join(&report.run_id);
    let json_path = base.with_extension("json");
    let html_path = base.with_extension("html");
    let sha_path = base.with_extension("sha256");
    let signature_path = base.with_extension("sig");
    let public_path = base.with_extension("pub");

    write_private_file(&json_path, &json_bytes)?;
    write_private_file(&html_path, html.as_bytes())?;
    write_private_file(&sha_path, format!("{sha256}\n").as_bytes())?;
    write_private_file(&signature_path, format!("{signature}\n").as_bytes())?;
    write_private_file(&public_path, format!("{public_key}\n").as_bytes())?;

    Ok(ReportArtifacts {
        json_path,
        html_path,
        sha256,
        signature,
        public_key,
    })
}

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
    #[must_use]
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

/// Recalcula o hash, confere o hash esperado (se houver) e valida a assinatura.
pub fn verify_report_bytes(
    json_bytes: &[u8],
    expected_sha256: Option<&str>,
    signature: &str,
    public_key: &str,
) -> Result<String> {
    let digest = sha256_digest(json_bytes);
    let sha256 = sha256_hex(json_bytes);
    if let Some(expected) = expected_sha256 {
        if !expected.eq_ignore_ascii_case(&sha256) {
            return Err(SentinelaError::Report(format!(
                "SHA-256 do relatório não confere: esperado {expected}, calculado {sha256}"
            )));
        }
    }
    verify_digest(&digest, signature, public_key)?;
    Ok(sha256)
}

/// Valida o identificador de uma execução antes de tocar no filesystem.
pub fn validate_run_id(run_id: &str) -> Result<()> {
    let valid = !run_id.is_empty()
        && run_id.len() <= 64
        && run_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if valid {
        Ok(())
    } else {
        Err(SentinelaError::Config(format!(
            "run_id inválido: {run_id:?}"
        )))
    }
}

/// Referência opcional aos metadados gravados no banco pelo `Store`.
///
/// Os sidecars no disco têm prioridade; o banco é usado apenas como fallback,
/// exatamente como na CLI e no painel web.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredReportRef {
    pub json_path: Option<PathBuf>,
    pub sha256: Option<String>,
    pub signature: Option<String>,
    pub public_key: Option<String>,
}

impl StoredReportRef {
    /// Constrói a referência a partir das colunas do histórico.
    #[must_use]
    pub fn new(json_path: &str, sha256: &str, signature: &str, public_key: &str) -> Self {
        Self {
            json_path: Some(PathBuf::from(json_path)),
            sha256: Some(sha256.to_string()),
            signature: Some(signature.to_string()),
            public_key: Some(public_key.to_string()),
        }
    }
}

/// Caminho e sidecars resolvidos de um relatório.
#[derive(Clone, Debug)]
pub struct ReportArtifactLocation {
    pub json_path: PathBuf,
    pub expected_sha256: Option<String>,
    pub signature: String,
    pub public_key: String,
}

impl ReportArtifactLocation {
    #[must_use]
    pub fn html_path(&self) -> PathBuf {
        self.json_path.with_extension("html")
    }
}

/// Relatório cujo SHA-256 e assinatura foram verificados com sucesso.
#[derive(Clone, Debug)]
pub struct VerifiedReport {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub signature: String,
    pub public_key: String,
}

/// Lê um sidecar `.sha256`, `.sig` ou `.pub`.
pub fn read_sidecar(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
}

/// Resolve o JSON e os sidecars de uma execução.
pub fn locate_artifacts(
    paths: &Paths,
    run_id: &str,
    stored: Option<&StoredReportRef>,
) -> Result<ReportArtifactLocation> {
    let json_path = stored
        .and_then(|report| report.json_path.clone())
        .unwrap_or_else(|| paths.reports_dir().join(format!("{run_id}.json")));

    let expected_sha256 = read_sidecar(&json_path.with_extension("sha256"))
        .or_else(|| stored.and_then(|report| report.sha256.clone()));
    let signature = read_sidecar(&json_path.with_extension("sig"))
        .or_else(|| stored.and_then(|report| report.signature.clone()))
        .ok_or_else(|| {
            SentinelaError::Report(format!(
                "assinatura ausente para {} (esperado o sidecar .sig)",
                json_path.display()
            ))
        })?;
    let public_key = read_sidecar(&json_path.with_extension("pub"))
        .or_else(|| stored.and_then(|report| report.public_key.clone()))
        .ok_or_else(|| {
            SentinelaError::Report(format!(
                "chave pública ausente para {} (esperado o sidecar .pub)",
                json_path.display()
            ))
        })?;

    Ok(ReportArtifactLocation {
        json_path,
        expected_sha256,
        signature,
        public_key,
    })
}

/// Lê e verifica o relatório de uma execução.
///
/// Nenhum bytes é devolvido antes de recalcular o SHA-256 e validar a
/// assinatura Ed25519. Chamadores devem tratar `Err` como "adulterado".
pub fn verify_stored_report(
    paths: &Paths,
    run_id: &str,
    stored: Option<&StoredReportRef>,
) -> Result<VerifiedReport> {
    let location = locate_artifacts(paths, run_id, stored)?;
    let bytes = fs::read(&location.json_path).map_err(|error| {
        SentinelaError::Report(format!(
            "não foi possível ler o relatório {}: {error}",
            location.json_path.display()
        ))
    })?;
    let sha256 = verify_report_bytes(
        &bytes,
        location.expected_sha256.as_deref(),
        &location.signature,
        &location.public_key,
    )?;

    Ok(VerifiedReport {
        bytes,
        sha256,
        signature: location.signature,
        public_key: location.public_key,
    })
}

fn set_key_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256_digest;

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = Paths {
            config_dir: directory.path().join("config"),
            data_dir: directory.path().join("data"),
        };
        paths.ensure().expect("ensure");
        (directory, paths)
    }

    #[test]
    fn signs_and_verifies_digest() {
        let (_temp, paths) = temp_paths();
        let signer = ReportSigner::load_or_create(&paths).expect("signer");
        let digest = sha256_digest(b"relatorio");
        let signature = signer.sign_digest(&digest);
        verify_digest(&digest, &signature, signer.public_key_base64()).expect("verified");

        let other = sha256_digest(b"outro");
        assert!(verify_digest(&other, &signature, signer.public_key_base64()).is_err());
    }

    #[test]
    fn rejects_invalid_signature_size() {
        let digest = sha256_digest(b"x");
        assert!(verify_digest(&digest, "abc", "abc").is_err());
    }

    #[test]
    fn detects_tampered_report_after_resolving_sidecars() {
        let (_temp, paths) = temp_paths();
        let signer = ReportSigner::load_or_create(&paths).expect("signer");
        let json = br#"{"schema_version":1,"run_id":"run-1"}"#;
        let digest = sha256_digest(json);
        let sha256 = sha256_hex(json);
        let signature = signer.sign_digest(&digest);
        let public_key = signer.public_key_base64();

        create_private_dir(&paths.reports_dir()).expect("reports dir");
        let json_path = paths.reports_dir().join("run-1.json");
        write_private_file(&json_path, json).expect("json");
        write_private_file(
            &json_path.with_extension("sha256"),
            format!("{sha256}\n").as_bytes(),
        )
        .expect("sha256");
        write_private_file(
            &json_path.with_extension("sig"),
            format!("{signature}\n").as_bytes(),
        )
        .expect("sig");
        write_private_file(
            &json_path.with_extension("pub"),
            format!("{public_key}\n").as_bytes(),
        )
        .expect("pub");

        let verified = verify_stored_report(&paths, "run-1", None).expect("verified");
        assert_eq!(verified.sha256, sha256);

        write_private_file(&json_path, br#"{"schema_version":1,"run_id":"run-2"}"#)
            .expect("tamper");
        assert!(verify_stored_report(&paths, "run-1", None).is_err());
    }

    #[test]
    fn exposes_key_paths_for_debugging() {
        let (_temp, paths) = temp_paths();
        assert!(paths.signing_key_path().ends_with("report-signing.key"));
        assert!(paths.verifying_key_path().ends_with("report-signing.pub"));
    }
}
