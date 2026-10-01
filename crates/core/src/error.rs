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
    Provider {
        provider: String,
        message: String,
        /// HTTP status when the error originates from a response.
        status: Option<u16>,
    },

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
    #[doc(hidden)]
    pub fn provider(provider: impl Into<String>, message: impl Into<String>) -> Self {
        VygrError::Provider {
            provider: provider.into(),
            message: message.into(),
            status: None,
        }
    }

    /// Provider error carrying the HTTP status (used for retry decisions).
    #[doc(hidden)]
    pub fn provider_status(
        provider: impl Into<String>,
        message: impl Into<String>,
        status: u16,
    ) -> Self {
        VygrError::Provider {
            provider: provider.into(),
            message: message.into(),
            status: Some(status),
        }
    }

    /// Whether a retry after a short backoff has a realistic chance of
    /// succeeding (429, 5xx, or connection-level failures). Anti-bot
    /// challenges like DDG's 202 are deliberately excluded: spacing, not
    /// retrying, is what fixes them (ADR-0010).
    pub fn is_retriable(&self) -> bool {
        match self {
            VygrError::Provider {
                status: Some(s), ..
            } => *s == 429 || (500..=599).contains(s),
            VygrError::Network(_) => true,
            _ => false,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            VygrError::Config(_) | VygrError::Auth(_) => 3,
            VygrError::Provider { .. } | VygrError::Network(_) | VygrError::Parse(_) => 4,
            VygrError::Io(_) | VygrError::Json(_) | VygrError::NotImplemented(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retriable_classification() {
        assert!(VygrError::provider_status("x", "rate", 429).is_retriable());
        assert!(VygrError::provider_status("x", "boom", 503).is_retriable());
        assert!(!VygrError::provider_status("x", "nope", 404).is_retriable());
        // DDG anti-bot: spacing fixes it, not retrying.
        assert!(!VygrError::provider_status("x", "challenge", 202).is_retriable());
        assert!(VygrError::Network("conn reset".into()).is_retriable());
        assert!(!VygrError::Config("no key".into()).is_retriable());
        assert!(!VygrError::provider("x", "empty").is_retriable());
    }

    #[test]
    fn display_keeps_provider_prefix() {
        let e = VygrError::provider_status("ddgs", "challenge", 202);
        assert_eq!(e.to_string(), "provider ddgs: challenge");
    }
}
