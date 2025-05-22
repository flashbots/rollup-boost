use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use eyre::{bail, eyre};
use http::{request, response};
use jsonrpsee::{
    core::{BoxError, http_helpers},
    http_client::{HttpBody, HttpRequest, HttpResponse},
};
use parking_lot::Mutex;
use tokio::{sync::watch, task::JoinHandle};
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

        let clone = manager.clone();
        tokio::spawn(async move {
            let mut attempt: Option<JoinHandle<_>> = None;
            loop {
                req_rx
                    .changed()
                    .await
                    .expect("channel should always be open");

                if let Some(attempt) = attempt {
                    if !attempt.is_finished() {
                        error!(target: "proxy::call", method = clone.method, message = "request cancelled");
                        attempt.abort();
                    }
                }

                let (parts, body) = req_rx
                    .borrow_and_update()
                    .as_ref()
                    .expect("value should always be Some")
                    .clone();

                let mut clone = clone.clone();
                let res_tx_clone = res_tx.clone();
                attempt = Some(tokio::spawn(async move {
                    clone
                        .send_with_retry_cancel_safe(parts, body, res_tx_clone)
                        .await
                }));
            }
        });

        manager
    }

    /// This function may be cancelled at any time from an incoming request.
    async fn send_with_retry_cancel_safe(
        &mut self,
        parts: request::Parts,
        body: Vec<u8>,
        res_tx: watch::Sender<Option<Result<(response::Parts, Vec<u8>), BoxError>>>,
    ) -> eyre::Result<()> {
        // We send the l2 request first, because we need to avoid the situation where the
        // l2 fails and the builder succeeds. If this were to happen, it would be too dangerous to
        // return the l2 error response back to the caller, since the builder would now have an
        // invalid state. We can return early if the l2 request fails. Note we're specifically
        // spawning new tasks here to avoid any issues with cancellation. We should ensure we're
        // in a valid state at all await points.
        let mut manager = self.clone();
        let parts_clone = parts.clone();
        let body_clone = body.clone();
        let res_sender_clone = res_tx.clone();
        tokio::spawn(async move {
            manager
                .send_to_l2(parts_clone, body_clone, res_sender_clone)
                .await
        })
        .await??;

        loop {
            let mut manager = self.clone();
            let parts_clone = parts.clone();
            let body_clone = body.clone();
            match tokio::spawn(
                async move { manager.send_to_builder(parts_clone, body_clone).await },
            )
            .await?
            {
                Ok(_) => return Ok(()),
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }

    async fn send_to_l2(
        &mut self,
        parts: request::Parts,
        body: Vec<u8>,
        res_tx: watch::Sender<Option<Result<(response::Parts, Vec<u8>), BoxError>>>,
    ) -> eyre::Result<()> {
        let l2_req = HttpRequest::from_parts(parts.clone(), HttpBody::from(body.clone()));
        let l2_res = self.l2_client.forward(l2_req, self.method.clone()).await;

        // Return the l2 response asap
        match l2_res {
            Ok(t) => {
                let (parts, body) = t.into_parts();
                let (body_bytes, _) =
                    http_helpers::read_body(&parts.headers, body, u32::MAX).await?;
                res_tx.send(Some(Ok((parts, body_bytes))))?;
                Ok(())
            }
            Err(e) => {
                let msg = format!("failed to send request to l2: {e}");
                res_tx.send(Some(Err(e)))?;
                Err(eyre!(msg))
            }
        }
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
