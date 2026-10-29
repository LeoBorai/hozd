//! Channels to gRPC servers: plaintext, TLS, and TLS without certificate verification.

use std::error::Error as _;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use hyper_util::client::legacy::connect::HttpConnector;
use rustls::DigitallySignedStruct;
use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tonic::codegen::Service;
use tonic::codegen::http::Uri;
use tonic::codegen::http::uri::Scheme;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use super::GrpcError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens a channel to `address`: `http://` is plaintext HTTP/2, `https://` is TLS verified
/// against the system trust store, or not verified at all when `insecure` is set.
pub async fn connect(address: &str, insecure: bool) -> Result<Channel, GrpcError> {
    install_crypto_provider();
    let address = address.trim();
    let secure = if address.starts_with("https://") {
        true
    } else if address.starts_with("http://") {
        false
    } else {
        return Err(GrpcError::Invalid(format!(
            "the address `{address}` needs a scheme: http:// for plaintext or https:// for TLS"
        )));
    };

    // tonic refuses an `https` endpoint that has no TLS configuration of its own, so the
    // insecure path hands it an `http` one and turns it back in the connector.
    let endpoint_uri = match (secure, insecure) {
        (true, true) => address.replacen("https://", "http://", 1),
        _ => address.to_owned(),
    };
    let endpoint = Endpoint::from_shared(endpoint_uri)
        .map_err(|e| GrpcError::Invalid(format!("invalid address `{address}`: {e}")))?
        .connect_timeout(CONNECT_TIMEOUT);

    let connected = if !secure {
        endpoint.connect().await
    } else if insecure {
        endpoint.connect_with_connector(insecure_connector()).await
    } else {
        endpoint
            .tls_config(ClientTlsConfig::new().with_native_roots())
            .map_err(|e| GrpcError::Invalid(format!("invalid TLS setup: {e}")))?
            .connect()
            .await
    };
    connected.map_err(|e| GrpcError::Connect(describe(address, secure && !insecure, &e)))
}

/// Several crates in the build enable different rustls providers, so rustls cannot pick a
/// process default by itself; choose `ring` once.
fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// The cause is in the source chain; `transport error` alone says nothing.
fn describe(address: &str, verifying: bool, err: &tonic::transport::Error) -> String {
    let mut chain = vec![err.to_string()];
    let mut source = err.source();
    while let Some(s) = source {
        chain.push(s.to_string());
        source = s.source();
    }
    let detail = chain.join(": ");
    let lower = detail.to_ascii_lowercase();
    if verifying && (lower.contains("certificate") || lower.contains("invalid peer")) {
        format!(
            "cannot connect to {address}: the server certificate is not trusted ({detail}); \
             turn on insecure mode to skip verification"
        )
    } else {
        format!("cannot connect to {address}: {detail}")
    }
}

/// Connects over TLS whatever the scheme of the URI it is given.
#[derive(Clone)]
struct InsecureConnector(hyper_rustls::HttpsConnector<HttpConnector>);

impl Service<Uri> for InsecureConnector {
    type Response = <hyper_rustls::HttpsConnector<HttpConnector> as Service<Uri>>::Response;
    type Error = <hyper_rustls::HttpsConnector<HttpConnector> as Service<Uri>>::Error;
    type Future = <hyper_rustls::HttpsConnector<HttpConnector> as Service<Uri>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let mut parts = uri.into_parts();
        parts.scheme = Some(Scheme::HTTPS);
        let uri = Uri::from_parts(parts).expect("the scheme was the only part changed");
        self.0.call(uri)
    }
}

fn insecure_connector() -> InsecureConnector {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
        .with_no_client_auth();

    let mut http = HttpConnector::new();
    http.enforce_http(false);
    InsecureConnector(
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_only()
            .enable_http2()
            .wrap_connector(http),
    )
}

/// Accepts every certificate. Only used when the user turns insecure mode on.
#[derive(Debug)]
struct AcceptAnyCertificate(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
