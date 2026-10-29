use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;

use super::{
    CallEvent, CallHandle, GrpcClient, GrpcError, GrpcOutcome, GrpcRequest, GrpcStatus, Schema,
    SchemaSource,
};

/// One step of a scripted call.
#[derive(Debug, Clone)]
pub enum FakeStep {
    Event(CallEvent),
    /// Keep the call open until it is cancelled.
    HoldOpen,
}

/// Test double: serves a canned schema, plays a scripted call and records every request.
pub struct FakeGrpc {
    schema: Result<Schema, GrpcError>,
    script: Vec<FakeStep>,
    seen: Mutex<Vec<GrpcRequest>>,
    loads: Mutex<Vec<(String, SchemaSource, bool)>>,
}

impl FakeGrpc {
    pub fn new(schema: Result<Schema, GrpcError>) -> Self {
        Self {
            schema,
            script: Vec::new(),
            seen: Mutex::new(Vec::new()),
            loads: Mutex::new(Vec::new()),
        }
    }

    pub fn with_script(mut self, script: Vec<FakeStep>) -> Self {
        self.script = script;
        self
    }

    pub fn requests(&self) -> Vec<GrpcRequest> {
        self.seen.lock().unwrap().clone()
    }

    /// The `(address, source, insecure)` of every schema load, in order.
    pub fn loads(&self) -> Vec<(String, SchemaSource, bool)> {
        self.loads.lock().unwrap().clone()
    }
}

impl Default for FakeGrpc {
    fn default() -> Self {
        Self::new(Ok(Schema::default()))
    }
}

#[async_trait]
impl GrpcClient for FakeGrpc {
    async fn load_schema(
        &self,
        address: &str,
        source: &SchemaSource,
        insecure: bool,
    ) -> Result<Schema, GrpcError> {
        self.loads
            .lock()
            .unwrap()
            .push((address.to_owned(), source.clone(), insecure));
        self.schema.clone()
    }

    fn call(&self, request: GrpcRequest, _schema: Schema) -> CallHandle {
        self.seen.lock().unwrap().push(request);
        let (tx, rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let script = self.script.clone();
        tokio::spawn(async move {
            let mut partial = GrpcOutcome::default();
            for step in script {
                match step {
                    FakeStep::Event(CallEvent::Done(mut outcome)) => {
                        // Like the real client, the final outcome holds everything received.
                        if outcome.messages.is_empty() {
                            outcome.messages = partial.messages;
                        }
                        if outcome.metadata.is_empty() {
                            outcome.metadata = partial.metadata;
                        }
                        let _ = tx.send(CallEvent::Done(outcome));
                        return;
                    }
                    FakeStep::Event(event) => {
                        match &event {
                            CallEvent::Headers(h) => partial.metadata = h.clone(),
                            CallEvent::Message(m) => partial.messages.push(m.clone()),
                            CallEvent::Log(_) | CallEvent::Done(_) => {}
                        }
                        let _ = tx.send(event);
                    }
                    FakeStep::HoldOpen => {
                        token.cancelled().await;
                        partial.status = Some(GrpcStatus {
                            code: 1,
                            message: "cancelled".into(),
                            trailers: Vec::new(),
                        });
                        let _ = tx.send(CallEvent::Done(partial));
                        return;
                    }
                }
            }
        });
        CallHandle::new(rx, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Header;

    fn done(code: i32) -> CallEvent {
        CallEvent::Done(GrpcOutcome {
            status: Some(GrpcStatus {
                code,
                message: String::new(),
                trailers: Vec::new(),
            }),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn plays_the_script_in_order_and_records_the_request() {
        let fake = FakeGrpc::default().with_script(vec![
            FakeStep::Event(CallEvent::Headers(vec![Header::new("a", "b")])),
            FakeStep::Event(CallEvent::Message("{\"n\":1}".into())),
            FakeStep::Event(done(0)),
        ]);
        let request = GrpcRequest {
            method: "a.B/C".into(),
            ..Default::default()
        };
        let mut call = fake.call(request.clone(), Schema::default());

        let mut got = Vec::new();
        while let Some(event) = call.events.recv().await {
            got.push(event);
        }
        assert_eq!(got.len(), 3);
        assert!(matches!(&got[0], CallEvent::Headers(h) if h.len() == 1));
        assert_eq!(got[1], CallEvent::Message("{\"n\":1}".into()));
        assert!(matches!(&got[2], CallEvent::Done(o) if o.status.as_ref().unwrap().code == 0));
        assert_eq!(fake.requests(), vec![request]);
    }

    #[tokio::test]
    async fn cancel_ends_a_held_call_keeping_the_messages_received() {
        let fake = FakeGrpc::default().with_script(vec![
            FakeStep::Event(CallEvent::Message("1".into())),
            FakeStep::Event(CallEvent::Message("2".into())),
            FakeStep::HoldOpen,
        ]);
        let mut call = fake.call(GrpcRequest::default(), Schema::default());

        assert_eq!(
            call.events.recv().await,
            Some(CallEvent::Message("1".into()))
        );
        assert_eq!(
            call.events.recv().await,
            Some(CallEvent::Message("2".into()))
        );
        call.cancel();
        let Some(CallEvent::Done(outcome)) = call.events.recv().await else {
            panic!("expected Done");
        };
        assert_eq!(outcome.status.unwrap().code, 1);
        assert_eq!(outcome.messages, vec!["1", "2"]);
        assert_eq!(call.events.recv().await, None);
    }
}
