use crate::client::ExecutionClient;
use crate::debug_api;
use crate::metrics::ServerMetrics;
use alloy_primitives::{Bytes, B256};
use debug_api::DebugServer;
use std::num::NonZero;
use std::sync::Arc;
use std::time::Instant;

use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, PayloadId,
    PayloadStatus,
};
use jsonrpsee::core::{async_trait, ClientError, RegisterMethodError, RpcResult};
use jsonrpsee::types::error::INVALID_REQUEST_CODE;
use jsonrpsee::types::{ErrorCode, ErrorObject};
use jsonrpsee::RpcModule;
use lru::LruCache;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};
use opentelemetry::global::{self, BoxedSpan, BoxedTracer};
use opentelemetry::trace::{Span, TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue};
use serde::{Deserialize, Serialize};

use tokio::sync::Mutex;
use tracing::{debug, error, info};

use jsonrpsee::proc_macros::rpc;

const CACHE_SIZE: usize = 100;

pub struct PayloadTraceContext {
    tracer: Arc<BoxedTracer>,
    block_hash_to_payload_ids: Arc<Mutex<LruCache<B256, Vec<PayloadId>>>>,
    payload_id_to_span: Arc<Mutex<LruCache<PayloadId, Arc<BoxedSpan>>>>,
    local_to_external_payload_ids: Arc<Mutex<LruCache<PayloadId, PayloadId>>>,
}

enum GetPayloadVersion {
    V3,
    V4,
}

impl GetPayloadVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            GetPayloadVersion::V3 => "v3",
            GetPayloadVersion::V4 => "v4",
        }
    }

    pub async fn get_payload(
        &self,
        client: &ExecutionClient,
        payload_id: PayloadId,
    ) -> RpcResult<(OpExecutionPayloadEnvelope, PayloadSource)> {
        match self {
            GetPayloadVersion::V3 => client
                .get_payload_v3(payload_id)
                .await
                .map(|(payload, source)| (OpExecutionPayloadEnvelope::V3(payload), source)),
            GetPayloadVersion::V4 => client
                .get_payload_v4(payload_id)
                .await
                .map(|(payload, source)| (OpExecutionPayloadEnvelope::V4(payload), source)),
        }
    }
}

impl Clone for GetPayloadVersion {
    fn clone(&self) -> Self {
        match self {
            GetPayloadVersion::V3 => GetPayloadVersion::V3,
            GetPayloadVersion::V4 => GetPayloadVersion::V4,
        }
    }
}

struct NewPayloadV3 {
    payload: ExecutionPayloadV3,
    versioned_hashes: Vec<B256>,
    parent_beacon_block_root: B256,
}

impl Clone for NewPayloadV3 {
    fn clone(&self) -> Self {
        NewPayloadV3 {
            payload: self.payload.clone(),
            versioned_hashes: self.versioned_hashes.clone(),
            parent_beacon_block_root: self.parent_beacon_block_root,
        }
    }
}

struct NewPayloadV4 {
    payload: OpExecutionPayloadV4,
    versioned_hashes: Vec<B256>,
    parent_beacon_block_root: B256,
    execution_requests: Vec<Bytes>,
}

impl Clone for NewPayloadV4 {
    fn clone(&self) -> Self {
        NewPayloadV4 {
            payload: self.payload.clone(),
            versioned_hashes: self.versioned_hashes.clone(),
            parent_beacon_block_root: self.parent_beacon_block_root,
            execution_requests: self.execution_requests.clone(),
        }
    }
}
enum NewPayloadVersion {
    V3(NewPayloadV3),
    V4(NewPayloadV4),
}

impl NewPayloadVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            NewPayloadVersion::V3(_) => "v3",
            NewPayloadVersion::V4(_) => "v4",
        }
    }

    pub fn execution_payload(&self) -> ExecutionPayloadV3 {
        match self {
            NewPayloadVersion::V3(payload) => payload.payload.clone(),
            NewPayloadVersion::V4(payload) => payload.payload.payload_inner.clone(),
        }
    }

    pub async fn new_payload(&self, client: &ExecutionClient) -> RpcResult<PayloadStatus> {
        match self {
            NewPayloadVersion::V3(payload) => {
                client
                    .new_payload_v3(
                        payload.payload.clone(),
                        payload.versioned_hashes.clone(),
                        payload.parent_beacon_block_root,
                    )
                    .await
            }
            NewPayloadVersion::V4(payload) => {
                client
                    .new_payload_v4(
                        payload.payload.clone(),
                        payload.versioned_hashes.clone(),
                        payload.parent_beacon_block_root,
                        payload.execution_requests.clone(),
                    )
                    .await
            }
        }
    }
}

impl Clone for NewPayloadVersion {
    fn clone(&self) -> Self {
        match self {
            NewPayloadVersion::V3(payload) => NewPayloadVersion::V3(payload.clone()),
            NewPayloadVersion::V4(payload) => NewPayloadVersion::V4(payload.clone()),
        }
    }
}

enum OpExecutionPayloadEnvelope {
    V3(OpExecutionPayloadEnvelopeV3),
    V4(OpExecutionPayloadEnvelopeV4),
}

impl OpExecutionPayloadEnvelope {
    fn execution_payload(&self) -> ExecutionPayloadV3 {
        match self {
            OpExecutionPayloadEnvelope::V3(envelope) => envelope.execution_payload.clone(),
            OpExecutionPayloadEnvelope::V4(envelope) => envelope.execution_payload.clone(),
        }
    }

    fn parent_beacon_block_root(&self) -> B256 {
        match self {
            OpExecutionPayloadEnvelope::V3(envelope) => envelope.parent_beacon_block_root,
            OpExecutionPayloadEnvelope::V4(envelope) => envelope.parent_beacon_block_root,
        }
    }
}

impl Clone for OpExecutionPayloadEnvelope {
    fn clone(&self) -> Self {
        match self {
            OpExecutionPayloadEnvelope::V3(envelope) => {
                OpExecutionPayloadEnvelope::V3(envelope.clone())
            }
            OpExecutionPayloadEnvelope::V4(envelope) => {
                OpExecutionPayloadEnvelope::V4(envelope.clone())
            }
        }
    }
}

