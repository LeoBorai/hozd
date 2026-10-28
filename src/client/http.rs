use std::time::Instant;

use async_trait::async_trait;

use super::{Client, ClientError, Header, Method, Request, Response};

/// Plain HTTP(S) client on top of `reqwest`.
#[derive(Clone, Default)]
pub struct HttpClient {
    inner: reqwest::Client,
}

impl HttpClient {
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<Method> for reqwest::Method {
    fn from(m: Method) -> Self {
        match m {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
            Method::Delete => reqwest::Method::DELETE,
            Method::Head => reqwest::Method::HEAD,
            Method::Options => reqwest::Method::OPTIONS,
        }
    }
}

#[async_trait]
impl Client for HttpClient {
    async fn send(&self, request: &Request) -> Result<Response, ClientError> {
        let url = reqwest::Url::parse(request.url.trim())
            .map_err(|e| ClientError::InvalidUrl(format!("{}: {e}", request.url)))?;

        let mut builder = self.inner.request(request.method.into(), url);
        for h in &request.headers {
            builder = builder.header(&h.name, &h.value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }

        let started = Instant::now();
        let res = builder
            .send()
            .await
            .map_err(|e| ClientError::Transport(describe(&e)))?;

        let status = res.status().as_u16();
        let headers = res
            .headers()
            .iter()
            .map(|(k, v)| Header::new(k.as_str(), String::from_utf8_lossy(v.as_bytes())))
            .collect();
        let body = res
            .text()
            .await
            .map_err(|e| ClientError::Transport(describe(&e)))?;

        Ok(Response {
            status,
            headers,
            body,
            elapsed: started.elapsed(),
        })
    }
}

const NOT_HTTP_HINT: &str = "the server did not answer with HTTP/1.x. Likely causes: an HTTPS port \
called with http:// (try https://), or a service that is not plain HTTP";

/// `reqwest` hides the root cause in the source chain; surface it.
fn describe(err: &reqwest::Error) -> String {
    let mut msg = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    if msg.contains("invalid HTTP version") {
        msg.push_str(&format!(" ({NOT_HTTP_HINT})"));
    }
    msg
}
