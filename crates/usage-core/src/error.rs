use thiserror::Error;

/// Errors are shown directly in the UI, so messages are written for people
/// and never include credentials.
#[derive(Debug, Error)]
pub enum UsageError {
    #[error("{0}")]
    Setup(String),
    /// The local app that provides the data isn't running right now.
    #[error("{0}")]
    Offline(String),
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
            UsageError::Network(network_cause(&e))
        }
    }
}

/// reqwest's own message ("error sending request") hides why; the innermost
/// error (DNS, refused, reset, certificate, proxy) is what people can act on.
fn network_cause(e: &reqwest::Error) -> String {
    let mut root: &dyn std::error::Error = e;
    while let Some(next) = root.source() {
        root = next;
    }
    let raw = root.to_string();
    let lower = raw.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["certificate", "unknownissuer", "invalid peer"]) {
        "the server's certificate isn't trusted. A proxy, VPN or antivirus may be inspecting HTTPS traffic.".into()
    } else if has(&["tunnel", "proxy"]) {
        format!("the proxy refused the connection ({raw}). Check your proxy or VPN settings.")
    } else if has(&["dns", "lookup address", "no such host", "name or service", "nodename"]) {
        "couldn't find the server. Check your internet connection or VPN.".into()
    } else if has(&["refused"]) {
        "the connection was refused. If you use a proxy or VPN, check that it's running.".into()
    } else if has(&["reset", "forcibly closed", "aborted", "unexpected eof", "closed before"]) {
        "the connection was cut off. The service may be blocked on this network; try a VPN.".into()
    } else if has(&["timed out", "timeout"]) {
        "the server didn't answer in time. Check your internet connection or VPN.".into()
    } else {
        raw.chars().take(160).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refused_connection_names_the_cause() {
        // Nothing listens on port 1, so the connection is refused straight away.
        let err = crate::http_client().get("https://127.0.0.1:1/").send().await.unwrap_err();
        let msg = UsageError::from(err).to_string();
        assert!(msg.contains("refused"), "{msg}");
        assert!(!msg.contains("error sending request"), "{msg}");
    }
}
