//! vygr-core: shared types, provider traits, configuration and the error /
//! exit-code contract used by every other crate in the workspace.

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::Config;
pub use error::VygrError;
