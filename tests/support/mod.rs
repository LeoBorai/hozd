//! An in-process gRPC server for the fixture service in `tests/fixtures/proto/demo.proto`.
//!
//! Messages are hand-written prost structs so the server does not depend on the code under test.
#![allow(dead_code)]

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::body::Body;
use tonic::codegen::{Service, http};
use tonic::server::{Grpc, NamedService};
use tonic::transport::{Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status, Streaming};
use tonic_prost::ProstCodec;

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn descriptor_bytes() -> Vec<u8> {
    std::fs::read(fixtures().join("descriptor.bin")).unwrap()
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GetUserRequest {
    #[prost(int64, tag = "1")]
    pub id: i64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ListUsersRequest {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(int32, tag = "2")]
    pub page_size: i32,
    #[prost(int32, tag = "3")]
    pub delay_ms: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct User {
    #[prost(int64, tag = "1")]
    pub id: i64,
    #[prost(string, tag = "2")]
    pub name: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Summary {
    #[prost(int32, tag = "1")]
    pub count: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ChatMessage {
    #[prost(string, tag = "1")]
    pub text: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Node {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, optional, boxed, tag = "2")]
    pub child: Option<Box<Node>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Reflection {
    None,
    V1,
    V1AlphaOnly,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub reflection: Reflection,
    pub tls: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            reflection: Reflection::V1,
            tls: false,
        }
    }
}

/// What the handlers saw, for assertions.
#[derive(Default)]
struct State {
    calls: AtomicUsize,
    /// Metadata of the last `GetUser` call, as `(name, value)`.
    metadata: Mutex<Vec<(String, String)>>,
}

pub struct TestServer {
    pub addr: SocketAddr,
    tls: bool,
    cert_pem: Option<String>,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl TestServer {
    pub async fn start(options: Options) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State::default());
        let mut cert_pem = None;
        let (tx, rx) = oneshot::channel::<()>();

        let mut builder = Server::builder();
        if options.tls {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            cert_pem = Some(certified.cert.pem());
            let identity =
                Identity::from_pem(certified.cert.pem(), certified.signing_key.serialize_pem());
            builder = builder
                .tls_config(ServerTlsConfig::new().identity(identity))
                .unwrap();
        }
        let mut router = builder.add_service(DemoSvc {
            state: state.clone(),
        });
        let descriptor = descriptor_bytes();
        if options.reflection == Reflection::V1 {
            let svc = tonic_reflection::server::Builder::configure()
                .register_encoded_file_descriptor_set(&descriptor)
                .build_v1()
                .unwrap();
            router = router.add_service(svc);
        }
        // The v1alpha service is served in both reflection modes except `None`.
        if options.reflection != Reflection::None {
            let svc = tonic_reflection::server::Builder::configure()
                .register_encoded_file_descriptor_set(&descriptor)
                .build_v1alpha()
                .unwrap();
            router = router.add_service(svc);
        }
        tokio::spawn(async move {
            let _ = router
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = rx.await;
                })
                .await;
        });

        Self {
            addr,
            tls: options.tls,
            cert_pem,
            state,
            shutdown: Some(tx),
        }
    }

    /// `http://127.0.0.1:<port>`, or `https://localhost:<port>` for a TLS server.
    pub fn url(&self) -> String {
        if self.tls {
            format!("https://localhost:{}", self.addr.port())
        } else {
            format!("http://{}", self.addr)
        }
    }

    /// Number of `Demo` calls that reached a handler.
    pub fn calls(&self) -> usize {
        self.state.calls.load(Ordering::SeqCst)
    }

    /// Metadata of the last `GetUser` call.
    pub fn metadata(&self) -> Vec<(String, String)> {
        self.state.metadata.lock().unwrap().clone()
    }

    /// The self-signed certificate of a TLS server, as PEM.
    pub fn cert_pem(&self) -> Option<&str> {
        self.cert_pem.as_deref()
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

/// Adapts an async closure to the `tower::Service` shape tonic's handlers accept.
struct Handler<F>(F);

impl<F, Req, Res, Fut> Service<Request<Req>> for Handler<F>
where
    F: FnMut(Request<Req>) -> Fut,
    Fut: Future<Output = Result<Response<Res>, Status>>,
{
    type Response = Response<Res>;
    type Error = Status;
    type Future = Fut;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Status>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Req>) -> Fut {
        (self.0)(request)
    }
}

