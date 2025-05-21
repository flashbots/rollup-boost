use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use eyre::bail;
use http::{request, response};
use jsonrpsee::{
    core::{BoxError, http_helpers},
    http_client::{HttpBody, HttpRequest, HttpResponse},
};
use parking_lot::Mutex;
use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::{ExecutionMode, Health, HttpClient, Probes};

/// A request manager that ensures requests are sent consistently
/// across both the builder and l2 client.
#[derive(Clone, Debug)]
pub struct ConsistentRequest {
    method: String,
    l2_client: HttpClient,
    builder_client: HttpClient,
    has_disabled_execution_mode: Arc<AtomicBool>,
    req_tx: watch::Sender<Option<(request::Parts, Vec<u8>)>>,
    res_rx: watch::Receiver<Option<Result<(response::Parts, Vec<u8>), BoxError>>>,
    probes: Arc<Probes>,
    execution_mode: Arc<Mutex<ExecutionMode>>,
}

impl ConsistentRequest {
    /// Creates a new `ConsistentRequest` instance.
    pub fn new(
        method: String,
        l2_client: HttpClient,
        builder_client: HttpClient,
        probes: Arc<Probes>,
        execution_mode: Arc<Mutex<ExecutionMode>>,
    ) -> Self {
        let (req_tx, mut req_rx) = watch::channel(None);
        let (res_tx, mut res_rx) = watch::channel(None);
        req_rx.mark_unchanged();
        res_rx.mark_unchanged();

        let has_disabled_execution_mode = Arc::new(AtomicBool::new(false));

        let manager = Self {
            method,
            l2_client,
            builder_client,
            has_disabled_execution_mode,
            req_tx,
            res_rx,
            probes,
            execution_mode,
        };

        let mut clone = manager.clone();
        tokio::spawn(async move {
            let mut failed = false;
            loop {
                tokio::select! {
                _ = req_rx.changed() => {
                    let (parts, body) = {
                        req_rx.borrow_and_update()
                            .as_ref()
                            .expect("value should always be Some")
                            .clone()
                    };
                    failed = clone.try_send(parts.clone(), body.clone(), res_tx.clone()).await.is_err();
                }
                _ = async {
                        if failed {
                            tokio::time::sleep(Duration::from_millis(200)).await
                        } else {
                            std::future::pending::<()>().await
                        }
                    } => {
                    let (parts, body) = {
                        req_rx.borrow_and_update()
                            .as_ref()
                            .expect("value should always be Some")
                            .clone()
                    };
                    failed = clone.send_to_builder(parts.clone(), body.clone(), ).await.is_err();
                    }
                }
            }
        });

        manager
    }

    async fn try_send(
        &mut self,
        parts: request::Parts,
        body: Vec<u8>,
        res_sender: watch::Sender<Option<Result<(response::Parts, Vec<u8>), BoxError>>>,
    ) -> eyre::Result<()> {
        // We send the l2 request first, because we need to avoid the situation where the
        // l2 fails and the builder succeeds. If this were to happen, it would bee too dangerous to
        // return the l2 error response back to the caller, since the builder would now have an
        // invalid state.
        let l2_req = HttpRequest::from_parts(parts.clone(), HttpBody::from(body.clone()));
        let l2_res = self.l2_client.forward(l2_req, self.method.clone()).await;

        // Return the l2 response asap
        let l2_res_parts = match l2_res {
            Ok(t) => {
                let (parts, body) = t.into_parts();
                let (body_bytes, _) =
                    http_helpers::read_body(&parts.headers, body, u32::MAX).await?;
                Ok((parts, body_bytes))
            }
            Err(e) => Err(e),
        };
        let l2_res_is_ok = l2_res_parts.is_ok();
        res_sender.send(Some(l2_res_parts))?;

        if !l2_res_is_ok {
            // If the l2 request failed, we don't need to send the builder request
            return Ok(());
        }

        self.send_to_builder(parts, body).await
    }

    async fn send_to_builder(&mut self, parts: request::Parts, body: Vec<u8>) -> eyre::Result<()> {
        let builder_req = HttpRequest::from_parts(parts.clone(), HttpBody::from(body.clone()));
        let builder_res = self
            .builder_client
            .forward(builder_req, self.method.clone())
            .await;

        match builder_res {
            Ok(_) => {
                if self
                    .has_disabled_execution_mode
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    let mut execution_mode = self.execution_mode.lock();
                    *execution_mode = ExecutionMode::Enabled;
                    // Drop before aquiring health lock
                    drop(execution_mode);
                    self.has_disabled_execution_mode
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    info!(target: "proxy::call", message = "setting execution mode to Enabled");
                }
                Ok(())
            }
            Err(e) => {
                // l2 request succeeded, but builder request failed
                // This state can only be recovered from if either the builder is restarted
                // or if a retry eventually goes through.
                error!(target: "proxy::call", method = self.method, message = "inconsistent responses from builder and L2");
                let mut execution_mode = self.execution_mode.lock();
                if *execution_mode == ExecutionMode::Enabled {
                    *execution_mode = ExecutionMode::Disabled;
                    // Drop before aquiring health lock
                    drop(execution_mode);
                    self.has_disabled_execution_mode
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    warn!(target: "proxy::call", message = "setting execution mode to Disabled");
                    // This health status will likely be later set back to healthy
                    // but this should be enough to trigger a new leader election.
                    self.probes.set_health(Health::PartialContent);
                };

                bail!("failed to send request to builder: {e}");
            }
        }
    }

    // Send a request, ensuring consistent responses from both the builder and l2 client.
    pub async fn send(
        &mut self,
        parts: request::Parts,
        body_bytes: Vec<u8>,
    ) -> Result<HttpResponse, BoxError> {
        self.req_tx
            .send(Some((parts.clone(), body_bytes.clone())))?;

        self.res_rx.changed().await?;
        let res = self.res_rx.borrow_and_update();
        match res.as_ref().expect("value should always be Some") {
            Ok((parts, body)) => Ok(HttpResponse::from_parts(
                parts.to_owned(),
                HttpBody::from(body.to_owned()),
            )),
            Err(e) => Err(format!("error sending consistent request: {e}").into()),
        }
    }
}
