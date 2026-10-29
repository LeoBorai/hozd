mod support;

use prost_reflect::DescriptorPool;
use support::*;

#[test]
fn descriptor_fixture_decodes() {
    let pool = DescriptorPool::decode(descriptor_bytes().as_slice()).unwrap();
    let service = pool.get_service_by_name("demo.v1.Demo").unwrap();
    assert_eq!(service.methods().count(), 6);
    assert!(
        pool.get_message_by_name("google.protobuf.Timestamp")
            .is_some()
    );
}

#[tokio::test]
async fn server_starts_answers_and_stops() {
    let mut server = TestServer::start(Options::default()).await;
    let stream = tokio::net::TcpStream::connect(server.addr).await;
    assert!(stream.is_ok());
    assert_eq!(server.calls(), 0);
    server.stop();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(tokio::net::TcpStream::connect(server.addr).await.is_err());
}
