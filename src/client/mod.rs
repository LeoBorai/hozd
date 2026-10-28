mod fake;
mod http;
mod types;

use std::sync::Arc;

use async_trait::async_trait;

pub use fake::FakeClient;
pub use http::HttpClient;
pub use types::{Header, Method, Request, Response};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ClientError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("request failed: {0}")]
    Transport(String),
}

/// Executes a request and reports the outcome. HTTP is the first implementation;
/// callers hold an `Arc<dyn Client>` so other kinds can be added later.
#[async_trait]
pub trait Client: Send + Sync {
    async fn send(&self, request: &Request) -> Result<Response, ClientError>;
}

pub type SharedClient = Arc<dyn Client>;
