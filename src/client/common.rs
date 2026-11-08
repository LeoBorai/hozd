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

/// `url` with `params` appended to its query string, percent-encoded. A `#fragment` stays
/// last, and an existing query is kept.
pub fn with_query(url: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return url.to_owned();
    }
    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let sep = if !base.contains('?') {
        "?"
    } else if base.ends_with('?') || base.ends_with('&') {
        ""
    } else {
        "&"
    };
    let mut out = format!("{base}{sep}{query}");
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// Percent-encodes everything but the RFC 3986 unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn p(k: &str, v: &str) -> (String, String) {
        (k.to_owned(), v.to_owned())
    }

    #[test]
    fn encodes_params() {
        assert_eq!(
            with_query("http://x/a", &[p("q", "a b&c=d"), p("é", "1")]),
            "http://x/a?q=a%20b%26c%3Dd&%C3%A9=1"
        );
    }

    #[test]
    fn keeps_query_and_fragment() {
        assert_eq!(
            with_query("http://x/?a=1#top", &[p("b", "2")]),
            "http://x/?a=1&b=2#top"
        );
        assert_eq!(with_query("http://x/", &[]), "http://x/");
    }
}
