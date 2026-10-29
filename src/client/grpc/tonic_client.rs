use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use base64::Engine;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use tonic::client::Grpc;
use tonic::metadata::{AsciiMetadataKey, AsciiMetadataValue, KeyAndValueRef, MetadataMap};
use tonic::transport::Channel;
use tonic::{Code, Request, Status};

use super::{
    CallEvent, CallHandle, CallKind, DynamicCodec, GrpcClient, GrpcError, GrpcOutcome, GrpcRequest,
    GrpcStatus, Schema, SchemaSource, connect, load_schema_file, message_to_json, parse_body,
    reflect,
};
use crate::client::{Header, Trace, trace_resolve};

/// The real gRPC client: dynamic calls over `tonic`.
///
/// Channels are cached per `(address, insecure)`; a `Channel` reconnects by itself, so a
/// cached one stays usable after the server restarts.
#[derive(Clone)]
pub struct TonicGrpc {
    inner: Arc<Inner>,
}

struct Inner {
    channels: Mutex<HashMap<(String, bool), Channel>>,
    proto_include: Vec<PathBuf>,
}

impl TonicGrpc {
    /// `proto_include` are the extra import roots for `.proto` schema files.
    pub fn new(proto_include: Vec<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                channels: Mutex::new(HashMap::new()),
                proto_include,
            }),
        }
    }
}

impl Inner {
    fn is_cached(&self, address: &str, insecure: bool) -> bool {
        let key = (address.trim().to_owned(), insecure);
        self.channels.lock().unwrap().contains_key(&key)
    }

    async fn channel(&self, address: &str, insecure: bool) -> Result<Channel, GrpcError> {
        let key = (address.trim().to_owned(), insecure);
        if let Some(channel) = self.channels.lock().unwrap().get(&key) {
            return Ok(channel.clone());
        }
        let channel = connect(address, insecure).await?;
        self.channels.lock().unwrap().insert(key, channel.clone());
        Ok(channel)
    }
}

#[async_trait]
impl GrpcClient for TonicGrpc {
    async fn load_schema(
        &self,
        address: &str,
        source: &SchemaSource,
        insecure: bool,
    ) -> Result<Schema, GrpcError> {
        match source {
            SchemaSource::Reflection => reflect(self.inner.channel(address, insecure).await?).await,
            SchemaSource::File(path) => {
                let (path, include) = (path.clone(), self.inner.proto_include.clone());
                tokio::task::spawn_blocking(move || load_schema_file(&path, &include))
                    .await
                    .map_err(|e| GrpcError::Schema(format!("schema loading failed: {e}")))?
            }
        }
    }

    fn call(&self, request: GrpcRequest, schema: Schema) -> CallHandle {
        let (tx, rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let mut outcome = GrpcOutcome::default();
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    outcome.status = Some(GrpcStatus {
                        code: Code::Cancelled as i32,
                        message: "cancelled by the user".into(),
                        trailers: Vec::new(),
                    });
                }
                result = drive(&inner, &request, &schema, &tx, &mut outcome) => {
                    if let Err(error) = result {
                        outcome.status = None;
                        outcome.error = Some(error.to_string());
                    }
                }
            }
            outcome.elapsed = started.elapsed();
            let _ = tx.send(CallEvent::Done(outcome));
        });
        CallHandle::new(rx, cancel)
    }
}

