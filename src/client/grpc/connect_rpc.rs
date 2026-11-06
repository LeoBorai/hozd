//! The Connect protocol (<https://connectrpc.com/docs/protocol>) over plain HTTP, with JSON
//! messages. Unary calls are one POST; server streaming uses enveloped `connect+json`
//! messages. Client and bidirectional streaming need full-duplex HTTP/2 and are refused.

use std::time::Duration;

use prost_reflect::{DynamicMessage, MethodDescriptor};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use super::{
    CallEvent, CallKind, GrpcError, GrpcOutcome, GrpcRequest, GrpcStatus, message_to_json,
};
use crate::client::common::trace_url_resolve;
use crate::client::{Header, Trace};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const END_STREAM: u8 = 0b10;
const COMPRESSED: u8 = 0b01;

/// A reqwest client for Connect calls; `insecure` skips TLS certificate verification.
pub fn http_client(insecure: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .danger_accept_invalid_certs(insecure)
        .build()
        .unwrap_or_default()
}

/// The numeric gRPC code of a Connect error code string.
pub fn code_number(code: &str) -> i32 {
    match code {
        "canceled" => 1,
        "invalid_argument" => 3,
        "deadline_exceeded" => 4,
        "not_found" => 5,
        "already_exists" => 6,
        "permission_denied" => 7,
        "resource_exhausted" => 8,
        "failed_precondition" => 9,
        "aborted" => 10,
        "out_of_range" => 11,
        "unimplemented" => 12,
        "internal" => 13,
        "unavailable" => 14,
        "data_loss" => 15,
        "unauthenticated" => 16,
        _ => 2,
    }
}

/// The Connect error code an HTTP status stands for when the body has none.
fn code_for_http(status: u16) -> i32 {
    match status {
        400 => 3,
        401 => 16,
        403 => 7,
        404 | 405 => 12,
        408 => 4,
        409 => 10,
        412 => 9,
        413 => 8,
        429 | 502 | 503 | 504 => 14,
        _ => 2,
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize, Default)]
struct EndStream {
    #[serde(default)]
    error: Option<ErrorBody>,
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, Vec<String>>,
}

fn error_status(body: &[u8], http: u16, trailers: Vec<Header>) -> GrpcStatus {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(ErrorBody {
            code: Some(code),
            message,
        }) => GrpcStatus {
            code: code_number(&code),
            message: message.unwrap_or_default(),
            trailers,
        },
        _ => GrpcStatus {
            code: code_for_http(http),
            message: format!("HTTP {http}"),
            trailers,
        },
    }
}

fn split_headers(map: &reqwest::header::HeaderMap) -> (Vec<Header>, Vec<Header>) {
    let (mut headers, mut trailers) = (Vec::new(), Vec::new());
    for (name, value) in map {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        match name.as_str().strip_prefix("trailer-") {
            Some(rest) => trailers.push(Header::new(rest, value)),
            None => headers.push(Header::new(name.as_str(), value)),
        }
    }
    (headers, trailers)
}

/// Normalizes a JSON response message through the schema, so that it looks like a gRPC one;
/// text the schema rejects is kept as received.
fn render(method: &MethodDescriptor, json: &[u8]) -> String {
    let mut de = serde_json::Deserializer::from_slice(json);
    match DynamicMessage::deserialize(method.output(), &mut de) {
        Ok(message) => message_to_json(&message),
        Err(_) => String::from_utf8_lossy(json).into_owned(),
    }
}

