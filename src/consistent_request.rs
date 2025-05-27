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
use jsonrpsee::core::ClientError;
use parking_lot::Mutex;
use tokio::{sync::watch, task::JoinHandle};
use tracing::{error, info, warn};

use crate::{ExecutionMode, Health, Probes, RpcClient};

pub type BoxFutureResult<U> = Pin<Box<dyn Future<Output = Result<U, ClientError>> + Send + 'static>>;
pub type RequestFn<U> = Arc<dyn Fn(RpcClient) -> BoxFutureResult<U> + Send + Sync>;

/// Executes every request once against the L2 and once against the builder,
/// keeping the two in lock-step.
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
    res_rx: watch::Receiver<Option<Result<U, ClientError>>>,

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
        let (res_tx, mut res_rx) = watch::channel::<Option<Result<U, ClientError>>>(None);
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

        // background dispatcher
        let clone = manager.clone();
        tokio::spawn(async move {
            let mut attempt: Option<JoinHandle<_>> = None;

            loop {
                req_rx
                    .changed()
                    .await
                    .expect("watch channel should stay open");

                if let Some(task) = attempt.take() {
                    if !task.is_finished() {
                        error!(
                            target: "proxy::call",
                            method = clone.method,
                            "request cancelled"
                        );
                        task.abort();
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

    async fn send_with_retry_cancel_safe(
        &mut self,
        req_fn: RequestFn<U>,
        res_tx: watch::Sender<Option<Result<U, ClientError>>>,
    ) -> EyreResult<()> {
        // 1️⃣  Fire L2 call (fail-fast on error).
        let mut manager_for_l2 = self.clone();
        let res_tx_l2 = res_tx.clone();
        let req_fn_l2 = req_fn.clone();
        tokio::spawn(async move { manager_for_l2.send_to_l2(req_fn_l2, res_tx_l2).await })
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
        res_tx: watch::Sender<Option<Result<U, ClientError>>>,
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
                error!(
                    target: "proxy::call",
                    method = self.method,
                    "inconsistent responses from builder and L2"
                );

                let mut mode = self.execution_mode.lock();
                if *mode == ExecutionMode::Enabled {
                    *mode = ExecutionMode::Disabled;
                    drop(mode);

                    self.has_disabled_execution_mode
                        .store(true, Ordering::SeqCst);
                    warn!(
                        target: "proxy::call",
                        "setting execution mode to Disabled"
                    );
                    self.probes.set_health(Health::PartialContent);
                }

                bail!("failed to send request to builder: {e}");
            }
        }
    }

    /// Public API: run the same request on both clients and return
    /// the **L2** result once it’s available.
    pub async fn send(&mut self, req_fn: RequestFn<U>) -> Result<U, ClientError> {
        self.req_tx.send(Some(req_fn))?;

        self.res_rx.changed().await?;
        match self.res_rx.borrow_and_update().as_ref().unwrap() {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(format!("error sending consistent request: {e}").into()),
        }
    }
}
