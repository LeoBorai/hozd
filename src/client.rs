use std::sync::Arc;

use async_trait::async_trait;

pub use auth::{
    Jwt, JwtError, Verification, basic_value, parse_basic, parse_token, secret_bytes, token_value,
};
pub use body::{
    Body, BodyError, BodyKind, Encoded, Field, JsonValue, Part, PartValue, expand_home,
};
pub use common::{split_query, with_query};
pub use fake::FakeClient;
pub use graphql::{
    GraphqlOperation, GraphqlRequest, GraphqlSchema, OperationKind, introspection_request,
};
pub use grpc::{
    CallEvent, CallHandle, CallKind, FakeGrpc, FakeStep, GrpcClient, GrpcError, GrpcOutcome,
    GrpcRequest, GrpcStatus, MethodInfo, Schema, SchemaSource, SharedGrpc, TonicGrpc,
    load_schema_file, reflect, status_name,
};
pub use http::ReqwestClient;
pub use trace::{Trace, trace_certificate, trace_resolve};
pub use types::{Header, HttpRequest, HttpResponse, Kind, Method, Outcome, Request};
pub use ws::{
    SharedWs, TungsteniteWs, WsClient, WsClose, WsDirection, WsEvent, WsMessage, WsOutcome,
    WsRequest, WsSession,
};

pub mod grpc;

mod auth;
mod body;
mod common;
mod fake;
mod graphql;
mod http;
mod trace;
mod types;
mod ws;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ClientError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("request failed: {0}")]
    Transport(String),
    #[error("invalid body: {0}")]
    Body(String),
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

/// Asks the GraphQL endpoint at `url` for its schema by introspection.
pub async fn introspect(
    client: &SharedHttp,
    url: &str,
    headers: &[Header],
) -> Result<GraphqlSchema, String> {
    let response = client
        .send(&introspection_request(url, headers))
        .await
        .map_err(|e| e.to_string())?;
    GraphqlSchema::from_response(&response)
}
