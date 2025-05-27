//! consistent_request.rs
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use eyre::{Result as EyreResult, bail};
use futures::Future;
use jsonrpsee::core::BoxError;
use parking_lot::Mutex;
use tokio::{sync::watch, task::JoinHandle};
use tracing::{error, info, warn};

use crate::{ExecutionMode, Health, Probes, RpcClient};

pub type BoxFutureResult<U> = Pin<Box<dyn Future<Output = Result<U, BoxError>> + Send + 'static>>;
pub type RequestFn<U> = Arc<dyn Fn(RpcClient) -> BoxFutureResult<U> + Send + Sync>;

/// A request manager that ensures requests are sent consistently
/// across both the builder and l2 client.
#[derive(Clone)]
pub struct ConsistentRequest<U>
where
    U: Clone + Send + Sync + 'static,
{
    method: String,
    l2_client: RpcClient,
    builder_client: RpcClient,
    has_disabled_execution_mode: Arc<AtomicBool>,
    req_tx: watch::Sender<Option<RequestFn<U>>>,
    res_rx: watch::Receiver<Option<Result<U, BoxError>>>,
    probes: Arc<Probes>,
    execution_mode: Arc<Mutex<ExecutionMode>>,
}

impl<U> ConsistentRequest<U>
where
    U: Clone + Send + Sync + 'static,
{
    pub fn new(
        method: String,
        l2_client: RpcClient,
        builder_client: RpcClient,
        probes: Arc<Probes>,
        execution_mode: Arc<Mutex<ExecutionMode>>,
    ) -> Self {
        let (req_tx, mut req_rx) = watch::channel::<Option<RequestFn<U>>>(None);
        let (res_tx, mut res_rx) = watch::channel::<Option<Result<U, BoxError>>>(None);
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
                    .expect("watch channel should stay open");

                if let Some(attempt) = attempt.take() {
                    if !attempt.is_finished() {
                        error!(
                            target: "proxy::call",
                            method = clone.method,
                            "request cancelled"
                        );
                        attempt.abort();
                    }
                }

                let req_fn = req_rx
                    .borrow_and_update()
                    .as_ref()
                    .expect("value is always Some")
                    .clone();

                let mut clone_for_task = clone.clone();
                let res_tx_clone = res_tx.clone();
                attempt = Some(tokio::spawn(async move {
                    clone_for_task
                        .send_with_retry_cancel_safe(req_fn, res_tx_clone)
                        .await
                }));
            }
        });

        manager
    }

    /// This function may be cancelled at any time from an incoming request.
    async fn send_with_retry_cancel_safe(
        &mut self,
        req_fn: RequestFn<U>,
        res_tx: watch::Sender<Option<Result<U, BoxError>>>,
    ) -> EyreResult<()> {
        // We send the l2 request first, because we need to avoid the situation where the
        // l2 fails and the builder succeeds. If this were to happen, it would be too dangerous to
        // return the l2 error response back to the caller, since the builder would now have an
        // invalid state. We can return early if the l2 request fails. Note we're specifically
        // spawning new tasks here to avoid any issues with cancellation. We should ensure we're
        // in a valid state at all await points.
        let mut manager_for_l2 = self.clone();
        let res_tx_clone = res_tx.clone();
        let req_fn_l2 = req_fn.clone();
        tokio::spawn(async move { manager_for_l2.send_to_l2(req_fn_l2, res_tx_clone).await })
            .await??;

        // 2️⃣  Retry builder until it works.
        loop {
            let mut manager_for_builder = self.clone();
            let req_fn_builder = req_fn.clone();

            match tokio::spawn(
                async move { manager_for_builder.send_to_builder(req_fn_builder).await },
            )
            .await?
            {
                Ok(_) => return Ok(()),
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
    }

    async fn send_to_l2(
        &mut self,
        req_fn: RequestFn<U>,
        res_tx: watch::Sender<Option<Result<U, BoxError>>>,
    ) -> EyreResult<()> {
        let l2_res = (req_fn)(self.l2_client.clone()).await;
        res_tx.send(Some(l2_res))?;
        Ok(())
    }

    async fn send_to_builder(&mut self, req_fn: RequestFn<U>) -> EyreResult<()> {
        match (req_fn)(self.builder_client.clone()).await {
            Ok(_) => {
                if self.has_disabled_execution_mode.load(Ordering::SeqCst) {
                    let mut mode = self.execution_mode.lock();
                    *mode = ExecutionMode::Enabled;
                    drop(mode);
                    self.has_disabled_execution_mode
                        .store(false, Ordering::SeqCst);
                    info!(
                        target: "proxy::call",
                        "setting execution mode to Enabled"
                    );
                }
                Ok(())
            }
            Err(e) => {
                // l2 request succeeded, but builder request failed
                // This state can only be recovered from if either the builder is restarted
                // or if a retry eventually goes through.
                error!(
                    target: "proxy::call",
                    method = self.method,
                    "inconsistent responses from builder and L2"
                );

                let mut mode = self.execution_mode.lock();
                if *mode == ExecutionMode::Enabled {
                    *mode = ExecutionMode::Disabled;
                    // Drop before aquiring health lock
                    drop(mode);

                    self.has_disabled_execution_mode
                        .store(true, Ordering::SeqCst);
                    warn!(
                        target: "proxy::call",
                        "setting execution mode to Disabled"
                    );
                    // This health status will likely be later set back to healthy
                    // but this should be enough to trigger a new leader election.
                    self.probes.set_health(Health::PartialContent);
                }

                bail!("failed to send request to builder: {e}");
            }
        }
    }

    // Send a request, ensuring consistent responses from both the builder and l2 client.
    pub async fn send(&mut self, req_fn: RequestFn<U>) -> Result<U, BoxError> {
        self.req_tx.send(Some(req_fn))?;

        self.res_rx.changed().await?;
        match self.res_rx.borrow_and_update().as_ref().unwrap() {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(format!("error sending consistent request: {e}").into()),
        }
    }
}
