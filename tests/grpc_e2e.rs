//! The whole path through the real clients: start-up wiring, reflection, the picker, a
//! streamed call, cancel, history and replay, against an in-process gRPC server.

mod support;

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hozd::client::{Kind, Outcome, Request};
use hozd::paths::Home;
use hozd::startup;
use hozd::tui::{App, Focus, Msg, Tab};
use ratatui_textarea::TextArea;
use support::{Options, TestServer};
use tokio::sync::mpsc::UnboundedReceiver;

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

async fn wait(
    app: &mut App,
    rx: &mut UnboundedReceiver<Msg>,
    what: &str,
    done: impl Fn(&App) -> bool,
) {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        while !done(app) {
            let msg = rx.recv().await.expect("the app dropped its channel");
            app.update(msg);
        }
    })
    .await;
    assert!(result.is_ok(), "timed out waiting for {what}");
}

fn live_messages(app: &App) -> usize {
    match &app.live {
        Some(Outcome::Grpc(g)) => g.messages.len(),
        _ => 0,
    }
}

#[tokio::test]
async fn reflect_pick_stream_cancel_and_replay_from_history() {
    let server = TestServer::start(Options::default()).await;
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, mut rx) = startup(&Home::at(tmp.path().join(".hozd"))).await.unwrap();

    // gRPC mode, the address, and the schema by reflection.
    app.handle_key(ctrl('g'));
    app.url = TextArea::new(vec![server.url()]);
    app.handle_key(ctrl('l'));
    wait(&mut app, &mut rx, "the schema", |a| a.schema.is_some()).await;
    assert_eq!(app.schema_status(), "1 service, 6 methods");

    // A server streaming method, from the picker; its skeleton becomes the body.
    app.handle_key(ctrl('n'));
    type_text(&mut app, "listusers");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.selected_method.as_deref(),
        Some("demo.v1.Demo/ListUsers")
    );
    app.body = TextArea::new(vec![
        r#"{"name": "u", "pageSize": 3, "delayMs": 20}"#.into(),
    ]);

    // It streams to the end.
    app.handle_key(ctrl('s'));
    assert!(app.sending);
    wait(&mut app, &mut rx, "the call to end", |a| {
        !a.sending && a.history.len() == 1
    })
    .await;
    let Some(Outcome::Grpc(done)) = &app.live else {
        panic!("expected a gRPC outcome");
    };
    assert_eq!(done.messages.len(), 3);
    assert_eq!(done.status.as_ref().unwrap().code, 0);
    assert!(done.response_stream);

    // A long one is cancelled after two messages.
    app.body = TextArea::new(vec![
        r#"{"name": "u", "pageSize": 100, "delayMs": 50}"#.into(),
    ]);
    app.handle_key(ctrl('s'));
    wait(&mut app, &mut rx, "two messages", |a| live_messages(a) >= 2).await;
    assert!(app.sending, "the stream is still open");
    app.handle_key(ctrl('x'));
    wait(
        &mut app,
        &mut rx,
        "the cancelled call to be recorded",
        |a| !a.sending && a.history.len() == 2,
    )
    .await;
    let Some(Outcome::Grpc(cancelled)) = &app.live else {
        panic!("expected a gRPC outcome");
    };
    assert_eq!(cancelled.status.as_ref().unwrap().code, 1);
    assert!(cancelled.messages.len() >= 2 && cancelled.messages.len() < 100);

    // Replay the first call from history: the editor gets all of it back.
    app.handle_key(key(KeyCode::Esc));
    app.set_tab(Tab::History);
    app.focus = Focus::List;
    app.url = TextArea::default();
    app.body = TextArea::default();
    app.selected_method = None;
    // History is newest first: row 1 is the finished call.
    app.handle_key(key(KeyCode::Down));
    wait(&mut app, &mut rx, "the preview", |a| {
        matches!(
            a.preview.as_ref().map(|e| &e.request),
            Some(Request::Grpc(r)) if r.body.as_deref().is_some_and(|b| b.contains("\"pageSize\": 3"))
        )
    })
    .await;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.kind, Kind::Grpc);
    assert_eq!(app.url_text(), server.url());
    assert_eq!(
        app.selected_method.as_deref(),
        Some("demo.v1.Demo/ListUsers")
    );
    assert!(app.body_text().contains("\"pageSize\": 3"));

    app.handle_key(ctrl('s'));
    wait(&mut app, &mut rx, "the replay to be recorded", |a| {
        !a.sending && a.history.len() == 3
    })
    .await;
    let Some(Outcome::Grpc(replayed)) = &app.live else {
        panic!("expected a gRPC outcome");
    };
    assert_eq!(replayed.messages.len(), 3);
    assert_eq!(app.history.len(), 3, "the original entries are untouched");
}

#[tokio::test]
async fn rest_and_grpc_calls_share_one_history() {
    let server = TestServer::start(Options::default()).await;
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, mut rx) = startup(&Home::at(tmp.path().join(".hozd"))).await.unwrap();

    // An HTTP call to a closed port fails, but is recorded all the same.
    app.url = TextArea::new(vec!["http://127.0.0.1:1".into()]);
    app.handle_key(ctrl('s'));
    wait(&mut app, &mut rx, "the HTTP entry", |a| {
        !a.sending && a.history.len() == 1
    })
    .await;

    app.handle_key(ctrl('g'));
    app.url = TextArea::new(vec![server.url()]);
    app.selected_method = Some("demo.v1.Demo/GetUser".into());
    app.body = TextArea::new(vec![r#"{"id": 7}"#.into()]);
    app.handle_key(ctrl('s')); // loads the schema first, then calls
    wait(&mut app, &mut rx, "the gRPC entry", |a| {
        !a.sending && a.history.len() == 2
    })
    .await;

    let kinds: Vec<_> = app.history.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, [Kind::Grpc, Kind::Http]);
    assert_eq!(app.history[0].status, Some(5), "NOT_FOUND");
    assert_eq!(app.history[1].status, None);
}
