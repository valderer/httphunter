use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid proxy request: {0}")]
    InvalidProxyRequest(String),

    #[error("upstream connection failed: {0}")]
    UpstreamConnection(#[source] std::io::Error),

    #[error("request timed out")]
    Timeout,
}
