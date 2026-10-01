//! Error taxonomy and the process exit-code contract (ADR-0008).
//!
//! | code | meaning                     |
//! |------|-----------------------------|
//! | 0    | success                     |
//! | 1    | internal / unexpected error |
//! | 2    | usage error (clap)          |
//! | 3    | configuration or auth error |
//! | 4    | provider / network failure  |

#[derive(Debug, thiserror::Error)]
pub enum VygrError {
    #[error("config: {0}")]
    Config(String),

    #[error("auth: {0}")]
    Auth(String),

    #[error("provider {provider}: {message}")]
    Provider { provider: String, message: String },

    #[error("network: {0}")]
    Network(String),

    #[error("parse: {0}")]
    Parse(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
}

impl VygrError {
    pub fn exit_code(&self) -> i32 {
        match self {
            VygrError::Config(_) | VygrError::Auth(_) => 3,
            VygrError::Provider { .. } | VygrError::Network(_) | VygrError::Parse(_) => 4,
            VygrError::Io(_) | VygrError::Json(_) | VygrError::NotImplemented(_) => 1,
        }
    }
}
