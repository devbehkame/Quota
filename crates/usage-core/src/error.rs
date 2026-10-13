use thiserror::Error;

/// Errors are shown directly in the UI, so messages are written for people
/// and never include credentials.
#[derive(Debug, Error)]
pub enum UsageError {
    #[error("{0}")]
    Setup(String),
    #[error("The provider rejected the credentials (HTTP 401). Check that the key is correct and not revoked.")]
    Unauthorized,
    #[error("Access denied (HTTP 403): {0}")]
    Forbidden(String),
    #[error("The provider is rate limiting requests. Try again in a minute.")]
    RateLimited,
    #[error("Provider returned HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("Network error: {0}")]
    Network(String),
    #[error("Unexpected response from provider: {0}")]
    Parse(String),
}

impl From<reqwest::Error> for UsageError {
    fn from(e: reqwest::Error) -> Self {
        let e = e.without_url();
        if e.is_timeout() {
            UsageError::Network("request timed out".into())
        } else if e.is_decode() {
            UsageError::Parse(e.to_string())
        } else {
            UsageError::Network(e.to_string())
        }
    }
}
