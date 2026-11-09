use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client::Header;

/// Where the schema of a gRPC server comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemaSource {
    /// Ask the server through the reflection protocol.
    #[default]
    Reflection,
    /// A `.proto` file or a compiled `FileDescriptorSet`.
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GrpcRequest {
    pub address: String,
    /// `package.Service/Method`.
    pub method: String,
    #[serde(default)]
    pub schema: SchemaSource,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub metadata: Vec<Header>,
    /// JSON: an object, or an array of objects for client and bidirectional streaming.
    #[serde(default)]
    pub body: Option<String>,
}

/// The final status of a call that reached the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrpcStatus {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub trailers: Vec<Header>,
}

impl GrpcStatus {
    pub fn name(&self) -> &'static str {
        status_name(self.code)
    }
}

pub fn status_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN",
    }
}

/// The result of a gRPC call: what was received before it ended, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GrpcOutcome {
    #[serde(default)]
    pub metadata: Vec<Header>,
    /// Response messages as JSON, in arrival order.
    #[serde(default)]
    pub messages: Vec<String>,
    /// `None` when the call never reached the server (see `error`).
    pub status: Option<GrpcStatus>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub elapsed: Duration,
    /// Whether the method streams responses (server streaming or bidirectional).
    #[serde(default)]
    pub response_stream: bool,
}

impl GrpcOutcome {
    /// The response as one text: the error when the call never reached the server, a JSON
    /// array of every message for streamed responses, otherwise the single message. A call
    /// that ended with an error status and no message is its status line.
    pub fn body_text(&self) -> String {
        let Some(status) = &self.status else {
            return self.error.clone().unwrap_or_default();
        };
        if self.messages.is_empty() && status.code != 0 && !self.response_stream {
            return match status.message.is_empty() {
                true => status.name().to_owned(),
                false => format!("{}: {}", status.name(), status.message),
            };
        }
        if self.response_stream {
            let items: Vec<serde_json::Value> = self
                .messages
                .iter()
                .map(|m| {
                    serde_json::from_str(m).unwrap_or_else(|_| serde_json::Value::String(m.clone()))
                })
                .collect();
            return serde_json::to_string_pretty(&items).unwrap_or_default();
        }
        self.messages.first().cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names_cover_the_standard_codes() {
        assert_eq!(status_name(0), "OK");
        assert_eq!(status_name(5), "NOT_FOUND");
        assert_eq!(status_name(16), "UNAUTHENTICATED");
        assert_eq!(status_name(99), "UNKNOWN");
    }

    fn outcome(messages: &[&str], stream: bool, code: i32, message: &str) -> GrpcOutcome {
        GrpcOutcome {
            messages: messages.iter().map(|m| m.to_string()).collect(),
            status: Some(GrpcStatus {
                code,
                message: message.into(),
                trailers: vec![],
            }),
            response_stream: stream,
            ..Default::default()
        }
    }

    #[test]
    fn body_text_is_the_single_message_for_unary_and_an_array_for_streams() {
        assert_eq!(
            outcome(&["{\"a\":1}"], false, 0, "").body_text(),
            "{\"a\":1}"
        );
        let streamed = outcome(&["{\"a\":1}"], true, 0, "").body_text();
        let value: serde_json::Value = serde_json::from_str(&streamed).unwrap();
        assert_eq!(value, serde_json::json!([{"a": 1}]));
        assert_eq!(
            outcome(&[], true, 0, "")
                .body_text()
                .replace(char::is_whitespace, ""),
            "[]"
        );
        assert_eq!(outcome(&[], false, 0, "").body_text(), "");
    }

    #[test]
    fn body_text_of_an_error_status_without_messages_is_the_status() {
        assert_eq!(
            outcome(&[], false, 5, "user 7 not found").body_text(),
            "NOT_FOUND: user 7 not found"
        );
        assert_eq!(outcome(&[], false, 14, "").body_text(), "UNAVAILABLE");
    }

    #[test]
    fn body_text_of_a_call_that_never_connected_is_the_error() {
        let out = GrpcOutcome {
            error: Some("refused".into()),
            ..Default::default()
        };
        assert_eq!(out.body_text(), "refused");
    }

    #[test]
    fn request_round_trips_through_json() {
        let req = GrpcRequest {
            address: "http://x:1".into(),
            method: "a.B/C".into(),
            schema: SchemaSource::File("api/a.proto".into()),
            insecure: true,
            metadata: vec![Header::new("k", "v")],
            body: Some("{}".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<GrpcRequest>(&json).unwrap(), req);
    }
}
