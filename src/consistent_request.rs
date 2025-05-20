use std::sync::{Arc, atomic::AtomicBool};

use http::{request, response};
use jsonrpsee::{
    core::{BoxError, http_helpers},
    http_client::{HttpBody, HttpRequest, HttpResponse},
};
use parking_lot::Mutex;
use tokio::{sync::watch, task::JoinHandle};
use tracing::{error, warn};

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
            // Error represents that the sender was dropped
            // No need to handle
            let mut last_attempt: Option<JoinHandle<()>> = None;
            while let Ok(_) = req_rx.changed().await {
                // None only occurs during startup
                if let Some((parts, body)) = req_rx.borrow_and_update().as_ref() {
                    let mut service = clone.clone();
                    let parts = parts.clone();
                    let body = body.clone();
                    let res_sender = res_tx.clone();
                    if let Some(handle) = last_attempt {
                        if !handle.is_finished() {
                            // If the last attempt is not finished, cancel it
                            error!(target: "proxy::call", message = "aborting previous attempt to set max DA size");
                            handle.abort();
                        }
                    }
                    last_attempt = Some(tokio::spawn(async move {
                        service.try_send(parts, body, res_sender).await;
                    }));
                }
            }
        });

        manager
    }

    pub async fn send(
        &mut self,
        parts: request::Parts,
        body_bytes: Vec<u8>,
    ) -> eyre::Result<HttpResponse> {
        self.req_tx.send(Some((
            parts.clone(),
            body_bytes.clone(),
            self.method.clone(),
        )))?;

        self.res_rx.changed().await?;
        let res = self.res_rx.borrow_and_update();
        match res.as_ref().expect("value should always be Some") {
            Ok((parts, body)) => Ok(HttpResponse::from_parts(
                parts.to_owned(),
                HttpBody::from(body.to_owned()),
            )),
            Err(e) => eyre::bail!("error sending consistent request: {}", e),
        }
    }

    async fn try_send(
        &mut self,
        parts: request::Parts,
        body: Vec<u8>,
        mut res_sender: watch::Sender<Option<Result<(response::Parts, Vec<u8>), BoxError>>>,
    ) -> eyre::Result<()> {
        let mut manager = self.clone();
        let l2_req = HttpRequest::from_parts(parts.clone(), HttpBody::from(body.clone()));
        let builder_req = HttpRequest::from_parts(parts.clone(), HttpBody::from(body.clone()));

        // Send the request to the builder immediately
        let builder_handle = tokio::spawn(async move {
            manager
                .builder_client
                .forward(builder_req, manager.method)
                .await
        });

        // Await the l2 request
        let l2_res = self.l2_client.forward(l2_req, self.method).await;

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

        // Await the builder request
        let builder_res = builder_handle.await?;

        if builder_res.is_ok() != l2_res_is_ok {
            // This state is unrecoverable. Messages have already been retried at this
            // point and the only way forward is to manually restart the builder.
            error!(target: "proxy::call", message = "inconsistent miner api responses from builder and L2");
            let mut execution_mode = manager.execution_mode.lock();
            if *execution_mode == ExecutionMode::Enabled {
                *execution_mode = ExecutionMode::Disabled;
                // Drop before aquiring health lock
                drop(execution_mode);
                warn!(target: "proxy::call", message = "setting execution mode to Disabled");
                // This health status will likely be later set back to healthy
                // but this should be enough to trigger a new leader election.
                manager.probes.set_health(Health::PartialContent);
            }
        }

        Ok(())
    }
}
