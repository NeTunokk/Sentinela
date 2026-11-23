//! Camada pura do aplicativo desktop.
//!
//! Não depende de Tauri: os comandos nativos apenas chamam estas funções e
//! empacotam o resultado. Assim `cargo test -p sentinela-gui` roda sem GTK/WebKit.

pub use sentinela_ui as app;

use sentinela_core::{Result, SentinelaError};
use sentinela_ui::UiState;

/// Converte erros de domínio na mensagem mostrada pelo frontend.
#[must_use]
pub fn command_error(error: SentinelaError) -> String {
    error.to_string()
}

/// JSON canônico do relatório, disponível somente após verificação.
pub fn report_json_text(state: &UiState, id: &str) -> Result<String> {
    let bytes = state.report_json(id)?;
    String::from_utf8(bytes)
        .map_err(|error| SentinelaError::Report(format!("relatório não é UTF-8 válido: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_error_is_human_readable() {
        let message = command_error(SentinelaError::Config("teste".to_string()));
        assert!(message.contains("teste"));
    }

    #[test]
    fn report_json_text_rejects_invalid_run_id_without_touching_disk() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = sentinela_core::paths::Paths {
            config_dir: directory.path().join("config"),
            data_dir: directory.path().join("data"),
        };
        let state = UiState::new(paths);
        assert!(report_json_text(&state, "../segredo").is_err());
    }
}
