use crate::common::proxy::ProxyHandler;
use futures::FutureExt;
use serde_json::Value;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Create a dynamic handler that delays all the calls by 2 seconds
pub struct DelayHandler {
    pub delay: Arc<Mutex<Duration>>,
}

impl DelayHandler {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay: Arc::new(Mutex::new(delay)),
        }
    }
}

impl ProxyHandler for DelayHandler {
    fn handle(
        &self,
        _method: String,
        _params: Value,
        _result: Value,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send>> {
        let delay = *self.delay.lock().unwrap();
        async move {
            tokio::time::sleep(delay).await;
            None
        }
        .boxed()
    }
}
