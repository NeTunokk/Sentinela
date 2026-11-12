//! Núcleo do Sentinela: configuração, modelos, expectativas e erros.
//!
//! Este crate não fala com Docker, SQLite, PostgreSQL nem com a rede. Ele é a
//! fonte de verdade para os tipos compartilhados entre os demais crates.

pub mod config;
pub mod error;
pub mod expectation;
pub mod hash;
pub mod model;
pub mod paths;

pub use error::{Result, SentinelaError};
