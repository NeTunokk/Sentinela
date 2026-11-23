//! Lógica da interface do Sentinela compartilhada pelo painel web legado e
//! pelo aplicativo desktop Tauri.

pub mod controller;
pub mod dto;

#[cfg(test)]
mod tests;

pub use controller::{ActiveRun, ActiveRunState, UiState};
pub use dto::*;