/// Runs the call, recording what happens in `outcome` as it happens so that a cancelled
/// call keeps what it received. `Err` means the call never reached the server.
async fn drive(
    inner: &Inner,
    request: &GrpcRequest,
    schema: &Schema,
    tx: &UnboundedSender<CallEvent>,
    outcome: &mut GrpcOutcome,
) -> Result<(), GrpcError> {
    let method = schema.method(&request.method).ok_or_else(|| {
        GrpcError::Invalid(format!(
            "`{}` is not in the loaded schema; load the schema and pick a method",
            request.method
        ))
    })?;
    let kind = CallKind::of(&method);
    outcome.response_stream = kind.streams_response();

    let messages = parse_body(kind, &method.input(), request.body.as_deref())?;
    let metadata = to_metadata(&request.metadata)?;
    let trace = {
        let tx = tx.clone();
        Trace::new(move |line| {
            let _ = tx.send(CallEvent::Log(line));
        })
    };
    if inner.is_cached(&request.address, request.insecure) {
        trace.emit(format!("* Re-using the connection to {}", request.address));
    } else {
        trace_connection(&request.address, request.insecure, &trace).await;
    }
    let channel = inner.channel(&request.address, request.insecure).await?;

    let mut grpc = Grpc::new(channel);
    grpc.ready()
        .await
        .map_err(|e| GrpcError::Connect(format!("cannot connect to {}: {e}", request.address)))?;
    let path = format!("/{}/{}", method.parent_service().full_name(), method.name())
        .parse()
        .map_err(|e| GrpcError::Invalid(format!("invalid method name: {e}")))?;

    trace.emit(format!(
        "> POST /{}/{} HTTP/2",
        method.parent_service().full_name(),
        method.name()
    ));
    trace.emit("> content-type: application/grpc");
    for h in &request.metadata {
        trace.emit(format!("> {}: {}", h.name.trim(), h.value.trim()));
    }
    trace.emit(">");
    trace.emit(format!("* Request messages: {}", messages.len()));

    let mut call = Request::new(tokio_stream::iter(messages));
    *call.metadata_mut() = metadata;
    let response = match grpc
        .streaming(call, path, DynamicCodec::new(method.output()))
        .await
    {
        Ok(response) => response,
        Err(status) => {
            log_status(&trace, &to_status(&status));
            outcome.status = Some(to_status(&status));
            return Ok(());
        }
    };

    outcome.metadata = to_headers(response.metadata());
    trace.emit("< HTTP/2 200");
    for h in &outcome.metadata {
        trace.emit(format!("< {}: {}", h.name, h.value));
    }
    let _ = tx.send(CallEvent::Headers(outcome.metadata.clone()));

    let mut stream = response.into_inner();
    loop {
        match stream.message().await {
            Ok(Some(message)) => {
                let json = message_to_json(&message);
                outcome.messages.push(json.clone());
                let _ = tx.send(CallEvent::Message(json));
            }
            Ok(None) => {
                let trailers = match stream.trailers().await {
                    Ok(Some(map)) => to_headers(&map),
                    _ => Vec::new(),
                };
                let status = GrpcStatus {
                    code: Code::Ok as i32,
                    message: String::new(),
                    trailers,
                };
                log_status(&trace, &status);
                outcome.status = Some(status);
                return Ok(());
            }
            Err(status) => {
                log_status(&trace, &to_status(&status));
                outcome.status = Some(to_status(&status));
                return Ok(());
            }
        }
    }
}

/// The resolution and the kind of connection about to be opened.
async fn trace_connection(address: &str, insecure: bool, trace: &Trace) {
    let address = address.trim();
    let Ok(uri) = address.parse::<tonic::codegen::http::Uri>() else {
        return;
    };
    let tls = uri.scheme_str() == Some("https");
    let (Some(host), port) = (uri.host(), uri.port_u16()) else {
        return;
    };
    let port = port.unwrap_or(if tls { 443 } else { 80 });
    trace_resolve(trace, host, port).await;
    trace.emit(match (tls, insecure) {
        (false, _) => format!("* Connecting to {host}:{port} (plaintext HTTP/2)"),
        (true, false) => format!("* Connecting to {host}:{port} (TLS, certificate verified)"),
        (true, true) => format!("* Connecting to {host}:{port} (TLS, certificate NOT verified)"),
    });
}

/// The trailers and the final status, as they came off the wire.
fn log_status(trace: &Trace, status: &GrpcStatus) {
    for h in &status.trailers {
        trace.emit(format!("< {}: {}", h.name, h.value));
    }
    trace.emit(format!(
        "< grpc-status: {} ({})",
        status.code,
        status.name()
    ));
    if !status.message.is_empty() {
        trace.emit(format!("< grpc-message: {}", status.message));
    }
    trace.emit("<");
}

fn to_status(status: &Status) -> GrpcStatus {
    GrpcStatus {
        code: status.code() as i32,
        message: status.message().to_owned(),
        trailers: to_headers(status.metadata()),
    }
}

fn to_headers(map: &MetadataMap) -> Vec<Header> {
    map.iter()
        .map(|entry| match entry {
            KeyAndValueRef::Ascii(key, value) => Header::new(
                key.as_str(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            ),
            // Binary metadata is shown in the same base64 form it is sent in.
            KeyAndValueRef::Binary(key, value) => Header::new(
                key.as_str(),
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(value.as_encoded_bytes()),
            ),
        })
        .collect()
}

/// Checks every entry, so one bad name or value is reported before anything is sent.
fn to_metadata(headers: &[Header]) -> Result<MetadataMap, GrpcError> {
    let mut map = MetadataMap::new();
    for header in headers {
        let name = header.name.trim();
        let key = AsciiMetadataKey::from_bytes(name.as_bytes())
            .map_err(|e| GrpcError::Invalid(format!("invalid metadata name `{name}`: {e}")))?;
        let value = AsciiMetadataValue::try_from(header.value.trim())
            .map_err(|e| GrpcError::Invalid(format!("invalid metadata value for `{name}`: {e}")))?;
        map.append(key, value);
    }
    Ok(map)
}
