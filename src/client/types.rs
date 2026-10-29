use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::ClientError;
use super::grpc::{GrpcOutcome, GrpcRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const ALL: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }

    /// The next method in `ALL`, wrapping around.
    pub fn next(self) -> Method {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Method {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Method {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Method::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("unsupported HTTP method `{s}`"))
    }
}

/// A header as an ordered name/value pair (order and duplicates are preserved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: String,
    pub elapsed: Duration,
}

/// What the user asked to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Http(HttpRequest),
    Grpc(GrpcRequest),
}

impl Request {
    pub fn kind(&self) -> Kind {
        match self {
            Request::Http(_) => Kind::Http,
            Request::Grpc(_) => Kind::Grpc,
        }
    }
}

impl From<HttpRequest> for Request {
    fn from(r: HttpRequest) -> Self {
        Request::Http(r)
    }
}

impl From<GrpcRequest> for Request {
    fn from(r: GrpcRequest) -> Self {
        Request::Grpc(r)
    }
}

/// What came back from an executed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// An HTTP response (any status).
    Response(HttpResponse),
    /// No HTTP response was received.
    Error(String),
    /// A gRPC call, including calls that failed before reaching the server.
    Grpc(GrpcOutcome),
}

impl From<Result<HttpResponse, ClientError>> for Outcome {
    fn from(r: Result<HttpResponse, ClientError>) -> Self {
        match r {
            Ok(res) => Outcome::Response(res),
            Err(e) => Outcome::Error(e.to_string()),
        }
    }
}

/// The protocol of a request, as stored in history and templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Http,
    Grpc,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Http => "http",
            Kind::Grpc => "grpc",
        }
    }
}

impl FromStr for Kind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "http" => Ok(Kind::Http),
            "grpc" => Ok(Kind::Grpc),
            other => Err(format!("unknown request kind `{other}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let req = HttpRequest {
            method: Method::Post,
            url: "http://x/y".into(),
            headers: vec![Header::new("A", "1"), Header::new("A", "2")],
            body: Some("{\"a\":1}".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<HttpRequest>(&json).unwrap(), req);
    }

    #[test]
    fn response_round_trips_through_json() {
        let res = HttpResponse {
            status: 201,
            headers: vec![Header::new("X", "y")],
            body: "ok".into(),
            elapsed: Duration::from_millis(12),
        };
        let json = serde_json::to_string(&res).unwrap();
        assert_eq!(serde_json::from_str::<HttpResponse>(&json).unwrap(), res);
    }

    #[test]
    fn method_parses_case_insensitively_and_cycles() {
        assert_eq!("patch".parse::<Method>().unwrap(), Method::Patch);
        assert!("TRACE".parse::<Method>().is_err());
        assert_eq!(Method::Options.next(), Method::Get);
        assert_eq!(Method::Get.prev(), Method::Options);
    }
}
