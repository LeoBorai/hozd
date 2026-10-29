mod fake;
pub mod grpc;
mod http;
mod trace;
mod types;

use std::sync::Arc;

use async_trait::async_trait;

pub use fake::FakeClient;
pub use grpc::{
    CallEvent, CallHandle, CallKind, FakeGrpc, FakeStep, GrpcClient, GrpcError, GrpcOutcome,
    GrpcRequest, GrpcStatus, MethodInfo, Schema, SchemaSource, SharedGrpc, TonicGrpc,
    load_schema_file, reflect, status_name,
};
pub use http::ReqwestClient;
pub use trace::{Trace, trace_certificate, trace_resolve};
pub use types::{Header, HttpRequest, HttpResponse, Kind, Method, Outcome, Request};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ClientError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("request failed: {0}")]
    Transport(String),
}

/// Executes an HTTP request and reports the outcome. Callers hold an `Arc<dyn HttpClient>`.
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, ClientError>;

    /// Like `send`, and reports what happens on the way (see [`Trace`]). Clients that have
    /// nothing to report just send.
    async fn send_traced(
        &self,
        request: &HttpRequest,
        _trace: &Trace,
    ) -> Result<HttpResponse, ClientError> {
        self.send(request).await
    }
}

pub type SharedHttp = Arc<dyn HttpClient>;
