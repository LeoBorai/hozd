mod codec;
mod connect;
mod fake;
mod load;
mod reflection;
mod schema;
mod tonic_client;
mod types;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use super::Header;

pub use codec::{DynamicCodec, message_to_json, parse_body};
pub use connect::connect;
pub use fake::{FakeGrpc, FakeStep};
pub use load::load_schema_file;
pub use reflection::reflect;
pub use schema::{CallKind, MethodInfo, Schema};
pub use tonic_client::TonicGrpc;
pub use types::{GrpcOutcome, GrpcRequest, GrpcStatus, SchemaSource, status_name};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum GrpcError {
    /// The schema could not be loaded (reflection, descriptor file or proto compilation).
    #[error("{0}")]
    Schema(String),
    /// The request is not valid; nothing was sent.
    #[error("{0}")]
    Invalid(String),
    /// No connection could be made.
    #[error("{0}")]
    Connect(String),
}

/// Something that happened during a call, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEvent {
    /// The response metadata arrived.
    Headers(Vec<Header>),
    /// A line of the connection and wire trace (see [`Trace`](super::Trace)).
    Log(String),
    /// One response message, as JSON.
    Message(String),
    /// The call ended; always the last event.
    Done(GrpcOutcome),
}

/// A call in flight: read `events` until [`CallEvent::Done`], or [`cancel`](Self::cancel) it.
pub struct CallHandle {
    pub events: UnboundedReceiver<CallEvent>,
    cancel: CancellationToken,
}

impl CallHandle {
    pub fn new(events: UnboundedReceiver<CallEvent>, cancel: CancellationToken) -> Self {
        Self { events, cancel }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// A token that cancels this call; clone it to cancel from elsewhere.
    pub fn canceller(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Executes gRPC calls. Callers hold an `Arc<dyn GrpcClient>`.
#[async_trait]
pub trait GrpcClient: Send + Sync {
    /// Fetches the schema of the server at `address` from `source`.
    async fn load_schema(
        &self,
        address: &str,
        source: &SchemaSource,
        insecure: bool,
    ) -> Result<Schema, GrpcError>;

    /// Starts `request` against `schema`. Every problem is reported as the final
    /// [`CallEvent::Done`]; this never fails to start.
    fn call(&self, request: GrpcRequest, schema: Schema) -> CallHandle;
}

pub type SharedGrpc = Arc<dyn GrpcClient>;
