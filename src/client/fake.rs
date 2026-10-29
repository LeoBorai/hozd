use std::sync::Mutex;

use async_trait::async_trait;

use super::{ClientError, HttpClient, HttpRequest, HttpResponse};

/// Test double: replies with a canned result and records every request it receives.
pub struct FakeClient {
    reply: Result<HttpResponse, ClientError>,
    seen: Mutex<Vec<HttpRequest>>,
}

impl FakeClient {
    pub fn new(reply: Result<HttpResponse, ClientError>) -> Self {
        Self {
            reply,
            seen: Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl HttpClient for FakeClient {
    async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, ClientError> {
        self.seen.lock().unwrap().push(request.clone());
        self.reply.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::client::SharedHttp;

    #[tokio::test]
    async fn usable_as_shared_dyn_client() {
        let fake = Arc::new(FakeClient::new(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: "hi".into(),
            elapsed: Duration::ZERO,
        })));
        let client: SharedHttp = fake.clone();
        let res = client.send(&HttpRequest::default()).await.unwrap();
        assert_eq!(res.body, "hi");
        assert_eq!(fake.requests().len(), 1);
    }
}
