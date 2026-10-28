use std::sync::Mutex;

use async_trait::async_trait;

use super::{Client, ClientError, Request, Response};

/// Test double: replies with a canned result and records every request it receives.
pub struct FakeClient {
    reply: Result<Response, ClientError>,
    seen: Mutex<Vec<Request>>,
}

impl FakeClient {
    pub fn new(reply: Result<Response, ClientError>) -> Self {
        Self {
            reply,
            seen: Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl Client for FakeClient {
    async fn send(&self, request: &Request) -> Result<Response, ClientError> {
        self.seen.lock().unwrap().push(request.clone());
        self.reply.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::client::SharedClient;

    #[tokio::test]
    async fn usable_as_shared_dyn_client() {
        let fake = Arc::new(FakeClient::new(Ok(Response {
            status: 200,
            headers: vec![],
            body: "hi".into(),
            elapsed: Duration::ZERO,
        })));
        let client: SharedClient = fake.clone();
        let res = client.send(&Request::default()).await.unwrap();
        assert_eq!(res.body, "hi");
        assert_eq!(fake.requests().len(), 1);
    }
}
