use std::sync::{Arc, Mutex};

use hozd::client::{ClientError, Header, HttpClient, HttpRequest, Method, ReqwestClient, Trace};
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req(method: Method, url: String) -> HttpRequest {
    HttpRequest {
        method,
        url,
        ..Default::default()
    }
}

#[tokio::test]
async fn get_returns_status_headers_body_and_timing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/hello"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-test", "yes")
                .set_body_string("hi"),
        )
        .mount(&server)
        .await;

    let res = ReqwestClient::new()
        .send(&req(Method::Get, format!("{}/hello", server.uri())))
        .await
        .unwrap();

    assert_eq!(res.status, 200);
    assert_eq!(res.body, "hi");
    assert!(res.headers.contains(&Header::new("x-test", "yes")));
    assert!(res.elapsed > std::time::Duration::ZERO);
}

#[tokio::test]
async fn post_sends_body_and_headers_unchanged() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("content-type", "application/json"))
        .and(body_string("{\"a\":1}"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;

    let request = HttpRequest {
        method: Method::Post,
        url: server.uri(),
        headers: vec![Header::new("Content-Type", "application/json")],
        body: Some("{\"a\":1}".into()),
    };
    let res = ReqwestClient::new().send(&request).await.unwrap();
    assert_eq!(res.status, 201);
}

#[tokio::test]
async fn every_method_is_sent_as_selected() {
    let server = MockServer::start().await;
    for m in Method::ALL {
        Mock::given(method(m.as_str()))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
    }
    for m in Method::ALL {
        let res = ReqwestClient::new()
            .send(&req(m, server.uri()))
            .await
            .unwrap();
        assert_eq!(res.status, 204, "{m}");
    }
}

#[tokio::test]
async fn error_statuses_are_responses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let res = ReqwestClient::new()
        .send(&req(Method::Get, server.uri()))
        .await
        .unwrap();
    assert_eq!((res.status, res.body.as_str()), (500, "boom"));
}

#[tokio::test]
async fn invalid_url_is_an_error() {
    let err = ReqwestClient::new()
        .send(&req(Method::Get, "not a url".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::InvalidUrl(_)), "{err:?}");
}

#[tokio::test]
async fn connection_refused_is_an_error() {
    // Port 1 (tcpmux) is reserved and nothing listens there; unlike a freed ephemeral
    // port it cannot be grabbed by another test server in parallel.
    let port = 1;
    let err = ReqwestClient::new()
        .send(&req(Method::Get, format!("http://127.0.0.1:{port}")))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Transport(_)), "{err:?}");
}

/// A server that answers with something other than HTTP/1.x (here a TLS alert, which is
/// what an HTTPS port sends back to a plain `http://` request).
#[tokio::test]
async fn non_http_reply_explains_the_likely_cause() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = socket.read(&mut buf).await;
        let _ = socket.write_all(b"\x15\x03\x01\x00\x02\x02\x46").await;
    });

    let err = ReqwestClient::new()
        .send(&req(Method::Get, format!("http://{addr}/")))
        .await
        .unwrap_err();
    let ClientError::Transport(msg) = err else {
        panic!("expected transport error");
    };
    assert!(msg.contains("invalid HTTP version"), "{msg}");
    assert!(msg.contains("try https://"), "{msg}");
}

#[tokio::test]
async fn a_traced_send_reports_the_connection_the_request_and_the_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/t"))
        .respond_with(ResponseTemplate::new(201).insert_header("x-test", "yes"))
        .mount(&server)
        .await;

    let lines = Arc::new(Mutex::new(Vec::new()));
    let trace = Trace::new({
        let lines = lines.clone();
        move |l| lines.lock().unwrap().push(l)
    });
    let request = HttpRequest {
        method: Method::Post,
        url: format!("{}/t?a=1", server.uri()),
        headers: vec![Header::new("x-sent", "1")],
        body: Some("abc".into()),
    };
    ReqwestClient::new()
        .send_traced(&request, &trace)
        .await
        .unwrap();

    let lines = lines.lock().unwrap().clone();
    let has = |prefix: &str| lines.iter().any(|l| l.starts_with(prefix));
    assert!(has("* Host 127.0.0.1:"), "{lines:?}");
    assert!(
        has("* Connected to 127.0.0.1 (127.0.0.1) port"),
        "{lines:?}"
    );
    assert!(has("> POST /t?a=1 HTTP/1.1"), "{lines:?}");
    assert!(has("> x-sent: 1"), "{lines:?}");
    assert!(has("* Request body: 3 bytes"), "{lines:?}");
    assert!(has("< HTTP/1.1 201"), "{lines:?}");
    assert!(has("< x-test: yes"), "{lines:?}");
    assert!(
        !has("* Server certificate"),
        "plain HTTP has no TLS: {lines:?}"
    );
}
