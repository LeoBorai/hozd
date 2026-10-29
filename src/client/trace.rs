//! A curl `-v` style account of what a request did, delivered line by line.

use std::sync::Arc;

use tokio::net::lookup_host;
use x509_parser::prelude::{FromDer, GeneralName, X509Certificate};

/// Receives the trace lines of a request: `* ` for connection events, `> ` for what was
/// sent and `< ` for what came back. Cheap to clone; does nothing when built with `none`.
#[derive(Clone, Default)]
pub struct Trace(Option<Arc<dyn Fn(String) + Send + Sync>>);

impl Trace {
    pub fn new(sink: impl Fn(String) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(sink)))
    }

    pub fn none() -> Self {
        Self::default()
    }

    pub fn emit(&self, line: impl Into<String>) {
        if let Some(sink) = &self.0 {
            sink(line.into());
        }
    }
}

/// Resolves `host` the way a connection would and reports the addresses.
pub async fn trace_resolve(trace: &Trace, host: &str, port: u16) {
    if trace.0.is_none() {
        return;
    }
    match lookup_host((host, port)).await {
        Ok(addrs) => {
            let (v6, v4): (Vec<_>, Vec<_>) = addrs.partition(|a| a.is_ipv6());
            let list = |addrs: Vec<std::net::SocketAddr>| {
                if addrs.is_empty() {
                    "(none)".to_owned()
                } else {
                    addrs
                        .iter()
                        .map(|a| a.ip().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            };
            trace.emit(format!("* Host {host}:{port} was resolved."));
            trace.emit(format!("* IPv6: {}", list(v6)));
            trace.emit(format!("* IPv4: {}", list(v4)));
        }
        Err(e) => trace.emit(format!("* Could not resolve host {host}: {e}")),
    }
}

/// The server certificate, as curl prints it.
pub fn trace_certificate(trace: &Trace, der: &[u8]) {
    let Ok((_, cert)) = X509Certificate::from_der(der) else {
        trace.emit("* Server certificate: could not be parsed");
        return;
    };
    trace.emit("* Server certificate:");
    trace.emit(format!("*  subject: {}", cert.subject()));
    trace.emit(format!("*  start date: {}", cert.validity().not_before));
    trace.emit(format!("*  expire date: {}", cert.validity().not_after));
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        let names: Vec<String> = san
            .value
            .general_names
            .iter()
            .filter_map(|n| match n {
                GeneralName::DNSName(name) => Some((*name).to_owned()),
                GeneralName::IPAddress(bytes) => Some(format!("{bytes:?}")),
                _ => None,
            })
            .collect();
        trace.emit(format!("*  subjectAltName: {}", names.join(", ")));
    }
    trace.emit(format!("*  issuer: {}", cert.issuer()));
}

/// `HTTP/2`, `HTTP/1.1`, ...
pub fn version_name(version: reqwest::Version) -> &'static str {
    match version {
        reqwest::Version::HTTP_09 => "HTTP/0.9",
        reqwest::Version::HTTP_10 => "HTTP/1.0",
        reqwest::Version::HTTP_11 => "HTTP/1.1",
        reqwest::Version::HTTP_2 => "HTTP/2",
        reqwest::Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/?",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn a_certificate_is_described_like_curl_does() {
        let key = rcgen::generate_simple_self_signed(vec!["example.test".to_owned()]).unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let trace = Trace::new({
            let lines = lines.clone();
            move |l| lines.lock().unwrap().push(l)
        });
        trace_certificate(&trace, key.cert.der());
        let lines = lines.lock().unwrap().join("\n");
        for expected in [
            "* Server certificate:",
            "*  subject:",
            "*  start date:",
            "*  expire date:",
            "*  subjectAltName: example.test",
            "*  issuer:",
        ] {
            assert!(lines.contains(expected), "{expected} in\n{lines}");
        }
    }
}
