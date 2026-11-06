//! WebSocket sessions: connect, send text messages, receive frames until the socket closes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_util::sync::CancellationToken;

use super::Header;
use super::grpc::install_crypto_provider;

/// How long a closing handshake may take before the socket is dropped.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// What the user asked to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WsRequest {
    /// `ws://` or `wss://`.
    pub url: String,
    /// Extra handshake headers.
    #[serde(default)]
    pub headers: Vec<Header>,
    /// A text message sent as soon as the socket opens.
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsDirection {
    Sent,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsMessage {
    pub direction: WsDirection,
    /// The text; binary frames are shown lossily as UTF-8.
    pub text: String,
    #[serde(default)]
    pub binary: bool,
}

impl WsMessage {
    pub fn sent(text: impl Into<String>) -> Self {
        Self {
            direction: WsDirection::Sent,
            text: text.into(),
            binary: false,
        }
    }

    pub fn received(text: impl Into<String>) -> Self {
        Self {
            direction: WsDirection::Received,
            text: text.into(),
            binary: false,
        }
    }
}

/// The close frame the session ended with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsClose {
    pub code: u16,
    pub reason: String,
}

/// The result of a session: what was exchanged, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WsOutcome {
    /// Whether the handshake succeeded.
    pub connected: bool,
    /// The handshake response headers.
    #[serde(default)]
    pub headers: Vec<Header>,
    /// Every message, in the order it was sent or received.
    #[serde(default)]
    pub messages: Vec<WsMessage>,
    #[serde(default)]
    pub close: Option<WsClose>,
    /// Why the session failed or was cut off, if it did.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub elapsed: Duration,
}

impl WsOutcome {
    /// The conversation as text: `->` sent, `<-` received, then how it ended.
    pub fn body_text(&self) -> String {
        let mut lines: Vec<String> = self
            .messages
            .iter()
            .map(|m| {
                let arrow = match m.direction {
                    WsDirection::Sent => "->",
                    WsDirection::Received => "<-",
                };
                format!("{arrow} {}", m.text)
            })
            .collect();
        if let Some(error) = &self.error {
            lines.push(error.clone());
        } else if let Some(close) = &self.close {
            lines.push(close.to_string());
        }
        lines.join("\n")
    }
}

impl std::fmt::Display for WsClose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reason.is_empty() {
            write!(f, "closed ({})", self.code)
        } else {
            write!(f, "closed ({}): {}", self.code, self.reason)
        }
    }
}

/// Something that happened during a session, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsEvent {
    /// A line of the connection trace.
    Log(String),
    /// The handshake completed; carries the response headers.
    Connected(Vec<Header>),
    /// A message was sent or received.
    Message(WsMessage),
    /// The session ended; always the last event.
    Done(WsOutcome),
}

/// A session in flight: read `events` until [`WsEvent::Done`], [`send`](Self::send)
/// messages, or [`close`](Self::close) it.
pub struct WsSession {
    pub events: UnboundedReceiver<WsEvent>,
    outgoing: UnboundedSender<String>,
    cancel: CancellationToken,
}

impl WsSession {
    pub fn new(
        events: UnboundedReceiver<WsEvent>,
        outgoing: UnboundedSender<String>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            events,
            outgoing,
            cancel,
        }
    }

    /// Queues a text message; `false` when the session already ended.
    pub fn send(&self, text: impl Into<String>) -> bool {
        self.outgoing.send(text.into()).is_ok()
    }

    /// A handle that sends messages from elsewhere.
    pub fn sender(&self) -> UnboundedSender<String> {
        self.outgoing.clone()
    }

    pub fn close(&self) {
        self.cancel.cancel();
    }

    /// A token that closes this session; clone it to close from elsewhere.
    pub fn canceller(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Opens WebSocket sessions. Callers hold an `Arc<dyn WsClient>`.
pub trait WsClient: Send + Sync {
    /// Starts `request`. Every problem is reported as the final [`WsEvent::Done`]; this
    /// never fails to start.
    fn connect(&self, request: WsRequest) -> WsSession;
}

pub type SharedWs = Arc<dyn WsClient>;

/// WebSocket client on top of `tokio-tungstenite`.
#[derive(Clone, Default)]
pub struct TungsteniteWs;

impl TungsteniteWs {
    pub fn new() -> Self {
        Self
    }
}

impl WsClient for TungsteniteWs {
    fn connect(&self, request: WsRequest) -> WsSession {
        install_crypto_provider();
        let (events_tx, events_rx) = unbounded_channel();
        let (out_tx, out_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            let outcome = run(request, out_rx, &events_tx, &token).await;
            let _ = events_tx.send(WsEvent::Done(outcome));
        });
        WsSession::new(events_rx, out_tx, cancel)
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn run(
    request: WsRequest,
    mut outgoing: UnboundedReceiver<String>,
    events: &UnboundedSender<WsEvent>,
    cancel: &CancellationToken,
) -> WsOutcome {
    let started = Instant::now();
    let mut outcome = WsOutcome::default();
    let fail = |mut outcome: WsOutcome, message: String| {
        outcome.error = Some(message);
        outcome.elapsed = started.elapsed();
        outcome
    };

    let handshake = match build_request(&request) {
        Ok(handshake) => handshake,
        Err(message) => return fail(outcome, message),
    };
    let _ = events.send(WsEvent::Log(format!("* Connecting to {}", request.url)));
    let connecting = tokio_tungstenite::connect_async(handshake);
    let (mut socket, response) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return fail(outcome, "cancelled by the user".into()),
        connected = connecting => match connected {
            Ok(connected) => connected,
            Err(e) => return fail(outcome, describe(&e)),
        },
    };

    outcome.connected = true;
    outcome.headers = response
        .headers()
        .iter()
        .map(|(k, v)| Header::new(k.as_str(), String::from_utf8_lossy(v.as_bytes())))
        .collect();
    let _ = events.send(WsEvent::Log(format!(
        "< HTTP/1.1 {}",
        response.status().as_u16()
    )));
    let _ = events.send(WsEvent::Connected(outcome.headers.clone()));

    if let Some(body) = request.body.as_deref().filter(|b| !b.is_empty())
        && let Err(message) = send_text(&mut socket, body, &mut outcome, events).await
    {
        outcome.error = Some(message);
        outcome.elapsed = started.elapsed();
        return outcome;
    }

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                close(&mut socket, &mut outcome, events).await;
                break;
            }
            Some(text) = outgoing.recv() => {
                if let Err(message) = send_text(&mut socket, &text, &mut outcome, events).await {
                    outcome.error = Some(message);
                    break;
                }
            }
            frame = socket.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    record(&mut outcome, events, WsMessage::received(text.as_str()));
                }
                Some(Ok(Message::Binary(bytes))) => {
                    let message = WsMessage {
                        binary: true,
                        ..WsMessage::received(String::from_utf8_lossy(&bytes))
                    };
                    record(&mut outcome, events, message);
                }
                Some(Ok(Message::Close(frame))) => {
                    outcome.close = Some(close_of(frame.as_ref()));
                    break;
                }
                // Pings are answered by the library; raw frames are not surfaced.
                Some(Ok(_)) => {}
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) | None => break,
                Some(Err(e)) => {
                    outcome.error = Some(describe(&e));
                    break;
                }
            },
        }
    }
    outcome.elapsed = started.elapsed();
    outcome
}

