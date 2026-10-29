mod support;

use std::time::Duration;

use hozd::client::{
    CallEvent, GrpcClient, GrpcOutcome, GrpcRequest, Header, Schema, SchemaSource, TonicGrpc,
    load_schema_file,
};
use support::{Options, Reflection, TestServer, fixtures};

fn schema() -> Schema {
    load_schema_file(&fixtures().join("descriptor.bin"), &[]).unwrap()
}

fn request(server: &TestServer, method: &str, body: &str) -> GrpcRequest {
    GrpcRequest {
        address: server.url(),
        method: format!("demo.v1.Demo/{method}"),
        schema: SchemaSource::Reflection,
        body: Some(body.to_owned()),
        ..Default::default()
    }
}

/// Plays a call to its end, returning every event and the final outcome.
async fn run(request: GrpcRequest) -> (Vec<CallEvent>, GrpcOutcome) {
    let client = TonicGrpc::new(Vec::new());
    let mut call = client.call(request, schema());
    let mut events = Vec::new();
    while let Some(event) = call.events.recv().await {
        let done = matches!(event, CallEvent::Done(_));
        events.push(event);
        if done {
            break;
        }
    }
    let Some(CallEvent::Done(outcome)) = events.last().cloned() else {
        panic!("the call never finished: {events:?}");
    };
    (events, outcome)
}

fn failed_before_sending(outcome: &GrpcOutcome) -> &str {
    assert!(outcome.status.is_none(), "{outcome:?}");
    outcome.error.as_deref().expect("an error message")
}

fn json(message: &str) -> serde_json::Value {
    serde_json::from_str(message).unwrap()
}

// ---- call kinds ------------------------------------------------------------

