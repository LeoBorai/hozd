mod support;

use hozd::client::{GrpcError, reflect};
use support::{Options, Reflection, TestServer};
use tonic::transport::{Channel, Endpoint};

async fn channel(server: &TestServer) -> Channel {
    Endpoint::from_shared(server.url())
        .unwrap()
        .connect()
        .await
        .unwrap()
}

fn full_names(schema: &hozd::client::Schema) -> Vec<String> {
    schema.methods().iter().map(|m| m.full_name()).collect()
}

const EXPECTED: [&str; 6] = [
    "demo.v1.Demo/Chat",
    "demo.v1.Demo/Collect",
    "demo.v1.Demo/Create",
    "demo.v1.Demo/GetUser",
    "demo.v1.Demo/ListUsers",
    "demo.v1.Demo/Tree",
];

#[tokio::test]
async fn v1_reflection_lists_the_services_without_the_reflection_service() {
    let server = TestServer::start(Options::default()).await;
    let schema = reflect(channel(&server).await).await.unwrap();
    assert_eq!(full_names(&schema), EXPECTED);
    // Imports (a sibling and a well-known type) came along.
    assert!(schema.pool().get_enum_by_name("demo.v1.Role").is_some());
    assert!(
        schema
            .pool()
            .get_message_by_name("google.protobuf.Timestamp")
            .is_some()
    );
    assert!(
        schema
            .pool()
            .get_service_by_name("grpc.reflection.v1.ServerReflection")
            .is_none()
    );
}

#[tokio::test]
async fn falls_back_to_v1alpha_when_v1_is_not_implemented() {
    let server = TestServer::start(Options {
        reflection: Reflection::V1AlphaOnly,
        ..Options::default()
    })
    .await;
    let schema = reflect(channel(&server).await).await.unwrap();
    assert_eq!(full_names(&schema), EXPECTED);
}

#[tokio::test]
async fn server_without_reflection_suggests_a_schema_file() {
    let server = TestServer::start(Options {
        reflection: Reflection::None,
        ..Options::default()
    })
    .await;
    let err = reflect(channel(&server).await).await.unwrap_err();
    let GrpcError::Schema(message) = err else {
        panic!("expected a schema error");
    };
    assert!(message.contains("does not support reflection"), "{message}");
    assert!(message.contains("schema file"), "{message}");
}