impl PayloadTraceContext {
    fn new() -> Self {
        PayloadTraceContext {
            tracer: Arc::new(global::tracer("rollup-boost")),
            block_hash_to_payload_ids: Arc::new(Mutex::new(LruCache::new(
                NonZero::new(CACHE_SIZE).unwrap(),
            ))),
            payload_id_to_span: Arc::new(Mutex::new(LruCache::new(
                NonZero::new(CACHE_SIZE).unwrap(),
            ))),
            local_to_external_payload_ids: Arc::new(Mutex::new(LruCache::new(
                NonZero::new(CACHE_SIZE).unwrap(),
            ))),
        }
    }

    async fn store(&self, payload_id: PayloadId, parent_hash: B256, parent_span: BoxedSpan) {
        let mut store = self.payload_id_to_span.lock().await;
        store.put(payload_id, Arc::new(parent_span));
        let mut store = self.block_hash_to_payload_ids.lock().await;
        if let Some(payload_ids) = store.get_mut(&parent_hash) {
            payload_ids.push(payload_id);
        } else {
            store.put(parent_hash, vec![payload_id]);
        }
    }

    async fn retrieve_by_parent_hash(&self, parent_hash: &B256) -> Option<Vec<Arc<BoxedSpan>>> {
        let mut block_hash_to_payload_ids = self.block_hash_to_payload_ids.lock().await;
        let mut payload_id_to_span = self.payload_id_to_span.lock().await;
        block_hash_to_payload_ids
            .get(parent_hash)
            .map(move |payload_ids| {
                payload_ids
                    .clone()
                    .iter()
                    .filter_map(move |payload_id| payload_id_to_span.get(payload_id).cloned())
                    .collect()
            })
    }

    async fn retrieve_by_payload_id(&self, payload_id: &PayloadId) -> Option<Arc<BoxedSpan>> {
        let mut store = self.payload_id_to_span.lock().await;
        store.get(payload_id).cloned()
    }

    async fn remove_by_parent_hash(&self, block_hash: &B256) {
        let mut block_hash_to_payload_ids = self.block_hash_to_payload_ids.lock().await;
        let mut payload_id_to_span = self.payload_id_to_span.lock().await;
        if let Some(payload_ids) = block_hash_to_payload_ids.get_mut(block_hash) {
            for payload_id in payload_ids {
                payload_id_to_span.pop(payload_id);
            }
        }
        block_hash_to_payload_ids.pop(block_hash);
    }

    async fn store_payload_id_mapping(&self, local_id: PayloadId, external_id: PayloadId) {
        let mut local_to_external = self.local_to_external_payload_ids.lock().await;
        local_to_external.put(local_id, external_id);
    }

    async fn get_external_payload_id(&self, local_id: &PayloadId) -> Option<PayloadId> {
        let mut store = self.local_to_external_payload_ids.lock().await;
        store.get(local_id).copied()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    // Normal execution, sending all requests
    Enabled,
    // Not sending get_payload requests
    DryRun,
    // Not sending any requests
    Disabled,
}

impl ExecutionMode {
    fn is_get_payload_enabled(&self) -> bool {
        // get payload is only enabled in 'enabled' mode
        matches!(self, ExecutionMode::Enabled)
    }

    fn is_disabled(&self) -> bool {
        matches!(self, ExecutionMode::Disabled)
    }
}

#[derive(Clone)]
pub struct RollupBoostServer {
    pub l2_client: ExecutionClient,
    pub builder_client: ExecutionClient,
    pub boost_sync: bool,
    pub metrics: Option<Arc<ServerMetrics>>,
    pub payload_trace_context: Arc<PayloadTraceContext>,
    pub execution_mode: Arc<Mutex<ExecutionMode>>,
}

impl RollupBoostServer {
    pub fn new(
        l2_client: ExecutionClient,
        builder_client: ExecutionClient,
        boost_sync: bool,
        metrics: Option<Arc<ServerMetrics>>,
        initial_execution_mode: ExecutionMode,
    ) -> Self {
        Self {
            l2_client,
            builder_client,
            boost_sync,
            metrics,
            payload_trace_context: Arc::new(PayloadTraceContext::new()),
            execution_mode: Arc::new(Mutex::new(initial_execution_mode)),
        }
    }

    pub async fn start_debug_server(&self, debug_addr: &str) -> eyre::Result<()> {
        let server = DebugServer::new(self.execution_mode.clone());
        server.run(debug_addr).await?;
        Ok(())
    }
}

impl TryInto<RpcModule<()>> for RollupBoostServer {
    type Error = RegisterMethodError;

    fn try_into(self) -> Result<RpcModule<()>, Self::Error> {
        let mut module: RpcModule<()> = RpcModule::new(());
        module.merge(EngineApiServer::into_rpc(self.clone()))?;

        for method in module.method_names() {
            info!(?method, "method registered");
        }

        Ok(module)
    }
}

#[derive(Debug, Clone)]
pub enum PayloadSource {
    L2,
    Builder,
}

impl std::fmt::Display for PayloadSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayloadSource::L2 => write!(f, "l2"),
            PayloadSource::Builder => write!(f, "builder"),
        }
    }
}

#[allow(dead_code)]
impl PayloadSource {
    pub fn is_builder(&self) -> bool {
        matches!(self, PayloadSource::Builder)
    }

    pub fn is_l2(&self) -> bool {
        matches!(self, PayloadSource::L2)
    }
}

#[rpc(server, client, namespace = "engine")]
pub trait EngineApi {
    #[method(name = "forkchoiceUpdatedV3")]
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "getPayloadV3")]
    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelopeV3>;

    #[method(name = "getPayloadV4")]
    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelopeV4>;

    #[method(name = "newPayloadV3")]
    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus>;

    #[method(name = "newPayloadV4")]
    async fn new_payload_v4(
        &self,
        payload: OpExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> RpcResult<PayloadStatus>;
}