#[tokio::test]
async fn unary_call_returns_one_message_and_ok() {
    let server = TestServer::start(Options::default()).await;
    let (events, outcome) = run(request(&server, "GetUser", r#"{"id": 3}"#)).await;

    let first_data = events.iter().find(|e| !matches!(e, CallEvent::Log(_)));
    assert!(matches!(first_data, Some(CallEvent::Headers(_))));
    let trace: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            CallEvent::Log(l) => Some(l.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        trace
            .iter()
            .any(|l| l.starts_with("> POST /") && l.ends_with("/GetUser HTTP/2")),
        "{trace:?}"
    );
    assert!(trace.contains(&"< grpc-status: 0 (OK)"), "{trace:?}");
    assert_eq!(outcome.messages.len(), 1);
    let user = json(&outcome.messages[0]);
    assert_eq!(
        (user["id"].as_str(), user["name"].as_str()),
        (Some("3"), Some("user-3"))
    );
    let status = outcome.status.unwrap();
    assert_eq!((status.code, status.name()), (0, "OK"));
    assert!(outcome.elapsed > Duration::ZERO);
    assert!(!outcome.response_stream);
}

#[tokio::test]
async fn server_streaming_delivers_every_message_in_order() {
    let server = TestServer::start(Options::default()).await;
    let (_, outcome) = run(request(
        &server,
        "ListUsers",
        r#"{"name": "u", "pageSize": 3}"#,
    ))
    .await;
    let names: Vec<_> = outcome
        .messages
        .iter()
        .map(|m| json(m)["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["u1", "u2", "u3"]);
    assert_eq!(outcome.status.unwrap().code, 0);
    assert!(outcome.response_stream);
}

#[tokio::test]
async fn client_streaming_sends_each_array_element_and_closes() {
    let server = TestServer::start(Options::default()).await;
    let (_, outcome) = run(request(
        &server,
        "Collect",
        r#"[{"id":1},{"id":2},{"id":3}]"#,
    ))
    .await;
    assert_eq!(json(&outcome.messages[0])["count"], 3);
    assert_eq!(outcome.status.unwrap().code, 0);
}

#[tokio::test]
async fn bidirectional_streaming_echoes_each_message() {
    let server = TestServer::start(Options::default()).await;
    let (_, outcome) = run(request(&server, "Chat", r#"[{"text":"a"},{"text":"b"}]"#)).await;
    let texts: Vec<_> = outcome
        .messages
        .iter()
        .map(|m| json(m)["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(texts, ["echo: a", "echo: b"]);
}

#[tokio::test]
async fn blank_body_sends_one_empty_message() {
    let server = TestServer::start(Options::default()).await;
    let mut req = request(&server, "GetUser", "");
    req.body = None;
    let (_, outcome) = run(req).await;
    assert_eq!(json(&outcome.messages[0])["name"], "user-0");
}

#[tokio::test]
async fn responses_show_default_fields() {
    let server = TestServer::start(Options::default()).await;
    let (_, outcome) = run(request(&server, "GetUser", r#"{"id": 0}"#)).await;
    let user = json(&outcome.messages[0]);
    assert_eq!(user["id"], "0");
    assert!(user.get("role").is_some(), "{user}");
}

// ---- status ----------------------------------------------------------------

#[tokio::test]
async fn error_status_is_a_normal_result_with_code_and_message() {
    let server = TestServer::start(Options::default()).await;
    let (_, outcome) = run(request(&server, "GetUser", r#"{"id": 7}"#)).await;
    let status = outcome.status.expect("a status");
    assert_eq!(status.code, 5);
    assert_eq!(status.name(), "NOT_FOUND");
    assert_eq!(status.message, "user 7 not found");
    assert!(outcome.error.is_none());
    assert!(outcome.messages.is_empty());
}

// ---- nothing is sent when the request is wrong -------------------------------

#[tokio::test]
async fn invalid_bodies_send_nothing_and_say_what_is_wrong() {
    let server = TestServer::start(Options::default()).await;
    for (method, body, needle) in [
        ("GetUser", "{not json", "not valid JSON"),
        ("GetUser", r#"{"nope": 1}"#, "nope"),
        ("GetUser", r#"{"id": "abc"}"#, "field `id`"),
        ("GetUser", r#"[{"id": 1}]"#, "one JSON object"),
        ("ListUsers", "[]", "one JSON object"),
        ("Collect", r#"{"id": 1}"#, "JSON array"),
        ("Chat", r#"[{"text": "a"}, {"oops": 1}]"#, "message 2"),
    ] {
        let (_, outcome) = run(request(&server, method, body)).await;
        let error = failed_before_sending(&outcome).to_owned();
        assert!(error.contains(needle), "{method} {body}: {error}");
    }
    assert_eq!(server.calls(), 0, "nothing may reach the server");
}

#[tokio::test]
async fn unknown_method_sends_nothing() {
    let server = TestServer::start(Options::default()).await;
    let (_, outcome) = run(request(&server, "Nope", "{}")).await;
    assert!(failed_before_sending(&outcome).contains("demo.v1.Demo/Nope"));
    assert_eq!(server.calls(), 0);
}

#[tokio::test]
async fn invalid_metadata_names_the_entry_and_sends_nothing() {
    let server = TestServer::start(Options::default()).await;
    let mut req = request(&server, "GetUser", r#"{"id": 1}"#);
    req.metadata = vec![Header::new("bad name", "x")];
    let (_, outcome) = run(req).await;
    assert!(failed_before_sending(&outcome).contains("bad name"));
    assert_eq!(server.calls(), 0);
}

#[tokio::test]
async fn metadata_reaches_the_server() {
    let server = TestServer::start(Options::default()).await;
    let mut req = request(&server, "GetUser", r#"{"id": 1}"#);
    req.metadata = vec![Header::new("authorization", "Bearer abc")];
    let (_, outcome) = run(req).await;
    assert_eq!(outcome.status.unwrap().code, 0);
    assert!(
        server
            .metadata()
            .contains(&("authorization".to_owned(), "Bearer abc".to_owned())),
        "{:?}",
        server.metadata()
    );
}

// ---- connections -------------------------------------------------------------

#[tokio::test]
async fn refused_connection_is_an_error_not_a_status() {
    let mut server = TestServer::start(Options::default()).await;
    let req = request(&server, "GetUser", r#"{"id": 1}"#);
    server.stop();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (_, outcome) = run(req).await;
    assert!(failed_before_sending(&outcome).contains("cannot connect"));
}

#[tokio::test]
async fn address_without_a_scheme_names_both_schemes() {
    let server = TestServer::start(Options::default()).await;
    let mut req = request(&server, "GetUser", "{}");
    req.address = server.addr.to_string();
    let (_, outcome) = run(req).await;
    let error = failed_before_sending(&outcome);
    assert!(
        error.contains("http://") && error.contains("https://"),
        "{error}"
    );
}

#[tokio::test]
async fn untrusted_certificate_is_refused_unless_insecure() {
    let server = TestServer::start(Options {
        tls: true,
        reflection: Reflection::None,
    })
    .await;
    let (_, outcome) = run(request(&server, "GetUser", r#"{"id": 1}"#)).await;
    let error = failed_before_sending(&outcome);
    assert!(error.contains("not trusted"), "{error}");
    assert!(error.contains("insecure"), "{error}");

    let mut req = request(&server, "GetUser", r#"{"id": 1}"#);
    req.insecure = true;
    let (_, outcome) = run(req).await;
    assert!(outcome.status.is_some(), "{outcome:?}");
    assert_eq!(outcome.status.clone().unwrap().code, 0, "{outcome:?}");
    assert_eq!(json(&outcome.messages[0])["name"], "user-1");
}

// ---- streaming and cancel ------------------------------------------------------

#[tokio::test]
async fn a_message_arrives_before_the_call_completes() {
    let server = TestServer::start(Options::default()).await;
    let client = TonicGrpc::new(Vec::new());
    let mut call = client.call(
        request(
            &server,
            "ListUsers",
            r#"{"name": "u", "pageSize": 3, "delayMs": 300}"#,
        ),
        schema(),
    );
    loop {
        match call.events.recv().await.expect("events") {
            CallEvent::Message(m) => {
                assert_eq!(json(&m)["name"], "u1");
                break;
            }
            CallEvent::Done(o) => panic!("finished before any message: {o:?}"),
            CallEvent::Headers(_) | CallEvent::Log(_) => {}
        }
    }
    assert!(
        call.events.try_recv().is_err(),
        "the call must still be open"
    );
}

#[tokio::test]
async fn cancel_ends_with_cancelled_and_keeps_received_messages() {
    let server = TestServer::start(Options::default()).await;
    let client = TonicGrpc::new(Vec::new());
    let mut call = client.call(
        request(
            &server,
            "ListUsers",
            r#"{"name": "u", "pageSize": 50, "delayMs": 100}"#,
        ),
        schema(),
    );
    let mut seen = 0;
    while seen < 2 {
        if let CallEvent::Message(_) = call.events.recv().await.expect("events") {
            seen += 1;
        }
    }
    call.cancel();

    let outcome = loop {
        match call.events.recv().await.expect("a final event") {
            CallEvent::Done(outcome) => break outcome,
            CallEvent::Message(_) | CallEvent::Headers(_) | CallEvent::Log(_) => {}
        }
    };
    let status = outcome.status.unwrap();
    assert_eq!((status.code, status.name()), (1, "CANCELLED"));
    assert!(outcome.messages.len() >= 2 && outcome.messages.len() < 50);
    assert_eq!(json(&outcome.messages[0])["name"], "u1");
    assert_eq!(json(&outcome.messages[1])["name"], "u2");
}
