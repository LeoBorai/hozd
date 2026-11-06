//! Logic every client shares: TLS setup, request tracing and header defaults.

use super::{Header, Trace, trace_resolve};

/// Several crates in the build enable different rustls providers, so rustls cannot pick a
/// process default by itself; choose `ring` once.
pub(crate) fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Reports the DNS resolution of the host in `url` to `trace`.
pub(crate) async fn trace_url_resolve(trace: &Trace, url: &reqwest::Url) {
    if let Some(host) = url.host_str() {
        trace_resolve(trace, host, url.port_or_known_default().unwrap_or(80)).await;
    }
}

/// Appends `name: value` unless a header called `name` (case-insensitive) is there already.
pub(crate) fn add_missing_header(headers: &mut Vec<Header>, name: &str, value: &str) {
    if !headers.iter().any(|h| h.name.eq_ignore_ascii_case(name)) {
        headers.push(Header::new(name, value));
    }
}

/// `: <first 200 chars of body>` for error messages; empty when the body is blank.
pub(crate) fn body_snippet(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        String::new()
    } else {
        format!(": {}", body.chars().take(200).collect::<String>())
    }
}
