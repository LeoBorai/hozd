//! GraphQL against a mock endpoint: introspection, the picker's skeleton, sending, history.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hozd::client::{FakeGrpc, Header, Kind, Request, ReqwestClient, SharedHttp, introspect};
use hozd::history::{History, Outcome};
use hozd::project::Loaded;
use hozd::tui::{App, Focus, Msg};
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use wiremock::matchers::{body_partial_json, body_string_contains, header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn schema_reply() -> serde_json::Value {
    let scalar = |name: &str| json!({"kind": "SCALAR", "name": name, "ofType": null});
    json!({"data": {"__schema": {
        "queryType": {"name": "Query"},
        "mutationType": null,
        "subscriptionType": null,
        "types": [
            {"kind": "OBJECT", "name": "Query", "fields": [
                {"name": "hello", "args": [
                    {"name": "name", "type": {"kind": "NON_NULL", "name": null, "ofType": scalar("String")}}
                ], "type": scalar("String")}
            ]},
            {"kind": "SCALAR", "name": "String"}
        ]
    }}})
}

async fn endpoint() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("IntrospectionQuery"))
        .and(header("authorization", "Bearer t"))
        .respond_with(ResponseTemplate::new(200).set_body_json(schema_reply()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({
            "query": "query Hello($name: String!) {\n  hello(name: $name)\n}",
            "variables": {"name": "ada"}
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"hello": "hi ada"}})),
        )
        .mount(&server)
        .await;
    server
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

async fn pump(app: &mut App, rx: &mut UnboundedReceiver<Msg>, done: impl Fn(&App) -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !done(app) {
            app.update(rx.recv().await.expect("channel open"));
        }
    })
    .await
    .expect("timed out waiting for app state");
}

#[tokio::test]
async fn introspect_reads_the_operations() {
    let server = endpoint().await;
    let client: SharedHttp = Arc::new(ReqwestClient::new());
    let schema = introspect(
        &client,
        &server.uri(),
        &[Header::new("Authorization", "Bearer t")],
    )
    .await
    .unwrap();
    let names: Vec<_> = schema.operations().iter().map(|o| o.full_name()).collect();
    assert_eq!(names, ["Query/hello"]);

    // without the header the mock does not match and the endpoint answers 404
    let err = introspect(&client, &server.uri(), &[]).await.unwrap_err();
    assert!(err.contains("HTTP 404"), "{err}");
}

#[tokio::test]
async fn introspect_pick_send_and_replay_from_history() {
    let server = endpoint().await;
    let tmp = tempfile::tempdir().unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let (tx, mut rx) = unbounded_channel();
    let mut app = App::new(
        Arc::new(ReqwestClient::new()),
        Arc::new(FakeGrpc::default()),
        store.clone(),
        Loaded::default(),
        tx,
    );

    // REST -> gRPC -> Connect -> WS -> GraphQL
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, &server.uri());
    for _ in 0..4 {
        app.handle_key(ctrl('g'));
    }
    assert_eq!(app.kind, Kind::Graphql);

    // headers first: introspection sends them too
    app.handle_key(ctrl('t'));
    app.handle_key(ctrl('t'));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Headers);
    type_text(&mut app, "Authorization");
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "Bearer t");

    app.handle_key(ctrl('l'));
    pump(&mut app, &mut rx, |a| a.current_graphql().is_some()).await;

    app.handle_key(ctrl('p'));
    assert!(app.picker.is_some());
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.selected_method.as_deref(), Some("Query/hello"));
    assert_eq!(
        app.body_text(),
        "query Hello($name: String!) {\n  hello(name: $name)\n}"
    );
    assert_eq!(app.variables_text(), "{\n  \"name\": \"\"\n}");

    // fill the variable in
    app.variables = ratatui_textarea::TextArea::new(vec!["{\"name\": \"ada\"}".into()]);
    app.handle_key(ctrl('s'));
    pump(&mut app, &mut rx, |a| {
        a.live.is_some() && !a.history.is_empty()
    })
    .await;

    let Some(Outcome::Response(res)) = &app.live else {
        panic!("expected a response, got {:?}", app.live);
    };
    assert_eq!(res.status, 200);
    assert!(res.body.contains("hi ada"), "{}", res.body);

    let entry = store.get(app.history[0].id).await.unwrap().unwrap();
    let Request::Graphql(req) = entry.request else {
        panic!("expected a GraphQL entry");
    };
    assert_eq!(req.url, server.uri());
    assert!(req.query.starts_with("query Hello("));
    assert_eq!(req.variables.as_deref(), Some("{\"name\": \"ada\"}"));
    assert_eq!(app.history[0].kind, Kind::Graphql);
}

#[tokio::test]
async fn invalid_variables_are_refused_before_sending() {
    let tmp = tempfile::tempdir().unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let (tx, _rx) = unbounded_channel();
    let mut app = App::new(
        Arc::new(ReqwestClient::new()),
        Arc::new(FakeGrpc::default()),
        store,
        Loaded::default(),
        tx,
    );
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "http://127.0.0.1:9/graphql");
    for _ in 0..4 {
        app.handle_key(ctrl('g'));
    }
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Body);
    type_text(&mut app, "{ a }");
    app.variables = ratatui_textarea::TextArea::new(vec!["{nope".into()]);
    app.handle_key(ctrl('s'));
    assert!(!app.sending);
    assert!(
        app.status
            .as_deref()
            .unwrap_or("")
            .contains("not valid JSON")
    );
}

#[tokio::test]
async fn graphql_layout_shows_endpoint_operation_and_query_tabs() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let tmp = tempfile::tempdir().unwrap();
    let store = History::open(&tmp.path().join("db.sqlite3")).await.unwrap();
    let (tx, _rx) = unbounded_channel();
    let mut app = App::new(
        Arc::new(ReqwestClient::new()),
        Arc::new(FakeGrpc::default()),
        store,
        Loaded::default(),
        tx,
    );
    for _ in 0..4 {
        app.handle_key(ctrl('g'));
    }
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| hozd::tui::ui::draw(f, &app)).unwrap();
    let out: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    for expected in [
        "Endpoint",
        "Operation",
        "Query",
        "Variables",
        "not introspected",
    ] {
        assert!(out.contains(expected), "missing `{expected}`");
    }
}
