//! TLS with a certificate the system trusts. Its own test binary: it points `SSL_CERT_FILE`
//! at the test server's certificate, which would make the "untrusted" tests meaningless.

mod support;

use hozd::client::{CallEvent, GrpcClient, GrpcRequest, SchemaSource, TonicGrpc, load_schema_file};
use support::{Options, Reflection, TestServer, fixtures};

#[tokio::test]
async fn verified_tls_succeeds_with_a_trusted_certificate() {
    let server = TestServer::start(Options {
        tls: true,
        reflection: Reflection::None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let pem = dir.path().join("ca.pem");
    std::fs::write(&pem, server.cert_pem().unwrap()).unwrap();
    // SAFETY: the only test of this binary, so nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("SSL_CERT_FILE", &pem) };

    let schema = load_schema_file(&fixtures().join("descriptor.bin"), &[]).unwrap();
    let request = GrpcRequest {
        address: server.url(),
        method: "demo.v1.Demo/GetUser".into(),
        schema: SchemaSource::File(fixtures().join("descriptor.bin")),
        insecure: false,
        body: Some(r#"{"id": 2}"#.into()),
        ..Default::default()
    };
    let mut call = TonicGrpc::new(Vec::new()).call(request, schema);
    let outcome = loop {
        if let CallEvent::Done(o) = call.events.recv().await.expect("events") {
            break o;
        }
    };
    assert_eq!(
        outcome.status.clone().expect("a status").code,
        0,
        "{outcome:?}"
    );
    assert!(outcome.messages[0].contains("user-2"));
}