#[derive(Clone)]
struct DemoSvc {
    state: Arc<State>,
}

impl NamedService for DemoSvc {
    const NAME: &'static str = "demo.v1.Demo";
}

impl Service<http::Request<Body>> for DemoSvc {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<Body>) -> Self::Future {
        let state = self.state.clone();
        Box::pin(async move {
            let path = req.uri().path().to_owned();
            let response = match path.as_str() {
                "/demo.v1.Demo/GetUser" => {
                    Grpc::new(ProstCodec::<User, GetUserRequest>::default())
                        .unary(
                            Handler(move |r: Request<GetUserRequest>| {
                                state.calls.fetch_add(1, Ordering::SeqCst);
                                *state.metadata.lock().unwrap() = r
                                    .metadata()
                                    .iter()
                                    .filter_map(|e| match e {
                                        tonic::metadata::KeyAndValueRef::Ascii(k, v) => {
                                            Some((k.to_string(), v.to_str().ok()?.to_owned()))
                                        }
                                        _ => None,
                                    })
                                    .collect();
                                async move { get_user(r.into_inner()) }
                            }),
                            req,
                        )
                        .await
                }
                "/demo.v1.Demo/ListUsers" => {
                    Grpc::new(ProstCodec::<User, ListUsersRequest>::default())
                        .server_streaming(
                            Handler(move |r: Request<ListUsersRequest>| {
                                state.calls.fetch_add(1, Ordering::SeqCst);
                                async move { Ok(Response::new(list_users(r.into_inner()))) }
                            }),
                            req,
                        )
                        .await
                }
                "/demo.v1.Demo/Collect" => {
                    Grpc::new(ProstCodec::<Summary, GetUserRequest>::default())
                        .client_streaming(
                            Handler(move |r: Request<Streaming<GetUserRequest>>| {
                                state.calls.fetch_add(1, Ordering::SeqCst);
                                async move { collect(r.into_inner()).await }
                            }),
                            req,
                        )
                        .await
                }
                "/demo.v1.Demo/Chat" => {
                    Grpc::new(ProstCodec::<ChatMessage, ChatMessage>::default())
                        .streaming(
                            Handler(move |r: Request<Streaming<ChatMessage>>| {
                                state.calls.fetch_add(1, Ordering::SeqCst);
                                async move { Ok(Response::new(chat(r.into_inner()))) }
                            }),
                            req,
                        )
                        .await
                }
                "/demo.v1.Demo/Tree" => {
                    Grpc::new(ProstCodec::<Node, Node>::default())
                        .unary(
                            Handler(move |r: Request<Node>| {
                                state.calls.fetch_add(1, Ordering::SeqCst);
                                async move { Ok(Response::new(r.into_inner())) }
                            }),
                            req,
                        )
                        .await
                }
                _ => Status::unimplemented("unknown method").into_http(),
            };
            Ok(response)
        })
    }
}

fn get_user(req: GetUserRequest) -> Result<Response<User>, Status> {
    if req.id == 7 {
        return Err(Status::not_found("user 7 not found"));
    }
    Ok(Response::new(User {
        id: req.id,
        name: format!("user-{}", req.id),
    }))
}

fn list_users(req: ListUsersRequest) -> BoxStream<User> {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        for id in 1..=i64::from(req.page_size) {
            if req.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(req.delay_ms as u64)).await;
            }
            let user = User {
                id,
                name: format!("{}{id}", req.name),
            };
            // A closed receiver means the client went away.
            if tx.send(Ok(user)).await.is_err() {
                return;
            }
        }
    });
    Box::pin(ReceiverStream::new(rx))
}

async fn collect(mut stream: Streaming<GetUserRequest>) -> Result<Response<Summary>, Status> {
    let mut count = 0;
    while stream.message().await?.is_some() {
        count += 1;
    }
    Ok(Response::new(Summary { count }))
}

fn chat(mut stream: Streaming<ChatMessage>) -> BoxStream<ChatMessage> {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        while let Ok(Some(m)) = stream.message().await {
            let echo = ChatMessage {
                text: format!("echo: {}", m.text),
            };
            if tx.send(Ok(echo)).await.is_err() {
                return;
            }
        }
    });
    Box::pin(ReceiverStream::new(rx))
}
