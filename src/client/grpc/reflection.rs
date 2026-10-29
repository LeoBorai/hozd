//! Fetching a schema from a server through the gRPC reflection protocol.
//!
//! Servers implement either `grpc.reflection.v1` or the older `v1alpha`; both have the same
//! shape, so the exchange is written once and instantiated for each.

use std::collections::{BTreeMap, BTreeSet};

use prost::Message;
use prost_reflect::DescriptorPool;
use prost_types::{FileDescriptorProto, FileDescriptorSet};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Code, Status};

use super::{GrpcError, Schema};

/// Services of the reflection protocol itself are not part of the server's API.
const REFLECTION_PREFIX: &str = "grpc.reflection.";

/// Loads the schema served by the server behind `channel`: `v1` first, then `v1alpha` if the
/// server does not implement `v1`.
pub async fn reflect(channel: Channel) -> Result<Schema, GrpcError> {
    let files = match fetch_v1(channel.clone()).await {
        Ok(files) => files,
        Err(status) if status.code() == Code::Unimplemented => match fetch_v1alpha(channel).await {
            Ok(files) => files,
            Err(status) if status.code() == Code::Unimplemented => {
                return Err(GrpcError::Schema(
                    "the server does not support reflection; load a schema file instead".into(),
                ));
            }
            Err(status) => return Err(failed(&status)),
        },
        Err(status) => return Err(failed(&status)),
    };
    build(files)
}

fn failed(status: &Status) -> GrpcError {
    GrpcError::Schema(format!(
        "reflection failed: {} ({})",
        status.message(),
        status.code()
    ))
}

macro_rules! reflection_exchange {
    ($name:ident, $version:ident) => {
        async fn $name(channel: Channel) -> Result<Vec<FileDescriptorProto>, Status> {
            use tonic_reflection::pb::$version::server_reflection_client::ServerReflectionClient;
            use tonic_reflection::pb::$version::server_reflection_request::MessageRequest;
            use tonic_reflection::pb::$version::server_reflection_response::MessageResponse;
            use tonic_reflection::pb::$version::{
                ServerReflectionRequest, ServerReflectionResponse,
            };

            /// Sends one request and waits for its response; servers answer in order.
            async fn ask(
                tx: &mpsc::Sender<ServerReflectionRequest>,
                responses: &mut tonic::Streaming<ServerReflectionResponse>,
                request: MessageRequest,
            ) -> Result<MessageResponse, Status> {
                let request = ServerReflectionRequest {
                    host: String::new(),
                    message_request: Some(request),
                };
                tx.send(request)
                    .await
                    .map_err(|_| Status::unavailable("reflection stream closed"))?;
                let response = responses
                    .message()
                    .await?
                    .ok_or_else(|| Status::unavailable("reflection stream ended early"))?;
                match response.message_response {
                    Some(MessageResponse::ErrorResponse(e)) => {
                        Err(Status::new(Code::from_i32(e.error_code), e.error_message))
                    }
                    Some(other) => Ok(other),
                    None => Err(Status::internal("empty reflection response")),
                }
            }

            fn files_of(response: MessageResponse) -> Result<Vec<FileDescriptorProto>, Status> {
                let MessageResponse::FileDescriptorResponse(r) = response else {
                    return Err(Status::internal("unexpected reflection response"));
                };
                r.file_descriptor_proto
                    .iter()
                    .map(|bytes| {
                        FileDescriptorProto::decode(bytes.as_slice())
                            .map_err(|e| Status::internal(format!("invalid descriptor: {e}")))
                    })
                    .collect()
            }

            let mut client = ServerReflectionClient::new(channel);
            let (tx, rx) = mpsc::channel(1);
            let mut responses = client
                .server_reflection_info(ReceiverStream::new(rx))
                .await?
                .into_inner();

            let MessageResponse::ListServicesResponse(list) = ask(
                &tx,
                &mut responses,
                MessageRequest::ListServices(String::new()),
            )
            .await?
            else {
                return Err(Status::internal("unexpected reflection response"));
            };

            let mut files: BTreeMap<String, FileDescriptorProto> = BTreeMap::new();
            for service in list.service {
                if service.name.starts_with(REFLECTION_PREFIX) {
                    continue;
                }
                let response = ask(
                    &tx,
                    &mut responses,
                    MessageRequest::FileContainingSymbol(service.name),
                )
                .await?;
                for file in files_of(response)? {
                    files.insert(file.name().to_owned(), file);
                }
            }

            // Servers usually send the transitive closure; ask for whatever is still missing.
            loop {
                let missing: BTreeSet<String> = files
                    .values()
                    .flat_map(|f| f.dependency.iter().cloned())
                    .filter(|dep| !files.contains_key(dep))
                    .collect();
                if missing.is_empty() {
                    break;
                }
                for name in missing {
                    let response =
                        ask(&tx, &mut responses, MessageRequest::FileByFilename(name)).await?;
                    for file in files_of(response)? {
                        files.insert(file.name().to_owned(), file);
                    }
                }
            }
            Ok(files.into_values().collect())
        }
    };
}

reflection_exchange!(fetch_v1, v1);
reflection_exchange!(fetch_v1alpha, v1alpha);

/// Builds the pool; a file must come after the files it imports.
fn build(files: Vec<FileDescriptorProto>) -> Result<Schema, GrpcError> {
    let by_name: BTreeMap<&str, &FileDescriptorProto> =
        files.iter().map(|f| (f.name(), f)).collect();
    let mut ordered = Vec::with_capacity(files.len());
    let mut seen = BTreeSet::new();
    fn visit<'a>(
        file: &'a FileDescriptorProto,
        by_name: &BTreeMap<&str, &'a FileDescriptorProto>,
        seen: &mut BTreeSet<&'a str>,
        ordered: &mut Vec<FileDescriptorProto>,
    ) {
        if !seen.insert(file.name()) {
            return;
        }
        for dep in &file.dependency {
            if let Some(dep) = by_name.get(dep.as_str()) {
                visit(dep, by_name, seen, ordered);
            }
        }
        ordered.push(file.clone());
    }
    for file in by_name.values() {
        visit(file, &by_name, &mut seen, &mut ordered);
    }

    let pool = DescriptorPool::from_file_descriptor_set(FileDescriptorSet { file: ordered })
        .map_err(|e| GrpcError::Schema(format!("the server sent an invalid schema: {e}")))?;
    Ok(Schema::from_pool(pool))
}
