//! WebSocket sessions against an in-process echo server: connect, send, receive, close,
//! and the TUI flow with history replay.

use std::net::SocketAddr;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures_util::{SinkExt, StreamExt};
use hozd::client::{
    Kind, Outcome, Request, TungsteniteWs, WsClient, WsDirection, WsEvent, WsMessage, WsOutcome,
    WsRequest,
};
use hozd::paths::Home;
use hozd::startup;
use hozd::tui::{App, Msg};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;

/// Accepts connections and answers every text message with `echo: <text>`; sends `hello`
/// first when `greet` is set. A message `close` makes the server close the socket.
async fn server(greet: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                if greet {
                    let _ = ws.send(Message::text("hello")).await;
                }
                while let Some(Ok(frame)) = ws.next().await {
                    match frame {
                        Message::Text(t) if t.as_str() == "close" => {
                            let _ = ws.close(None).await;
                        }
                        Message::Text(t) => {
                            let _ = ws.send(Message::text(format!("echo: {t}"))).await;
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            });
        }
    });
    addr
}

async fn next_event(session: &mut hozd::client::WsSession) -> WsEvent {
    tokio::time::timeout(Duration::from_secs(10), session.events.recv())
        .await
        .expect("timed out waiting for an event")
        .expect("the session ended without Done")
}

async fn until_done(session: &mut hozd::client::WsSession) -> WsOutcome {
    loop {
        if let WsEvent::Done(outcome) = next_event(session).await {
            return outcome;
        }
    }
}

#[tokio::test]
async fn sends_and_receives_then_closes_on_request() {
    let addr = server(true).await;
    let mut session = TungsteniteWs::new().connect(WsRequest {
        url: format!("ws://{addr}"),
        headers: vec![],
        body: Some("first".into()),
    });

    let mut seen = Vec::new();
    while seen.len() < 3 {
        match next_event(&mut session).await {
            WsEvent::Message(m) => seen.push(m),
            WsEvent::Done(o) => panic!("ended early: {o:?}"),
            _ => {}
        }
    }
    assert!(seen.contains(&WsMessage::sent("first")));
    assert!(seen.contains(&WsMessage::received("hello")));
    assert!(seen.contains(&WsMessage::received("echo: first")));

    assert!(session.send("second"));
    loop {
        if let WsEvent::Message(m) = next_event(&mut session).await
            && m == WsMessage::received("echo: second")
        {
            break;
        }
    }

    session.close();
    let outcome = until_done(&mut session).await;
    assert!(outcome.connected);
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.close.as_ref().map(|c| c.code), Some(1000));
    let sent: Vec<_> = outcome
        .messages
        .iter()
        .filter(|m| m.direction == WsDirection::Sent)
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(sent, ["first", "second"]);
}

#[tokio::test]
async fn server_close_ends_the_session() {
    let addr = server(false).await;
    let mut session = TungsteniteWs::new().connect(WsRequest {
        url: format!("ws://{addr}"),
        headers: vec![],
        body: Some("close".into()),
    });
    let outcome = until_done(&mut session).await;
    assert!(outcome.connected);
    assert!(outcome.close.is_some());
    assert_eq!(outcome.error, None);
}

#[tokio::test]
async fn refused_connection_is_reported_not_raised() {
    let addr = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    let mut session = TungsteniteWs::new().connect(WsRequest {
        url: format!("ws://{addr}"),
        ..Default::default()
    });
    let outcome = until_done(&mut session).await;
    assert!(!outcome.connected);
    assert!(outcome.error.is_some());
}

#[tokio::test]
async fn invalid_url_never_connects() {
    let mut session = TungsteniteWs::new().connect(WsRequest {
        url: "http://nope".into(),
        ..Default::default()
    });
    let outcome = until_done(&mut session).await;
    assert!(outcome.error.unwrap().contains("ws://"));
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
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

fn live_ws(app: &App) -> Option<&WsOutcome> {
    match &app.live {
        Some(Outcome::Ws(w)) => Some(w),
        _ => None,
    }
}

#[tokio::test]
async fn tui_connects_sends_closes_and_replays_from_history() {
    let addr = server(false).await;
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, mut rx) = startup(&Home::at(tmp.path().join(".hozd"))).await.unwrap();

    // REST -> gRPC -> Connect -> WebSocket.
    for _ in 0..3 {
        app.handle_key(ctrl('g'));
    }
    assert_eq!(app.kind, Kind::Ws);
    app.load_request(&Request::Ws(WsRequest {
        url: format!("ws://{addr}"),
        headers: vec![],
        body: Some("ping".into()),
    }));
    assert_eq!(app.kind, Kind::Ws);

    app.handle_key(ctrl('s'));
    wait(&mut app, &mut rx, "echo of the first body", |a| {
        live_ws(a).is_some_and(|w| w.messages.contains(&WsMessage::received("echo: ping")))
    })
    .await;
    assert!(app.ws_open());

    // A second Ctrl+S sends the body again over the open socket.
    app.handle_key(ctrl('s'));
    wait(&mut app, &mut rx, "echo of the second send", |a| {
        live_ws(a).is_some_and(|w| {
            w.messages
                .iter()
                .filter(|m| **m == WsMessage::received("echo: ping"))
                .count()
                == 2
        })
    })
    .await;

    app.handle_key(ctrl('x'));
    wait(&mut app, &mut rx, "the session to end", |a| {
        !a.ws_open() && !a.history.is_empty()
    })
    .await;
    let outcome = live_ws(&app).unwrap();
    assert!(outcome.connected);
    assert!(outcome.close.is_some());

    // The finished session is in history, transcript and all.
    let entry = {
        let id = app.history[0].id;
        assert_eq!(app.history[0].kind, Kind::Ws);
        let store = hozd::history::History::open(&Home::at(tmp.path().join(".hozd")).db_path())
            .await
            .unwrap();
        store.get(id).await.unwrap().unwrap()
    };
    let Outcome::Ws(stored) = entry.outcome else {
        panic!("expected a WebSocket outcome");
    };
    assert_eq!(stored.messages, outcome.messages);
    assert!(matches!(entry.request, Request::Ws(_)));
}