fn build_request(
    request: &WsRequest,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    let url = request.url.trim();
    if !(url.starts_with("ws://") || url.starts_with("wss://")) {
        return Err(format!(
            "invalid URL: {url}: a WebSocket URL starts with ws:// or wss://"
        ));
    }
    let mut handshake = url
        .into_client_request()
        .map_err(|e| format!("invalid URL: {url}: {e}"))?;
    for h in &request.headers {
        let name = HeaderName::from_bytes(h.name.as_bytes())
            .map_err(|e| format!("invalid header name `{}`: {e}", h.name))?;
        let value = HeaderValue::from_str(&h.value)
            .map_err(|e| format!("invalid value for header `{}`: {e}", h.name))?;
        handshake.headers_mut().append(name, value);
    }
    Ok(handshake)
}

fn record(outcome: &mut WsOutcome, events: &UnboundedSender<WsEvent>, message: WsMessage) {
    outcome.messages.push(message.clone());
    let _ = events.send(WsEvent::Message(message));
}

async fn send_text(
    socket: &mut Socket,
    text: &str,
    outcome: &mut WsOutcome,
    events: &UnboundedSender<WsEvent>,
) -> Result<(), String> {
    socket
        .send(Message::text(text))
        .await
        .map_err(|e| describe(&e))?;
    record(outcome, events, WsMessage::sent(text));
    Ok(())
}

/// Sends a normal close frame and waits briefly for the server's answer.
async fn close(socket: &mut Socket, outcome: &mut WsOutcome, events: &UnboundedSender<WsEvent>) {
    let frame = CloseFrame {
        code: CloseCode::Normal,
        reason: "".into(),
    };
    if socket.close(Some(frame)).await.is_err() {
        return;
    }
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
        while let Some(Ok(frame)) = socket.next().await {
            match frame {
                Message::Close(frame) => {
                    outcome.close = Some(close_of(frame.as_ref()));
                    return;
                }
                Message::Text(text) => {
                    record(outcome, events, WsMessage::received(text.as_str()));
                }
                _ => {}
            }
        }
    })
    .await;
    outcome.close.get_or_insert(WsClose {
        code: 1000,
        reason: "closed by the user".into(),
    });
}

fn close_of(frame: Option<&CloseFrame>) -> WsClose {
    match frame {
        Some(f) => WsClose {
            code: u16::from(f.code),
            reason: f.reason.to_string(),
        },
        None => WsClose {
            code: 1005,
            reason: String::new(),
        },
    }
}

fn describe(err: &WsError) -> String {
    match err {
        WsError::Http(response) => format!(
            "the server refused the upgrade: HTTP {}",
            response.status().as_u16()
        ),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_websocket_urls() {
        let request = WsRequest {
            url: "http://x".into(),
            ..Default::default()
        };
        assert!(build_request(&request).unwrap_err().contains("ws://"));
    }

    #[test]
    fn rejects_invalid_header_names() {
        let request = WsRequest {
            url: "ws://x".into(),
            headers: vec![Header::new("bad name", "v")],
            body: None,
        };
        assert!(
            build_request(&request)
                .unwrap_err()
                .contains("invalid header name")
        );
    }

    #[test]
    fn body_text_marks_direction_and_close() {
        let outcome = WsOutcome {
            connected: true,
            messages: vec![WsMessage::sent("hi"), WsMessage::received("hello")],
            close: Some(WsClose {
                code: 1000,
                reason: "bye".into(),
            }),
            ..Default::default()
        };
        assert_eq!(outcome.body_text(), "-> hi\n<- hello\nclosed (1000): bye");
    }
}