/// Runs one Connect call, recording progress in `outcome`. `Err` means nothing reached
/// the server.
pub async fn call(
    client: &reqwest::Client,
    request: &GrpcRequest,
    method: &MethodDescriptor,
    kind: CallKind,
    messages: &[DynamicMessage],
    tx: &UnboundedSender<CallEvent>,
    outcome: &mut GrpcOutcome,
) -> Result<(), GrpcError> {
    let streaming = match kind {
        CallKind::Unary => false,
        CallKind::ServerStreaming => true,
        _ => {
            return Err(GrpcError::Invalid(
                "the Connect protocol supports unary and server streaming methods only".into(),
            ));
        }
    };
    let address = request.address.trim().trim_end_matches('/');
    if !(address.starts_with("http://") || address.starts_with("https://")) {
        return Err(GrpcError::Invalid(format!(
            "the address `{address}` needs a scheme: http:// or https://"
        )));
    }
    let path = format!("/{}/{}", method.parent_service().full_name(), method.name());
    let url = reqwest::Url::parse(&format!("{address}{path}"))
        .map_err(|e| GrpcError::Invalid(format!("invalid address `{address}`: {e}")))?;

    let trace = {
        let tx = tx.clone();
        Trace::new(move |line| {
            let _ = tx.send(CallEvent::Log(line));
        })
    };
    trace_url_resolve(&trace, &url).await;

    let payload = messages
        .first()
        .map(message_to_json)
        .unwrap_or_else(|| "{}".into());
    let (content_type, body) = if streaming {
        let mut envelope = vec![0u8];
        envelope.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        envelope.extend_from_slice(payload.as_bytes());
        ("application/connect+json", envelope)
    } else {
        ("application/json", payload.into_bytes())
    };

    let mut builder = client
        .post(url)
        .header("content-type", content_type)
        .header("connect-protocol-version", "1");
    if streaming {
        builder = builder.header("connect-content-encoding", "identity");
    }
    trace.emit(format!("> POST {path}"));
    trace.emit(format!("> content-type: {content_type}"));
    trace.emit("> connect-protocol-version: 1");
    for h in &request.metadata {
        builder = builder.header(h.name.trim(), h.value.trim());
        trace.emit(format!("> {}: {}", h.name.trim(), h.value.trim()));
    }
    trace.emit(">");

    let mut response = builder
        .body(body)
        .send()
        .await
        .map_err(|e| GrpcError::Connect(format!("cannot connect to {address}: {e}")))?;

    let http = response.status().as_u16();
    let (headers, mut trailers) = split_headers(response.headers());
    trace.emit(format!("< {:?} {http}", response.version()));
    for h in &headers {
        trace.emit(format!("< {}: {}", h.name, h.value));
    }
    outcome.metadata = headers.clone();
    let _ = tx.send(CallEvent::Headers(headers));

    let finish = |outcome: &mut GrpcOutcome, status: GrpcStatus| {
        trace.emit(format!("< status: {} ({})", status.code, status.name()));
        if !status.message.is_empty() {
            trace.emit(format!("< message: {}", status.message));
        }
        outcome.status = Some(status);
    };

    if http != 200 {
        let body = response.bytes().await.unwrap_or_default();
        finish(outcome, error_status(&body, http, trailers));
        return Ok(());
    }

    if !streaming {
        let body = response
            .bytes()
            .await
            .map_err(|e| GrpcError::Connect(format!("the connection to {address} broke: {e}")))?;
        let text = render(method, &body);
        outcome.messages.push(text.clone());
        let _ = tx.send(CallEvent::Message(text));
        finish(
            outcome,
            GrpcStatus {
                code: 0,
                message: String::new(),
                trailers,
            },
        );
        return Ok(());
    }

    // Server streaming: `flags(1) length(4, big endian) payload` envelopes.
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        while buffer.len() >= 5 {
            let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
            if buffer.len() < 5 + len {
                break;
            }
            let flags = buffer[0];
            let payload: Vec<u8> = buffer[5..5 + len].to_vec();
            buffer.drain(..5 + len);
            if flags & COMPRESSED != 0 {
                finish(
                    outcome,
                    GrpcStatus {
                        code: 13,
                        message:
                            "the server sent a compressed message; compression is not supported"
                                .into(),
                        trailers,
                    },
                );
                return Ok(());
            }
            if flags & END_STREAM != 0 {
                let end: EndStream = serde_json::from_slice(&payload).unwrap_or_default();
                for (name, values) in end.metadata {
                    trailers.extend(values.into_iter().map(|v| Header::new(name.clone(), v)));
                }
                let status = match end.error {
                    Some(e) => GrpcStatus {
                        code: code_number(e.code.as_deref().unwrap_or("unknown")),
                        message: e.message.unwrap_or_default(),
                        trailers,
                    },
                    None => GrpcStatus {
                        code: 0,
                        message: String::new(),
                        trailers,
                    },
                };
                finish(outcome, status);
                return Ok(());
            }
            let text = render(method, &payload);
            outcome.messages.push(text.clone());
            let _ = tx.send(CallEvent::Message(text));
        }
        match response.chunk().await {
            Ok(Some(chunk)) => buffer.extend_from_slice(&chunk),
            Ok(None) => {
                finish(
                    outcome,
                    GrpcStatus {
                        code: 14,
                        message: "the stream ended without an end-of-stream message".into(),
                        trailers,
                    },
                );
                return Ok(());
            }
            Err(e) => {
                finish(
                    outcome,
                    GrpcStatus {
                        code: 14,
                        message: format!("the connection broke: {e}"),
                        trailers,
                    },
                );
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_codes_map_to_grpc_numbers() {
        assert_eq!(code_number("not_found"), 5);
        assert_eq!(code_number("unauthenticated"), 16);
        assert_eq!(code_number("whatever"), 2);
    }

    #[test]
    fn error_body_is_read_and_http_status_is_the_fallback() {
        let s = error_status(br#"{"code":"not_found","message":"no user"}"#, 404, vec![]);
        assert_eq!((s.code, s.message.as_str()), (5, "no user"));
        let s = error_status(b"<html>", 503, vec![]);
        assert_eq!((s.code, s.message.as_str()), (14, "HTTP 503"));
    }
}