#[async_trait]
impl EngineApiServer for RollupBoostServer {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        let start = Instant::now();
        let res = self
            .fork_choice_updated_v3(fork_choice_state, payload_attributes)
            .await;
        let elapsed = start.elapsed();
        if let Some(metrics) = &self.metrics {
            metrics.fork_choice_updated_v3_total.record(elapsed);
        }
        res
    }

    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelopeV3> {
        let start = Instant::now();
        let res = self.get_payload(GetPayloadVersion::V3, payload_id).await;
        let elapsed = start.elapsed();
        if let Some(metrics) = &self.metrics {
            metrics.get_payload_v3_total.record(elapsed);
        }
        res.and_then(|envelope| match envelope {
            OpExecutionPayloadEnvelope::V3(payload) => Ok(payload),
            _ => Err(ErrorCode::InternalError.into()),
        })
    }

    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelopeV4> {
        let start = Instant::now();
        let res = self.get_payload(GetPayloadVersion::V4, payload_id).await;
        let elapsed = start.elapsed();
        if let Some(metrics) = &self.metrics {
            metrics.get_payload_v4_total.record(elapsed);
        }
        res.and_then(|envelope| match envelope {
            OpExecutionPayloadEnvelope::V4(payload) => Ok(payload),
            _ => Err(ErrorCode::InternalError.into()),
        })
    }

    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        let start = Instant::now();
        let res = self
            .new_payload(NewPayloadVersion::V3(NewPayloadV3 {
                payload,
                versioned_hashes,
                parent_beacon_block_root,
            }))
            .await;
        let elapsed = start.elapsed();
        if let Some(metrics) = &self.metrics {
            metrics.new_payload_v3_total.record(elapsed);
        }
        res
    }

    async fn new_payload_v4(
        &self,
        payload: OpExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> RpcResult<PayloadStatus> {
        let start = Instant::now();
        let res = self
            .new_payload(NewPayloadVersion::V4(NewPayloadV4 {
                payload,
                versioned_hashes,
                parent_beacon_block_root,
                execution_requests,
            }))
            .await;
        let elapsed = start.elapsed();
        if let Some(metrics) = &self.metrics {
            metrics.new_payload_v4_total.record(elapsed);
        }
        res
    }
}

impl RollupBoostServer {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        info!(
            message = "received fork_choice_updated_v3",
            "head_block_hash" = %fork_choice_state.head_block_hash,
            "has_attributes" = payload_attributes.is_some(),
        );

        // First get the local payload ID from L2 client
        let l2_response = self
            .l2_client
            .fork_choice_updated_v3(fork_choice_state, payload_attributes.clone())
            .await?;

        // TODO: Use _is_block_building_call to log the correct message during the async call to builder
        let (should_send_to_builder, _is_block_building_call) =
            if let Some(attr) = payload_attributes.as_ref() {
                // payload attributes are present. It is a FCU call to start block building
                // Do not send to builder if no_tx_pool is set, meaning that the CL node wants
                // a deterministic block without txs. We let the fallback EL node compute those.
                let use_tx_pool = !attr.no_tx_pool.unwrap_or_default();
                (use_tx_pool, true)
            } else {
                // no payload attributes. It is a FCU call to lock the head block
                // previously synced with the new_payload_v3 call. Only send to builder if boost_sync is enabled
                (self.boost_sync, false)
            };

        let execution_mode = self.execution_mode.lock().await;

        if execution_mode.is_disabled() {
            debug!(message = "execution mode is disabled, skipping FCU call to builder", "head_block_hash" = %fork_choice_state.head_block_hash);
        } else if should_send_to_builder {
            let span: Option<BoxedSpan> =
                if let (Some(payload_attributes), Some(local_payload_id)) =
                    (payload_attributes.clone(), l2_response.payload_id)
                {
                    let mut parent_span = self
                        .payload_trace_context
                        .tracer
                        .start_with_context("build-block", &Context::current());

                    parent_span.set_attribute(KeyValue::new(
                        "parent_hash",
                        fork_choice_state.head_block_hash.to_string(),
                    ));
                    parent_span.set_attribute(KeyValue::new(
                        "timestamp",
                        payload_attributes.payload_attributes.timestamp as i64,
                    ));
                    parent_span
                        .set_attribute(KeyValue::new("payload_id", local_payload_id.to_string()));
                    let ctx = Context::current()
                        .with_remote_span_context(parent_span.span_context().clone());
                    self.payload_trace_context
                        .store(
                            local_payload_id,
                            fork_choice_state.head_block_hash,
                            parent_span,
                        )
                        .await;
                    Some(
                        self.payload_trace_context
                            .tracer
                            .start_with_context("fcu", &ctx),
                    )
                } else {
                    None
                };

            // async call to builder to trigger payload building and sync
            let builder_client = self.builder_client.clone();
            let attr = payload_attributes.clone();
            let payload_trace_context = self.payload_trace_context.clone();
            let local_payload_id = l2_response.payload_id;
            tokio::spawn(async move {
                match builder_client
                    .fork_choice_updated_v3(fork_choice_state, attr)
                    .await
                {
                    Ok(response) => {
                        let external_payload_id = response.payload_id;
                        if let (Some(local_id), Some(external_id)) =
                            (local_payload_id, external_payload_id)
                        {
                            // Only store mapping if local and external IDs are different
                            if local_id != external_id {
                                payload_trace_context
                                    .store_payload_id_mapping(local_id, external_id)
                                    .await;
                            }
                        }
                        if response.is_invalid() {
                            let payload_id_str = external_payload_id
                                .map(|id| id.to_string())
                                .unwrap_or_default();
                            error!(message = "builder rejected fork_choice_updated_v3 with attributes", "url" = ?builder_client.auth_rpc, "payload_id" = payload_id_str, "validation_error" = %response.payload_status.status);
                        } else if let Some(external_id) = external_payload_id {
                            info!(
                                message = "called fork_choice_updated_v3 to builder with payload attributes",
                                "url" = ?builder_client.auth_rpc,
                                "payload_status" = %response.payload_status.status,
                                "payload_id" = %external_id
                            );
                        } else {
                            info!(
                                message = "called fork_choice_updated_v3 to builder without payload attributes",
                                "url" = ?builder_client.auth_rpc,
                                "payload_status" = %response.payload_status.status
                            );
                        }
                    }

                    Err(e) => {
                        error!(
                            message = "error calling fork_choice_updated_v3 to builder",
                            "url" = ?builder_client.auth_rpc,
                            "error" = %e,
                            "head_block_hash" = %fork_choice_state.head_block_hash
                        );
                    }
                }
                if let Some(mut s) = span {
                    s.end()
                };
            });
        } else {
            // If no payload attributes are provided, the builder will not build a block
            // We store a mapping from the local payload ID to an empty payload ID to signal
            // during get_payload_v3 request that the builder does not need to be queried.
            if let Some(local_id) = l2_response.payload_id {
                let payload_trace_context = self.payload_trace_context.clone();

                payload_trace_context
                    .store_payload_id_mapping(local_id, PayloadId::default())
                    .await;
            } else {
                error!(message = "no local payload id returned from l2 client", "head_block_hash" = %fork_choice_state.head_block_hash);
            }

            info!(message = "no payload attributes provided or no_tx_pool is set", "head_block_hash" = %fork_choice_state.head_block_hash);
        }

