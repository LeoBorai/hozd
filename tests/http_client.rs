use hozd::client::{Client, ClientError, Header, HttpClient, Method, Request};
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req(method: Method, url: String) -> Request {
    Request {
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

    let res = HttpClient::new()
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

    let request = Request {
        method: Method::Post,
        url: server.uri(),
        headers: vec![Header::new("Content-Type", "application/json")],
        body: Some("{\"a\":1}".into()),
    };
    let res = HttpClient::new().send(&request).await.unwrap();
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
        let res = HttpClient::new().send(&req(m, server.uri())).await.unwrap();
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
    let res = HttpClient::new()
        .send(&req(Method::Get, server.uri()))
        .await
        .unwrap();
    assert_eq!((res.status, res.body.as_str()), (500, "boom"));
}

#[tokio::test]
async fn invalid_url_is_an_error() {
    let err = HttpClient::new()
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
    let err = HttpClient::new()
        .send(&req(Method::Get, format!("http://127.0.0.1:{port}")))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Transport(_)), "{err:?}");
}
