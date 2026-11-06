mod support;

use hozd::client::{
    CallEvent, GrpcClient, GrpcOutcome, GrpcRequest, Header, Protocol, Schema, TonicGrpc,
    load_schema_file,
};
use support::fixtures;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn schema() -> Schema {
    load_schema_file(&fixtures().join("descriptor.bin"), &[]).unwrap()
}

fn request(server: &MockServer, rpc: &str, body: &str) -> GrpcRequest {
    GrpcRequest {
        address: server.uri(),
        method: format!("demo.v1.Demo/{rpc}"),
        protocol: Protocol::Connect,
        metadata: vec![Header::new("authorization", "Bearer t")],
        body: Some(body.to_owned()),
        ..Default::default()
    }
}

async fn run(request: GrpcRequest) -> GrpcOutcome {
    let mut call = TonicGrpc::new(Vec::new()).call(request, schema());
    while let Some(event) = call.events.recv().await {
        if let CallEvent::Done(outcome) = event {
            return outcome;
        }
    }
    panic!("the call never finished");
}

fn envelope(flags: u8, payload: &str) -> Vec<u8> {
    let mut out = vec![flags];
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload.as_bytes());
    out
}

#[tokio::test]
async fn unary_posts_json_and_returns_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/demo.v1.Demo/GetUser"))
        .and(header("content-type", "application/json"))
        .and(header("connect-protocol-version", "1"))
        .and(header("authorization", "Bearer t"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(r#"{"id":"3","name":"Ana"}"#, "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let outcome = run(request(&server, "GetUser", r#"{"id": 3}"#)).await;
    let status = outcome.status.clone().expect("a status");
    assert_eq!(status.code, 0, "{outcome:?}");
    let message: serde_json::Value = serde_json::from_str(&outcome.messages[0]).unwrap();
    assert_eq!(message["name"], "Ana");
}

#[tokio::test]
async fn unary_error_body_becomes_the_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404).set_body_raw(
            r#"{"code":"not_found","message":"no user 3"}"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let outcome = run(request(&server, "GetUser", r#"{"id": 3}"#)).await;
    let status = outcome.status.expect("a status");
    assert_eq!((status.code, status.message.as_str()), (5, "no user 3"));
    assert!(outcome.messages.is_empty());
}

#[tokio::test]
async fn server_stream_reads_envelopes_until_end_stream() {
    let server = MockServer::start().await;
    let mut body = envelope(0, r#"{"id":"1","name":"a"}"#);
    body.extend(envelope(0, r#"{"id":"2","name":"b"}"#));
    body.extend(envelope(2, r#"{"metadata":{"x-trace":["t1"]}}"#));
    Mock::given(method("POST"))
        .and(path("/demo.v1.Demo/ListUsers"))
        .and(header("content-type", "application/connect+json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/connect+json"))
        .mount(&server)
        .await;

    let outcome = run(request(&server, "ListUsers", "{}")).await;
    assert_eq!(outcome.messages.len(), 2, "{outcome:?}");
    let status = outcome.status.unwrap();
    assert_eq!(status.code, 0);
    assert!(status.trailers.contains(&Header::new("x-trace", "t1")));
}

#[tokio::test]
async fn server_stream_end_error_is_the_status() {
    let server = MockServer::start().await;
    let mut body = envelope(0, r#"{"id":"1","name":"a"}"#);
    body.extend(envelope(
        2,
        r#"{"error":{"code":"unavailable","message":"gone"}}"#,
    ));
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/connect+json"))
        .mount(&server)
        .await;

    let outcome = run(request(&server, "ListUsers", "{}")).await;
    assert_eq!(outcome.messages.len(), 1);
    assert_eq!(outcome.status.unwrap().code, 14);
}

#[tokio::test]
async fn client_streaming_is_refused_before_sending() {
    let server = MockServer::start().await;
    let outcome = run(request(&server, "Collect", "[]")).await;
    assert!(outcome.status.is_none());
    assert!(
        outcome
            .error
            .unwrap()
            .contains("unary and server streaming")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}
