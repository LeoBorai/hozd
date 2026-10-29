use std::time::Instant;

use async_trait::async_trait;

use super::trace::version_name;
use super::{
    ClientError, Header, HttpClient, HttpRequest, HttpResponse, Method, Trace, trace_certificate,
    trace_resolve,
};

/// Plain HTTP(S) client on top of `reqwest`.
#[derive(Clone)]
pub struct ReqwestClient {
    inner: reqwest::Client,
}

impl ReqwestClient {
    pub fn new() -> Self {
        Self {
            inner: reqwest::Client::builder()
                .tls_info(true)
                .build()
                .unwrap_or_default(),
        }
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new()
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
impl HttpClient for ReqwestClient {
    async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, ClientError> {
        self.send_traced(request, &Trace::none()).await
    }

    async fn send_traced(
        &self,
        request: &HttpRequest,
        trace: &Trace,
    ) -> Result<HttpResponse, ClientError> {
        let url = reqwest::Url::parse(request.url.trim())
            .map_err(|e| ClientError::InvalidUrl(format!("{}: {e}", request.url)))?;

        if let Some(host) = url.host_str() {
            let port = url.port_or_known_default().unwrap_or(80);
            trace_resolve(trace, host, port).await;
        }

        let mut builder = self.inner.request(request.method.into(), url.clone());
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

        trace_exchange(trace, request, &url, &res);

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

        Ok(HttpResponse {
            status,
            headers,
            body,
            elapsed: started.elapsed(),
        })
    }
}

/// Reports the connection, what was sent and what came back, in curl `-v` form.
fn trace_exchange(
    trace: &Trace,
    request: &HttpRequest,
    url: &reqwest::Url,
    res: &reqwest::Response,
) {
    let host = url.host_str().unwrap_or_default();
    let port = url.port_or_known_default().unwrap_or(80);
    let version = version_name(res.version());
    if let Some(addr) = res.remote_addr() {
        trace.emit(format!("* Connected to {host} ({}) port {port}", addr.ip()));
    }
    if url.scheme() == "https" {
        if let Some(der) = res
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(|t| t.peer_certificate())
        {
            trace_certificate(trace, der);
        }
        trace.emit("*  SSL certificate verify ok.");
    }
    if res.version() == reqwest::Version::HTTP_2 {
        trace.emit("* ALPN: server accepted h2");
    }
    trace.emit(format!("* using {version}"));

    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    trace.emit(format!("> {} {target} {version}", request.method));
    trace.emit(format!("> Host: {host}"));
    for h in &request.headers {
        trace.emit(format!("> {}: {}", h.name, h.value));
    }
    trace.emit(">");
    if let Some(body) = &request.body {
        trace.emit(format!("* Request body: {} bytes", body.len()));
    }
    trace.emit(format!("< {version} {}", res.status().as_u16()));
    for (name, value) in res.headers() {
        trace.emit(format!(
            "< {name}: {}",
            String::from_utf8_lossy(value.as_bytes())
        ));
    }
    trace.emit("<");
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