        Ok(l2_response)
    }

    async fn get_payload(
        &self,
        version: GetPayloadVersion,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelope> {
        let label = version.as_str();
        info!(message = format!("received get_payload_{}", label), "payload_id" = %payload_id);

        let l2_version = version.clone();
        let l2_client_future =
            async move { l2_version.get_payload(&self.l2_client, payload_id).await };

        let builder_client_future = Box::pin(async move {
            let execution_mode = self.execution_mode.lock().await;
            if !execution_mode.is_get_payload_enabled() {
                info!(message = "dry run mode is enabled, skipping get payload builder call");

                // We are in dry run mode, so we do not want to call the builder.
                return Err(ClientError::Call(ErrorObject::owned(
                    INVALID_REQUEST_CODE,
                    "Dry run mode is enabled",
                    None::<String>,
                )));
            }

            let parent_span = self
                .payload_trace_context
                .retrieve_by_payload_id(&payload_id)
                .await;
            let span = parent_span.clone().map(|span| {
                self.payload_trace_context.tracer.start_with_context(
                    "get_payload",
                    &Context::current().with_remote_span_context(span.span_context().clone()),
                )
            });

            // Get the external builder's payload ID that corresponds to our local payload ID
            // If no mapping exists, fallback to local ID
            let external_payload_id = self
                .payload_trace_context
                .get_external_payload_id(&payload_id)
                .await
                .unwrap_or(payload_id);

            if external_payload_id == PayloadId::default() {
                info!(
                    message =
                        "no-tx-pool call and builder did not build a block, defer to L2 result"
                );

                // Note: We are sending an error here to return early from the future and this error
                // is not logged later on, so it's not causing issues. However, we should find a better
                // way to handle this case in the future rather than relying on this error being silently
                // ignored.
                return Err(ClientError::Call(ErrorObject::owned(
                    INVALID_REQUEST_CODE,
                    "Builder payload was not valid",
                    None::<String>,
                )));
            }

            let builder = self.builder_client.clone();
            let (payload, source) = version.get_payload(&builder, external_payload_id).await.map_err(|e| {
                error!(message = "error calling get_payload from builder", "url" = ?builder.auth_rpc, "error" = %e, "local_payload_id" = %payload_id, "external_payload_id" = %external_payload_id);
                e
            })?;

            let block_hash =
                ExecutionPayload::from(payload.clone().execution_payload()).block_hash();
            info!(message = "received payload from builder", "local_payload_id" = %payload_id, "external_payload_id" = %external_payload_id, "block_hash" = %block_hash);

            // Send the payload to the local execution engine with engine_newPayload to validate the block from the builder.
            // Otherwise, we do not want to risk the network to a halt since op-node will not be able to propose the block.
            // If validation fails, return the local block since that one has already been validated.
            let l2_call = match version {
                GetPayloadVersion::V3 => self.l2_client.auth_client.new_payload_v3(
                    payload.execution_payload(),
                    vec![],
                    payload.parent_beacon_block_root(),
                ),
                GetPayloadVersion::V4 => self.l2_client.auth_client.new_payload_v4(
                    OpExecutionPayloadV4 {
                        payload_inner: payload.execution_payload(),
                        withdrawals_root: B256::ZERO,
                    },
                    vec![],
                    payload.parent_beacon_block_root(),
                    vec![],
                ),
            };
            let payload_status = l2_call.await.map_err(|e| {
                error!(message = format!("error calling new_payload_{} to validate builder payload", label), "url" = ?self.l2_client.auth_rpc, "error" = %e, "local_payload_id" = %payload_id, "external_payload_id" = %external_payload_id);
                e
            })?;
            if let Some(mut s) = span {
                s.end();
            };
            if let Some(mut parent) = parent_span {
                let parent = Arc::get_mut(&mut parent);
                if let Some(parent) = parent {
                    parent.end();
                }
            };

            if payload_status.is_invalid() {
                error!(message = "builder payload was not valid", "url" = ?builder.auth_rpc, "payload_status" = %payload_status.status, "local_payload_id" = %payload_id, "external_payload_id" = %external_payload_id);
                Err(ClientError::Call(ErrorObject::owned(
                    INVALID_REQUEST_CODE,
                    "Builder payload was not valid",
                    None::<String>,
                )))
            } else {
                info!(message = "received payload status from local execution engine validating builder payload", "local_payload_id" = %payload_id, "external_payload_id" = %external_payload_id);
                Ok((payload, source))
            }
        });

        let (l2_payload, builder_payload) = tokio::join!(l2_client_future, builder_client_future);
        let payload = match (builder_payload, l2_payload) {
            (Ok(builder), _) => Ok(builder),
            (Err(_), Ok(l2)) => Ok(l2),
            (Err(_), Err(e)) => Err(e),
        };
        payload.map(|(payload, context)| {
            let inner_payload = ExecutionPayload::from(payload.clone().execution_payload());
            let block_hash = inner_payload.block_hash();
            let block_number = inner_payload.block_number();

            if let Some(metrics) = &self.metrics {
                metrics.increment_blocks_created(&context);
            }

            // Note: This log message is used by integration tests to track payload context.
            // While not ideal to rely on log parsing, it provides a reliable way to verify behavior.
            // Happy to consider an alternative approach later on.
            info!(
                message = "returning block",
                "hash" = %block_hash,
                "number" = %block_number,
                "context" = %context,
                "payload_id" = %payload_id
            );

            payload
        })
    }

    async fn new_payload(&self, version: NewPayloadVersion) -> RpcResult<PayloadStatus> {
        let label = version.as_str();
        let execution_payload = ExecutionPayload::from(version.execution_payload());
        let block_hash = execution_payload.block_hash();
        let parent_hash = execution_payload.parent_hash();
        info!(message = format!("received new_payload_{}", label), "block_hash" = %block_hash);
        // async call to builder to sync the builder node
        let execution_mode = self.execution_mode.lock().await;
        if self.boost_sync && !execution_mode.is_disabled() {
            let parent_spans = self
                .payload_trace_context
                .retrieve_by_parent_hash(&parent_hash)
                .await;
            let spans: Option<Vec<BoxedSpan>> = parent_spans.as_ref().map(|spans| {
                spans
                    .iter()
                    .map(|span| {
                        self.payload_trace_context.tracer.start_with_context(
                            "new_payload",
                            &Context::current()
                                .with_remote_span_context(span.span_context().clone()),
                        )
                    })
                    .collect()
            });
            self.payload_trace_context
                .remove_by_parent_hash(&parent_hash)
                .await;

            let builder = self.builder_client.clone();
            let builder_version = version.clone();
            tokio::spawn(async move {
                let new_payload_response = builder_version.new_payload(&builder).await;
                let _ = new_payload_response
                .map(|response: PayloadStatus| {
                    if response.is_invalid() {
                        error!(message = format!("builder rejected new_payload_{}", label), "url" = ?builder.auth_rpc, "block_hash" = %block_hash);
                    } else {
                        info!(message = format!("called new_payload_{} to builder", label), "url" = ?builder.auth_rpc, "payload_status" = %response.status, "block_hash" = %block_hash);
                    }
                }).map_err(|e| {
                    error!(message = format!("error calling new_payload_{} to builder", label), "url" = ?builder.auth_rpc, "error" = %e, "block_hash" = %block_hash);
                    e
                });
                if let Some(mut spans) = spans {
                    spans.iter_mut().for_each(|s| s.end());
                };
            });
        }

        version.new_payload(&self.l2_client).await
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use alloy_primitives::hex;
    use alloy_primitives::{FixedBytes, U256};
    use alloy_rpc_types_engine::{
        BlobsBundleV1, ExecutionPayloadV1, ExecutionPayloadV2, PayloadStatusEnum,
    };

    use alloy_rpc_types_engine::JwtSecret;
    use http::Uri;
    use jsonrpsee::http_client::HttpClient;
    use jsonrpsee::server::{ServerBuilder, ServerHandle};
    use jsonrpsee::RpcModule;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::time::sleep;

    const HOST: &str = "0.0.0.0";
    const L2_PORT: u16 = 8545;
    const L2_ADDR: &str = "127.0.0.1:8545";
    const BUILDER_PORT: u16 = 8544;
    const BUILDER_ADDR: &str = "127.0.0.1:8544";
    const SERVER_ADDR: &str = "0.0.0.0:8556";

    #[derive(Debug, Clone)]
    pub struct MockEngineServer {
        fcu_requests: Arc<Mutex<Vec<(ForkchoiceState, Option<OpPayloadAttributes>)>>>,
        get_payload_requests: Arc<Mutex<Vec<PayloadId>>>,
        get_payload_v4_requests: Arc<Mutex<Vec<PayloadId>>>,
        new_payload_requests: Arc<Mutex<Vec<(ExecutionPayloadV3, Vec<B256>, B256)>>>,
        new_payload_v4_requests: Arc<Mutex<Vec<(ExecutionPayloadV3, Vec<B256>, B256, Vec<Bytes>)>>>,
        fcu_response: RpcResult<ForkchoiceUpdated>,
        get_payload_response: RpcResult<OpExecutionPayloadEnvelopeV3>,
        get_payload_v4_response: RpcResult<OpExecutionPayloadEnvelopeV4>,
        new_payload_response: RpcResult<PayloadStatus>,
        new_payload_v4_response: RpcResult<PayloadStatus>,

        pub override_payload_id: Option<PayloadId>,
    }

    impl MockEngineServer {
        pub fn new() -> Self {
            let execution_payload = ExecutionPayloadV3 {
                payload_inner: ExecutionPayloadV2 {
                    payload_inner: ExecutionPayloadV1 {
                        base_fee_per_gas:  U256::from(7u64),
                        block_number: 0xa946u64,
                        block_hash: hex!("a5ddd3f286f429458a39cafc13ffe89295a7efa8eb363cf89a1a4887dbcf272b").into(),
                        logs_bloom: hex!("00200004000000000000000080000000000200000000000000000000000000000000200000000000000000000000000000000000800000000200000000000000000000000000000000000008000000200000000000000000000001000000000000000000000000000000800000000000000000000100000000000030000000000000000040000000000000000000000000000000000800080080404000000000000008000000000008200000000000200000000000000000000000000000000000000002000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000100000000000000000000").into(),
                        extra_data: hex!("d883010d03846765746888676f312e32312e31856c696e7578").into(),
                        gas_limit: 0x1c9c380,
                        gas_used: 0x1f4a9,
                        timestamp: 0x651f35b8,
                        fee_recipient: hex!("f97e180c050e5ab072211ad2c213eb5aee4df134").into(),
                        parent_hash: hex!("d829192799c73ef28a7332313b3c03af1f2d5da2c36f8ecfafe7a83a3bfb8d1e").into(),
                        prev_randao: hex!("753888cc4adfbeb9e24e01c84233f9d204f4a9e1273f0e29b43c4c148b2b8b7e").into(),
                        receipts_root: hex!("4cbc48e87389399a0ea0b382b1c46962c4b8e398014bf0cc610f9c672bee3155").into(),
                        state_root: hex!("017d7fa2b5adb480f5e05b2c95cb4186e12062eed893fc8822798eed134329d1").into(),
                        transactions: vec![],
                    },
                    withdrawals: vec![],
                },
                blob_gas_used: 0xc0000,
                excess_blob_gas: 0x580000,
            };

            let base_payload_envelope = OpExecutionPayloadEnvelopeV3 {
                execution_payload: execution_payload.clone(),
                block_value: U256::from(0),
                blobs_bundle: BlobsBundleV1 {
                    commitments: vec![],
                    proofs: vec![],
                    blobs: vec![],
                },
                should_override_builder: false,
                parent_beacon_block_root: B256::ZERO,
            };

            Self {
                fcu_requests: Arc::new(Mutex::new(vec![])),
                get_payload_requests: Arc::new(Mutex::new(vec![])),
                get_payload_v4_requests: Arc::new(Mutex::new(vec![])),
                new_payload_requests: Arc::new(Mutex::new(vec![])),
                new_payload_v4_requests: Arc::new(Mutex::new(vec![])),
                fcu_response: Ok(ForkchoiceUpdated::new(PayloadStatus::from_status(
                    PayloadStatusEnum::Valid,
                ))),
                get_payload_response: Ok(base_payload_envelope.clone()),
                get_payload_v4_response: Ok(OpExecutionPayloadEnvelopeV4 {
                    execution_payload: base_payload_envelope.execution_payload.clone(),
                    block_value: base_payload_envelope.block_value,
                    blobs_bundle: base_payload_envelope.blobs_bundle.clone(),
                    should_override_builder: base_payload_envelope.should_override_builder,
                    parent_beacon_block_root: base_payload_envelope.parent_beacon_block_root,
                    execution_requests: vec![],
                }),
                override_payload_id: None,
                new_payload_response: Ok(PayloadStatus::from_status(PayloadStatusEnum::Valid)),
                new_payload_v4_response: Ok(PayloadStatus::from_status(PayloadStatusEnum::Valid)),
            }
        }
    }

    struct TestHarness {
        l2_server: ServerHandle,
        l2_mock: MockEngineServer,
        builder_server: ServerHandle,
        builder_mock: MockEngineServer,
        proxy_server: ServerHandle,
        client: HttpClient,
    }

    impl TestHarness {
        async fn new(
            boost_sync: bool,
            l2_mock: Option<MockEngineServer>,
            builder_mock: Option<MockEngineServer>,
        ) -> Self {
            let jwt_secret = JwtSecret::random();

            let l2_auth_rpc = Uri::from_str(&format!("http://{}:{}", HOST, L2_PORT)).unwrap();
            let l2_client =
                ExecutionClient::new(l2_auth_rpc, jwt_secret, 2000, None, PayloadSource::L2)
                    .unwrap();

            let builder_auth_rpc =
                Uri::from_str(&format!("http://{}:{}", HOST, BUILDER_PORT)).unwrap();
            let builder_client = ExecutionClient::new(
                builder_auth_rpc,
                jwt_secret,
                2000,
                None,
                PayloadSource::Builder,
            )
            .unwrap();

            let rollup_boost_client = RollupBoostServer::new(
                l2_client,
                builder_client,
                boost_sync,
                None,
                ExecutionMode::Enabled,
            );

            let module: RpcModule<()> = rollup_boost_client.try_into().unwrap();

            let proxy_server = ServerBuilder::default()
                .build("0.0.0.0:8556".parse::<SocketAddr>().unwrap())
                .await
                .unwrap()
                .start(module);
            let l2_mock = l2_mock.unwrap_or(MockEngineServer::new());
            let builder_mock = builder_mock.unwrap_or(MockEngineServer::new());
            let l2_server = spawn_server(l2_mock.clone(), L2_ADDR).await;
            let builder_server = spawn_server(builder_mock.clone(), BUILDER_ADDR).await;
            TestHarness {
                l2_server,
                l2_mock,
                builder_server,
                builder_mock,
                proxy_server,
                client: HttpClient::builder()
                    .build(format!("http://{SERVER_ADDR}"))
                    .unwrap(),
            }
        }

        async fn cleanup(self) {
            self.l2_server.stop().unwrap();
            self.l2_server.stopped().await;
            self.builder_server.stop().unwrap();
            self.builder_server.stopped().await;
            self.proxy_server.stop().unwrap();
            self.proxy_server.stopped().await;
        }
    }

    #[tokio::test]
    async fn test_server() {
        engine_success().await;
        boost_sync_enabled().await;
        builder_payload_err().await;
        test_local_external_payload_ids_different().await;
        test_local_external_payload_ids_same().await;
    }

    async fn engine_success() {
        let test_harness = TestHarness::new(false, None, None).await;

        // test fork_choice_updated_v3 success
        let fcu = ForkchoiceState {
            head_block_hash: FixedBytes::random(),
            safe_block_hash: FixedBytes::random(),
            finalized_block_hash: FixedBytes::random(),
        };
        let fcu_response = test_harness.client.fork_choice_updated_v3(fcu, None).await;
        assert!(fcu_response.is_ok());
        let fcu_requests = test_harness.l2_mock.fcu_requests.clone();
        let fcu_requests_mu = fcu_requests.lock().unwrap();
        let fcu_requests_builder = test_harness.builder_mock.fcu_requests.clone();
        let fcu_requests_builder_mu = fcu_requests_builder.lock().unwrap();
        assert_eq!(fcu_requests_mu.len(), 1);
        assert_eq!(fcu_requests_builder_mu.len(), 0);
        let req: &(ForkchoiceState, Option<OpPayloadAttributes>) = fcu_requests_mu.first().unwrap();
        assert_eq!(req.0, fcu);
        assert_eq!(req.1, None);

        // test new_payload_v3 success
        let new_payload_response = test_harness
            .client
            .new_payload_v3(
                test_harness
                    .l2_mock
                    .get_payload_response
                    .clone()
                    .unwrap()
                    .execution_payload
                    .clone(),
                vec![],
                B256::ZERO,
            )
            .await;
        assert!(new_payload_response.is_ok());
        let new_payload_requests = test_harness.l2_mock.new_payload_requests.clone();
        let new_payload_requests_mu = new_payload_requests.lock().unwrap();
        let new_payload_requests_builder = test_harness.builder_mock.new_payload_requests.clone();
        let new_payload_requests_builder_mu = new_payload_requests_builder.lock().unwrap();
        assert_eq!(new_payload_requests_mu.len(), 1);
        assert_eq!(new_payload_requests_builder_mu.len(), 0);
        let req: &(ExecutionPayloadV3, Vec<FixedBytes<32>>, B256) =
            new_payload_requests_mu.first().unwrap();
        assert_eq!(
            req.0,
            test_harness
                .l2_mock
                .get_payload_response
                .clone()
                .unwrap()
                .execution_payload
                .clone()
        );
        assert_eq!(req.1, Vec::<FixedBytes<32>>::new());
        assert_eq!(req.2, B256::ZERO);
        drop(new_payload_requests_mu);

        // test new_payload_v4 success
        let new_payload_response = test_harness
            .client
            .new_payload_v4(
                OpExecutionPayloadV4 {
                    payload_inner: test_harness
                        .l2_mock
                        .get_payload_v4_response
                        .clone()
                        .unwrap()
                        .execution_payload
                        .clone(),
                    withdrawals_root: B256::ZERO,
                },
                vec![],
                B256::ZERO,
                vec![],
            )
            .await;
        if let Err(ref err) = new_payload_response {
            println!("Test encountered error: {:?}", err);
        }
        assert!(new_payload_response.is_ok());
        let new_payload_requests = test_harness.l2_mock.new_payload_v4_requests.clone();
        let new_payload_requests_mu = new_payload_requests.lock().unwrap();
        let new_payload_requests_builder =
            test_harness.builder_mock.new_payload_v4_requests.clone();
        let new_payload_requests_builder_mu = new_payload_requests_builder.lock().unwrap();
        assert_eq!(new_payload_requests_mu.len(), 1);
        assert_eq!(new_payload_requests_builder_mu.len(), 0);
        let req: &(ExecutionPayloadV3, Vec<FixedBytes<32>>, B256, Vec<Bytes>) =
            new_payload_requests_mu.first().unwrap();
        assert_eq!(
            req.0,
            test_harness
                .l2_mock
                .get_payload_v4_response
                .clone()
                .unwrap()
                .execution_payload
                .clone()
        );
        assert_eq!(req.1, Vec::<FixedBytes<32>>::new());
        assert_eq!(req.2, B256::ZERO);
        assert_eq!(req.3, Vec::<Bytes>::new());
        drop(new_payload_requests_mu);

        // test get_payload_v3 success
        let get_payload_response = test_harness
            .client
            .get_payload_v3(PayloadId::new([0, 0, 0, 0, 0, 0, 0, 1]))
            .await;
        assert!(get_payload_response.is_ok());
        let get_payload_requests = test_harness.l2_mock.get_payload_requests.clone();
        let get_payload_requests_mu = get_payload_requests.lock().unwrap();
        let get_payload_requests_builder = test_harness.builder_mock.get_payload_requests.clone();
        let get_payload_requests_builder_mu = get_payload_requests_builder.lock().unwrap();
        let new_payload_requests = test_harness.l2_mock.new_payload_requests.clone();
        let new_payload_requests_mu = new_payload_requests.lock().unwrap();
        assert_eq!(get_payload_requests_builder_mu.len(), 1);
        assert_eq!(get_payload_requests_mu.len(), 1);
        assert_eq!(new_payload_requests_mu.len(), 2);
        let req: &PayloadId = get_payload_requests_mu.first().unwrap();
        assert_eq!(*req, PayloadId::new([0, 0, 0, 0, 0, 0, 0, 1]));

        // test get_payload_v4 success
        let get_payload_response = test_harness
            .client
            .get_payload_v4(PayloadId::new([0, 0, 0, 0, 0, 0, 1, 0]))
            .await;
        assert!(get_payload_response.is_ok());
        let get_payload_requests = test_harness.l2_mock.get_payload_v4_requests.clone();
        let get_payload_requests_mu = get_payload_requests.lock().unwrap();
        let get_payload_requests_builder =
            test_harness.builder_mock.get_payload_v4_requests.clone();
        let get_payload_requests_builder_mu = get_payload_requests_builder.lock().unwrap();
        let new_payload_requests = test_harness.l2_mock.new_payload_v4_requests.clone();
        let new_payload_requests_mu = new_payload_requests.lock().unwrap();
        assert_eq!(get_payload_requests_builder_mu.len(), 1);
        assert_eq!(get_payload_requests_mu.len(), 1);
        assert_eq!(new_payload_requests_mu.len(), 2);
        let req: &PayloadId = get_payload_requests_mu.first().unwrap();
        assert_eq!(*req, PayloadId::new([0, 0, 0, 0, 0, 0, 1, 0]));

        test_harness.cleanup().await;
    }

    async fn boost_sync_enabled() {
        let test_harness = TestHarness::new(true, None, None).await;

        let fcu = ForkchoiceState {
            head_block_hash: FixedBytes::random(),
            safe_block_hash: FixedBytes::random(),
            finalized_block_hash: FixedBytes::random(),
        };
        let fcu_response = test_harness.client.fork_choice_updated_v3(fcu, None).await;
        assert!(fcu_response.is_ok());

        sleep(std::time::Duration::from_millis(100)).await;

        let fcu_requests = test_harness.l2_mock.fcu_requests.clone();
        let fcu_requests_mu = fcu_requests.lock().unwrap();
        let fcu_requests_builder = test_harness.builder_mock.fcu_requests.clone();
        let fcu_requests_builder_mu = fcu_requests_builder.lock().unwrap();
        assert_eq!(fcu_requests_mu.len(), 1);
        assert_eq!(fcu_requests_builder_mu.len(), 1);

        // test new_payload_v3 success
        let new_payload_response = test_harness
            .client
            .new_payload_v3(
                test_harness
                    .l2_mock
                    .get_payload_response
                    .clone()
                    .unwrap()
                    .execution_payload
                    .clone(),
                vec![],
                B256::ZERO,
            )
            .await;
        assert!(new_payload_response.is_ok());
        let new_payload_requests = test_harness.l2_mock.new_payload_requests.clone();
        let new_payload_requests_mu = new_payload_requests.lock().unwrap();
        let new_payload_requests_builder = test_harness.builder_mock.new_payload_requests.clone();
        let new_payload_requests_builder_mu = new_payload_requests_builder.lock().unwrap();
        assert_eq!(new_payload_requests_mu.len(), 1);
        assert_eq!(new_payload_requests_builder_mu.len(), 1);

        test_harness.cleanup().await;
    }

    async fn builder_payload_err() {
        let mut l2_mock = MockEngineServer::new();
        l2_mock.new_payload_response = l2_mock.new_payload_response.clone().map(|mut status| {
            status.status = PayloadStatusEnum::Invalid {
                validation_error: "test".to_string(),
            };
            status
        });
        l2_mock.get_payload_response = l2_mock.get_payload_response.clone().map(|mut payload| {
            payload.block_value = U256::from(10);
            payload
        });
        let test_harness = TestHarness::new(true, Some(l2_mock), None).await;

        // test get_payload_v3 return l2 payload if builder payload is invalid
        let get_payload_response = test_harness
            .client
            .get_payload_v3(PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0]))
            .await;
        assert!(get_payload_response.is_ok());
        assert_eq!(get_payload_response.unwrap().block_value, U256::from(10));

        test_harness.cleanup().await;
    }

    async fn spawn_server(mock_engine_server: MockEngineServer, addr: &str) -> ServerHandle {
        let server = ServerBuilder::default().build(addr).await.unwrap();
        let mut module: RpcModule<()> = RpcModule::new(());

        module
            .register_method("engine_forkchoiceUpdatedV3", move |params, _, _| {
                let params: (ForkchoiceState, Option<OpPayloadAttributes>) = params.parse()?;
                let mut fcu_requests = mock_engine_server.fcu_requests.lock().unwrap();
                fcu_requests.push(params);

                let mut response = mock_engine_server.fcu_response.clone();
                if let Ok(ref mut fcu_response) = response {
                    if let Some(override_id) = mock_engine_server.override_payload_id {
                        fcu_response.payload_id = Some(override_id);
                    }
                }

                response
            })
            .unwrap();

        module
            .register_method("engine_getPayloadV3", move |params, _, _| {
                let params: (PayloadId,) = params.parse()?;
                let mut get_payload_requests =
                    mock_engine_server.get_payload_requests.lock().unwrap();
                get_payload_requests.push(params.0);

                mock_engine_server.get_payload_response.clone()
            })
            .unwrap();

        module
            .register_method("engine_getPayloadV4", move |params, _, _| {
                let params: (PayloadId,) = params.parse()?;
                let mut get_payload_requests =
                    mock_engine_server.get_payload_v4_requests.lock().unwrap();
                get_payload_requests.push(params.0);

                mock_engine_server.get_payload_v4_response.clone()
            })
            .unwrap();

        module
            .register_method("engine_newPayloadV3", move |params, _, _| {
                let params: (ExecutionPayloadV3, Vec<B256>, B256) = params.parse()?;
                let mut new_payload_requests =
                    mock_engine_server.new_payload_requests.lock().unwrap();
                new_payload_requests.push(params);

                mock_engine_server.new_payload_response.clone()
            })
            .unwrap();

        module
            .register_method("engine_newPayloadV4", move |params, _, _| {
                let params: (ExecutionPayloadV3, Vec<B256>, B256, Vec<Bytes>) = params.parse()?;
                let mut new_payload_requests =
                    mock_engine_server.new_payload_v4_requests.lock().unwrap();
                new_payload_requests.push(params);

                mock_engine_server.new_payload_v4_response.clone()
            })
            .unwrap();

        server.start(module)
    }

    async fn test_local_external_payload_ids_same() {
        let same_id = PayloadId::new([0, 0, 0, 0, 0, 0, 0, 42]);

        let mut l2_mock = MockEngineServer::new();
        l2_mock.fcu_response = Ok(ForkchoiceUpdated::new(PayloadStatus::from_status(
            PayloadStatusEnum::Valid,
        ))
        .with_payload_id(same_id));

        let mut builder_mock = MockEngineServer::new();
        builder_mock.override_payload_id = Some(same_id);

        let test_harness =
            TestHarness::new(true, Some(l2_mock.clone()), Some(builder_mock.clone())).await;

        // Test FCU call
        let fcu = ForkchoiceState {
            head_block_hash: FixedBytes::random(),
            safe_block_hash: FixedBytes::random(),
            finalized_block_hash: FixedBytes::random(),
        };
        let fcu_response = test_harness.client.fork_choice_updated_v3(fcu, None).await;
        assert!(fcu_response.is_ok());

        // wait for builder to observe the FCU call
        sleep(std::time::Duration::from_millis(100)).await;

        let builder_fcu_req = builder_mock.fcu_requests.lock().unwrap();
        assert_eq!(builder_fcu_req.len(), 1);
        assert_eq!(l2_mock.fcu_requests.lock().unwrap().len(), 1);

        // Test getPayload call
        let get_res = test_harness.client.get_payload_v3(same_id).await;
        assert!(get_res.is_ok());

        // wait for builder to observe the getPayload call
        sleep(std::time::Duration::from_millis(100)).await;

        let builder_gp_reqs = builder_mock.get_payload_requests.lock().unwrap();
        assert_eq!(builder_gp_reqs.len(), 1);
        assert_eq!(builder_gp_reqs[0], same_id);

        let local_gp_reqs = l2_mock.get_payload_requests.lock().unwrap();
        assert_eq!(local_gp_reqs.len(), 1);
        assert_eq!(local_gp_reqs[0], same_id);

        test_harness.cleanup().await;
    }

    async fn test_local_external_payload_ids_different() {
        let local_id = PayloadId::new([1, 2, 3, 4, 5, 6, 7, 8]);
        let external_id = PayloadId::new([9, 9, 9, 9, 9, 9, 9, 9]);

        let mut l2_mock = MockEngineServer::new();
        let mut fcu_resp =
            ForkchoiceUpdated::new(PayloadStatus::from_status(PayloadStatusEnum::Valid));
        fcu_resp.payload_id = Some(local_id);
        l2_mock.fcu_response = Ok(fcu_resp);

        let mut builder_mock = MockEngineServer::new();
        builder_mock.override_payload_id = Some(external_id);

        let test_harness =
            TestHarness::new(true, Some(l2_mock.clone()), Some(builder_mock.clone())).await;

        // Test FCU call
        let fcu = ForkchoiceState {
            head_block_hash: B256::random(),
            safe_block_hash: B256::random(),
            finalized_block_hash: B256::random(),
        };
        let fcu_response = test_harness.client.fork_choice_updated_v3(fcu, None).await;
        assert!(fcu_response.is_ok());

        // wait for builder to observe the FCU call
        sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(l2_mock.fcu_requests.lock().unwrap().len(), 1);
        assert_eq!(builder_mock.fcu_requests.lock().unwrap().len(), 1);

        // Test getPayload call with local->external mapping
        let get_res = test_harness.client.get_payload_v3(local_id).await;
        assert!(get_res.is_ok(), "getPayload should succeed");

        // wait for builder to observe the getPayload call
        sleep(std::time::Duration::from_millis(100)).await;

        let builder_gp = builder_mock.get_payload_requests.lock().unwrap();
        assert_eq!(builder_gp.len(), 1);
        assert_eq!(builder_gp[0], external_id);

        let l2_gp = l2_mock.get_payload_requests.lock().unwrap();
        assert_eq!(l2_gp.len(), 1);
        assert_eq!(l2_gp[0], local_id);

        test_harness.cleanup().await;
    }
}
