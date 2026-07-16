use super::outbound::WebSocketPublisher;
use crate::flashblocks::metrics::FlashblocksServiceMetrics;
use crate::{ClientResult, EngineApiExt, RpcClient};
use alloy_primitives::U256;
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
};
use alloy_rpc_types_engine::{ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use core::net::SocketAddr;
use jsonrpsee::core::async_trait;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};
use op_alloy_rpc_types_engine::{
    OpFlashblockPayload, OpFlashblockPayloadBase, OpFlashblockPayloadDelta,
};
use rollup_boost_types::payload::{NewPayload, OpExecutionPayloadEnvelope, PayloadVersion};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// op-reth uses EngineApiMessageVersion::default() to calculate the payload id instead
/// since the FCU version is not injected into the payload id. This is a bug on op-reth
/// described in https://github.com/ethereum-optimism/optimism/issues/20226
const BUILDER_PAYLOAD_ID_VERSION: u8 = 4;

/// Maximum number of flashblocks buffered while the builder's FCU response is in flight. The
/// buffer normally holds at most a handful of flashblocks for one FCU round-trip, but a stalled
/// builder response combined with a misbehaving flashblock stream must not grow it without
/// bound; anything beyond the limit is dropped.
const MAX_PENDING_FLASHBLOCKS: usize = 1024;

#[derive(Debug, Error, PartialEq)]
pub enum FlashblocksError {
    #[error("Missing base payload for initial flashblock")]
    MissingBasePayload,
    #[error("Unexpected base payload for non-initial flashblock")]
    UnexpectedBasePayload,
    #[error("Missing delta for flashblock")]
    MissingDelta,
    #[error("Invalid index for flashblock")]
    InvalidIndex,
    #[error("Missing payload")]
    MissingPayload,
    #[error("invalid authorizer signature")]
    InvalidAuthorizerSig,
}

// Simplify actor messages to just handle shutdown
#[derive(Debug)]
enum FlashblocksEngineMessage {
    OpFlashblockPayload(OpFlashblockPayload),
}

#[derive(Clone, Debug, Default)]
pub struct FlashblockBuilder {
    base: Option<OpFlashblockPayloadBase>,
    flashblocks: Vec<OpFlashblockPayloadDelta>,
}

impl FlashblockBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn extend(&mut self, payload: OpFlashblockPayload) -> Result<(), FlashblocksError> {
        tracing::debug!(message = "Extending payload", payload_id = %payload.payload_id, index = payload.index, has_base=payload.base.is_some());

        // Validate the index is contiguous
        if payload.index != self.flashblocks.len() as u64 {
            return Err(FlashblocksError::InvalidIndex);
        }

        // Check base payload rules
        if payload.index == 0 {
            if let Some(base) = payload.base {
                self.base = Some(base)
            } else {
                return Err(FlashblocksError::MissingBasePayload);
            }
        } else if payload.base.is_some() {
            return Err(FlashblocksError::UnexpectedBasePayload);
        }

        // Update latest diff and accumulate transactions and withdrawals
        self.flashblocks.push(payload.diff);

        Ok(())
    }

    pub fn into_envelope(
        self,
        version: PayloadVersion,
    ) -> Result<OpExecutionPayloadEnvelope, FlashblocksError> {
        self.build_envelope(version)
    }

    pub fn build_envelope(
        &self,
        version: PayloadVersion,
    ) -> Result<OpExecutionPayloadEnvelope, FlashblocksError> {
        let base = self.base.as_ref().ok_or(FlashblocksError::MissingPayload)?;

        // There must be at least one delta
        let diff = self
            .flashblocks
            .last()
            .ok_or(FlashblocksError::MissingDelta)?;

        let (transactions, withdrawals) = self.flashblocks.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut transactions, mut withdrawals), delta| {
                transactions.extend(delta.transactions.clone());
                withdrawals.extend(delta.withdrawals.clone());
                (transactions, withdrawals)
            },
        );

        let withdrawals_root = diff.withdrawals_root;

        let execution_payload = ExecutionPayloadV3 {
            blob_gas_used: diff.blob_gas_used.unwrap_or(0),
            excess_blob_gas: 0,
            payload_inner: ExecutionPayloadV2 {
                withdrawals,
                payload_inner: ExecutionPayloadV1 {
                    parent_hash: base.parent_hash,
                    fee_recipient: base.fee_recipient,
                    state_root: diff.state_root,
                    receipts_root: diff.receipts_root,
                    logs_bloom: diff.logs_bloom,
                    prev_randao: base.prev_randao,
                    block_number: base.block_number,
                    gas_limit: base.gas_limit,
                    gas_used: diff.gas_used,
                    timestamp: base.timestamp,
                    extra_data: base.extra_data.clone(),
                    base_fee_per_gas: base.base_fee_per_gas,
                    block_hash: diff.block_hash,
                    transactions,
                },
            },
        };

        match version {
            PayloadVersion::V3 => Ok(OpExecutionPayloadEnvelope::V3(
                OpExecutionPayloadEnvelopeV3 {
                    parent_beacon_block_root: base.parent_beacon_block_root,
                    block_value: U256::ZERO,
                    blobs_bundle: BlobsBundleV1::default(),
                    should_override_builder: false,
                    execution_payload,
                },
            )),
            PayloadVersion::V4 | PayloadVersion::V5 => Ok(OpExecutionPayloadEnvelope::V4(
                OpExecutionPayloadEnvelopeV4 {
                    parent_beacon_block_root: base.parent_beacon_block_root,
                    block_value: U256::ZERO,
                    blobs_bundle: BlobsBundleV1::default(),
                    should_override_builder: false,
                    execution_payload: OpExecutionPayloadV4 {
                        withdrawals_root,
                        payload_inner: execution_payload,
                    },
                    execution_requests: vec![],
                },
            )),
        }
    }
}

/// State of the payload building round currently in progress
#[derive(Debug, Default)]
struct PayloadBuildingState {
    /// Current payload ID we're processing. Set from local payload calculation and updated from
    /// external source.
    /// None means rollup-boost has not served FCU with attributes yet.
    payload_id: Option<PayloadId>,

    /// Whether a FCU with attributes builder response for the builder payload id is in flight
    is_pending_builder_payload: bool,

    /// If payload_id calculation changes there will be a mismatch between the current payload id
    /// and the builder payload id. A race condition where the builder FCU response returns after
    /// the first flashblock arrives at the websocket will cause the flashblock to be discarded,
    /// resulting in subsequent flashblocks unable to be extended
    /// We have a short buffer of flashblocks if the FCU builder payload id response is still in
    /// flight and there is a payload id mismatch
    pending_flashblocks: Vec<OpFlashblockPayload>,
}

#[derive(Clone, Debug)]
pub struct FlashblocksService {
    client: RpcClient,

    /// The payload building round currently in progress.
    building_state: Arc<RwLock<PayloadBuildingState>>,

    /// The current Flashblock's payload being constructed.
    best_payload: Arc<RwLock<FlashblockBuilder>>,

    /// Websocket publisher for sending valid pre-confirmations to clients.
    ws_pub: Arc<WebSocketPublisher>,

    /// Metrics
    metrics: FlashblocksServiceMetrics,

    /// Atomic to track absolute maximum number of flashblocks used is block building.
    /// This used to measures the reduction in flashblocks issued.
    max_flashblocks: Arc<AtomicU64>,
}

impl FlashblocksService {
    pub fn new(client: RpcClient, outbound_addr: SocketAddr) -> io::Result<Self> {
        let ws_pub = WebSocketPublisher::new(outbound_addr)?.into();

        Ok(Self {
            client,
            building_state: Arc::new(RwLock::new(PayloadBuildingState::default())),
            best_payload: Arc::new(RwLock::new(FlashblockBuilder::new())),
            ws_pub,
            metrics: Default::default(),
            max_flashblocks: Arc::new(AtomicU64::new(0)),
        })
    }

    pub async fn get_best_payload(
        &self,
        version: PayloadVersion,
        payload_id: PayloadId,
    ) -> Result<OpExecutionPayloadEnvelope, FlashblocksError> {
        // Check that we have flashblocks for correct payload.
        let building_state = self.building_state.read().await;
        if building_state.payload_id != Some(payload_id) {
            // We have an outdated payload id so we should fallback to get_payload
            // Clearing best_payload in here would cause situation when old `get_payload` would clear
            // currently built correct flashblocks.
            // This will self-heal on the next FCU.
            return Err(FlashblocksError::MissingPayload);
        }
        // consume the best payload and reset the builder
        let payload = {
            let mut builder = self.best_payload.write().await;
            let flashblocks_number = builder.flashblocks.len() as u64;
            let max_flashblocks = self
                .max_flashblocks
                .fetch_max(flashblocks_number, Ordering::Relaxed)
                .max(flashblocks_number);
            self.metrics
                .record_flashblocks(flashblocks_number, max_flashblocks);
            tracing::Span::current().record("flashblocks_count", flashblocks_number);
            // Take payload and place new one in its place in one go to avoid double locking
            std::mem::replace(&mut *builder, FlashblockBuilder::new()).into_envelope(version)?
        };

        Ok(payload)
    }

    pub async fn start_payload_building(&self, expected_payload_id: PayloadId) {
        tracing::debug!(message = "Starting payload building", payload_id = %expected_payload_id);
        let mut building_state = self.building_state.write().await;
        building_state.payload_id = Some(expected_payload_id);
        building_state.is_pending_builder_payload = true;
        building_state.pending_flashblocks.clear();
        // Current state won't be useful anymore because chain progressed.
        *self.best_payload.write().await = FlashblockBuilder::new();
    }

    /// Confirms the payload id for the current round from the builder's FCU response
    pub async fn confirm_payload_building(&self, builder_payload_id: Option<PayloadId>) {
        let mut building_state = self.building_state.write().await;
        building_state.is_pending_builder_payload = false;
        let pending = std::mem::take(&mut building_state.pending_flashblocks);

        let Some(payload_id) = builder_payload_id else {
            tracing::debug!(message = "Forkchoice updated with no payload ID");
            return;
        };
        if building_state.payload_id == Some(payload_id) {
            // The precomputed id was right - nothing for this id can be pending.
            tracing::debug!(message = "Forkchoice updated", payload_id = %payload_id);
            return;
        }

        tracing::debug!(
            message = "Payload id returned by builder differs from calculated. Using builder payload id",
            builder_payload_id = %payload_id,
            calculated_payload_id = %building_state.payload_id.unwrap_or_default(),
        );
        building_state.payload_id = Some(payload_id);

        // The precomputed id had mismatch, try extending with flashblocks in the pending buffer
        let mut pending: Vec<_> = pending
            .into_iter()
            .filter(|p| p.payload_id == payload_id)
            .collect();
        pending.sort_by_key(|p| p.index);

        let mut builder = FlashblockBuilder::new();
        let mut buffered = Vec::new();
        for payload in pending {
            match builder.extend(payload.clone()) {
                Ok(()) => buffered.push(payload),
                // Contiguity broken (e.g. the base flashblock was never buffered);
                // keep whatever prefix applied cleanly.
                Err(e) => {
                    debug!(
                        message = "Error extending buffered flashblocks",
                        error = %e,
                        payload_id = %payload_id,
                        index = payload.index,
                    );
                    break;
                }
            }
        }
        *self.best_payload.write().await = builder;
        drop(building_state);

        for payload in buffered {
            self.metrics.pending_flashblocks_buffered.increment(1);
            debug!(
                message = "Pending flashblock was buffered",
                payload_id = %payload_id,
                index = payload.index,
            );
            if let Err(e) = self.ws_pub.publish(&payload) {
                error!(message = "Failed to broadcast payload", error = %e);
            }
        }
    }

    async fn on_event(&mut self, event: FlashblocksEngineMessage) {
        match event {
            FlashblocksEngineMessage::OpFlashblockPayload(payload) => {
                self.metrics.messages_processed.increment(1);

                tracing::debug!(
                    message = "Received flashblock payload",
                    payload_id = %payload.payload_id,
                    index = payload.index
                );

                // Make sure the payload id matches the current payload id.
                let mut building_state = self.building_state.write().await;
                match building_state.payload_id {
                    Some(payload_id) => {
                        if payload_id != payload.payload_id {
                            self.metrics.current_payload_id_mismatch.increment(1);
                            if building_state.is_pending_builder_payload {
                                if building_state.pending_flashblocks.len()
                                    >= MAX_PENDING_FLASHBLOCKS
                                {
                                    self.metrics.pending_flashblocks_dropped.increment(1);
                                    warn!(
                                        message = "Pending flashblocks buffer full, dropping flashblock",
                                        payload_id = %payload.payload_id,
                                        local_payload_id = %payload_id,
                                        index = payload.index,
                                    );
                                    return;
                                }
                                // The builder's FCU response may still confirm this
                                // flashblock's id, so hold on to it instead of discarding
                                debug!(
                                    message = "Payload ID mismatch while FCU response in flight, buffering flashblock",
                                    payload_id = %payload.payload_id,
                                    local_payload_id = %payload_id,
                                    index = payload.index,
                                );
                                building_state.pending_flashblocks.push(payload);
                            } else {
                                debug!(
                                    message = "Payload ID mismatch, discarding flashblock",
                                    payload_id = %payload.payload_id,
                                    local_payload_id = %payload_id,
                                    index = payload.index,
                                );
                            }
                            return;
                        }
                    }
                    None => {
                        // We haven't served FCU with attributes yet, just ignore flashblocks
                        debug!(
                            message = "Received flashblocks, but no FCU with attributes was sent",
                            payload_id = %payload.payload_id,
                            index = payload.index,
                        );
                        return;
                    }
                }

                let extend_result = self.best_payload.write().await.extend(payload.clone());
                // Publishing doesn't touch round state; release the lock first
                drop(building_state);

                if let Err(e) = extend_result {
                    self.metrics.extend_payload_errors.increment(1);
                    error!(
                        message = "Failed to extend payload",
                        error = %e,
                        payload_id = %payload.payload_id,
                        index = payload.index,
                    );
                } else {
                    // Broadcast the valid message
                    if let Err(e) = self.ws_pub.publish(&payload) {
                        error!(
                            message = "Failed to broadcast payload",
                            error = %e,
                            payload_id = %payload.payload_id,
                            index = payload.index,
                        );
                    }
                }
            }
        }
    }

    pub async fn run(&mut self, mut stream: mpsc::Receiver<OpFlashblockPayload>) {
        while let Some(event) = stream.recv().await {
            self.on_event(FlashblocksEngineMessage::OpFlashblockPayload(event))
                .await;
        }
    }
}

#[async_trait]
impl EngineApiExt for FlashblocksService {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        // Calculate and set expected payload_id
        if let Some(attr) = &payload_attributes {
            let payload_id = attr.payload_id(
                &fork_choice_state.head_block_hash,
                BUILDER_PAYLOAD_ID_VERSION,
            );
            self.start_payload_building(payload_id).await;
        }

        let result = self
            .client
            .fork_choice_updated_v3(fork_choice_state, payload_attributes)
            .await;

        // The FCU response sets the canonical payload id from the builder
        let builder_payload_id = result.as_ref().ok().and_then(|resp| resp.payload_id);
        self.confirm_payload_building(builder_payload_id).await;
        result
    }

    async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus> {
        self.client.new_payload(new_payload).await
    }

    async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope> {
        // First try to get the best flashblocks payload from the builder if it exists

        match self.get_best_payload(version, payload_id).await {
            Ok(payload) => {
                info!(message = "Returning fb payload");
                // This will finalise block building in builder.
                let client = self.client.clone();
                tokio::spawn(async move {
                    if let Err(e) = client.get_payload(payload_id, version).await {
                        error!(
                            message = "Failed to send finalising getPayload to builder",
                            error = %e,
                        );
                    }
                });
                Ok(payload)
            }
            Err(e) => {
                error!(message = "Error getting fb best payload, falling back on client", error = %e);
                info!(message = "Falling back to get_payload on client", payload_id = %payload_id);
                let result = self.client.get_payload(payload_id, version).await?;
                Ok(result)
            }
        }
    }

    async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Block> {
        self.client.get_block_by_number(number, full).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{MockEngineServer, spawn_server};
    use alloy_rpc_types_engine::JwtSecret;
    use http::Uri;
    use rollup_boost_types::payload::PayloadSource;
    use std::str::FromStr;

    /// Test that we fallback to the getPayload method if the flashblocks payload is not available
    #[tokio::test]
    async fn test_flashblocks_fallback_to_get_payload() -> eyre::Result<()> {
        let builder_mock: MockEngineServer = MockEngineServer::new();
        let (_fallback_server, fallback_server_addr) = spawn_server(builder_mock.clone()).await;
        let jwt_secret = JwtSecret::random();

        let builder_auth_rpc = Uri::from_str(&format!("http://{fallback_server_addr}")).unwrap();
        let builder_client = RpcClient::new(
            builder_auth_rpc.clone(),
            jwt_secret,
            2000,
            PayloadSource::Builder,
        )?;

        let service =
            FlashblocksService::new(builder_client, "127.0.0.1:8000".parse().unwrap()).unwrap();

        // by default, builder_mock returns a valid payload always
        service
            .get_payload(PayloadId::default(), PayloadVersion::V3)
            .await?;

        let get_payload_requests_builder = builder_mock.get_payload_requests.clone();
        assert_eq!(get_payload_requests_builder.lock().len(), 1);

        Ok(())
    }

    /// Test that we don't return block from flashblocks if payload_id is different
    #[tokio::test]
    async fn test_flashblocks_different_payload_id() -> eyre::Result<()> {
        let builder_mock: MockEngineServer = MockEngineServer::new();
        let (_fallback_server, fallback_server_addr) = spawn_server(builder_mock.clone()).await;
        let jwt_secret = JwtSecret::random();

        let builder_auth_rpc = Uri::from_str(&format!("http://{fallback_server_addr}")).unwrap();
        let builder_client = RpcClient::new(
            builder_auth_rpc.clone(),
            jwt_secret,
            2000,
            PayloadSource::Builder,
        )?;

        let service =
            FlashblocksService::new(builder_client, "127.0.0.1:8001".parse().unwrap()).unwrap();

        // Some "random" payload id
        service.building_state.write().await.payload_id =
            Some(PayloadId::new([1, 1, 1, 1, 1, 1, 1, 1]));

        // We ensure that request will skip rollup-boost and serve payload from backup if payload id
        // don't match
        service
            .get_payload(PayloadId::default(), PayloadVersion::V3)
            .await?;

        let get_payload_requests_builder = builder_mock.get_payload_requests.clone();
        assert_eq!(get_payload_requests_builder.lock().len(), 1);

        Ok(())
    }

    /// A flashblock whose payload id differs from the precomputed one (e.g. the builder stamps a
    /// different payload-id version byte, or it raced ahead of the FCU response) must be buffered
    /// and adopted once the builder's FCU response confirms its payload id.
    #[tokio::test]
    async fn test_flashblock_buffered_then_adopted() -> eyre::Result<()> {
        let jwt_secret = JwtSecret::random();
        let builder_client = RpcClient::new(
            Uri::from_str("http://127.0.0.1:1").unwrap(),
            jwt_secret,
            2000,
            PayloadSource::Builder,
        )?;
        let mut service =
            FlashblocksService::new(builder_client, "127.0.0.1:0".parse().unwrap()).unwrap();

        // Same attributes hash, different version byte: locally precomputed id stamps
        // BUILDER_PAYLOAD_ID_VERSION (4), while this builder stamps 3.
        let precomputed_id = PayloadId::new([4, 1, 1, 1, 1, 1, 1, 1]);
        let builder_id = PayloadId::new([3, 1, 1, 1, 1, 1, 1, 1]);

        // FCU with attributes: precompute is set while the builder's FCU response is in flight
        service.start_payload_building(precomputed_id).await;
        assert!(
            service
                .building_state
                .read()
                .await
                .is_pending_builder_payload
        );

        // The base flashblock arrives before the FCU response, under the builder's id
        service
            .on_event(FlashblocksEngineMessage::OpFlashblockPayload(
                OpFlashblockPayload {
                    payload_id: builder_id,
                    index: 0,
                    base: Some(OpFlashblockPayloadBase::default()),
                    ..Default::default()
                },
            ))
            .await;

        // Not adopted yet: the builder id is unconfirmed
        assert_eq!(
            service
                .get_best_payload(PayloadVersion::V3, builder_id)
                .await
                .unwrap_err(),
            FlashblocksError::MissingPayload
        );

        // The builder's FCU response confirms its payload id; the buffered base is adopted
        service.confirm_payload_building(Some(builder_id)).await;
        assert!(
            !service
                .building_state
                .read()
                .await
                .is_pending_builder_payload
        );

        service
            .on_event(FlashblocksEngineMessage::OpFlashblockPayload(
                OpFlashblockPayload {
                    payload_id: builder_id,
                    index: 1,
                    ..Default::default()
                },
            ))
            .await;

        assert!(
            service
                .get_best_payload(PayloadVersion::V3, builder_id)
                .await
                .is_ok()
        );

        Ok(())
    }

    /// A forkchoice update without attributes must clear stale pending flashblocks
    #[tokio::test]
    async fn test_pending_flashblocks_reset_on_fcu_without_attributes() -> eyre::Result<()> {
        let builder_mock: MockEngineServer = MockEngineServer::new();
        let (_server, server_addr) = spawn_server(builder_mock.clone()).await;
        let jwt_secret = JwtSecret::random();
        let builder_client = RpcClient::new(
            Uri::from_str(&format!("http://{server_addr}")).unwrap(),
            jwt_secret,
            2000,
            PayloadSource::Builder,
        )?;
        let service =
            FlashblocksService::new(builder_client, "127.0.0.1:0".parse().unwrap()).unwrap();

        // A flashblock left over from a previous round
        service
            .building_state
            .write()
            .await
            .is_pending_builder_payload = true;
        service
            .building_state
            .write()
            .await
            .pending_flashblocks
            .push(OpFlashblockPayload {
                payload_id: PayloadId::new([3, 1, 1, 1, 1, 1, 1, 1]),
                index: 0,
                base: Some(OpFlashblockPayloadBase::default()),
                ..Default::default()
            });

        service
            .fork_choice_updated_v3(ForkchoiceState::default(), None)
            .await?;

        assert!(
            service
                .building_state
                .read()
                .await
                .pending_flashblocks
                .is_empty()
        );
        assert!(
            !service
                .building_state
                .read()
                .await
                .is_pending_builder_payload
        );

        Ok(())
    }

    /// The pending buffer must not grow past [`MAX_PENDING_FLASHBLOCKS`] while the builder's
    /// FCU response is in flight; overflow is dropped.
    #[tokio::test]
    async fn test_pending_flashblocks_bounded() -> eyre::Result<()> {
        let jwt_secret = JwtSecret::random();
        let builder_client = RpcClient::new(
            Uri::from_str("http://127.0.0.1:1").unwrap(),
            jwt_secret,
            2000,
            PayloadSource::Builder,
        )?;
        let mut service =
            FlashblocksService::new(builder_client, "127.0.0.1:0".parse().unwrap()).unwrap();

        service
            .start_payload_building(PayloadId::new([4, 1, 1, 1, 1, 1, 1, 1]))
            .await;

        // Fill the buffer to the limit, then deliver one more mismatching flashblock
        service.building_state.write().await.pending_flashblocks =
            vec![OpFlashblockPayload::default(); MAX_PENDING_FLASHBLOCKS];
        service
            .on_event(FlashblocksEngineMessage::OpFlashblockPayload(
                OpFlashblockPayload {
                    payload_id: PayloadId::new([3, 1, 1, 1, 1, 1, 1, 1]),
                    index: 0,
                    base: Some(OpFlashblockPayloadBase::default()),
                    ..Default::default()
                },
            ))
            .await;

        assert_eq!(
            service
                .building_state
                .read()
                .await
                .pending_flashblocks
                .len(),
            MAX_PENDING_FLASHBLOCKS
        );

        Ok(())
    }

    /// A flashblock with a mismatched payload id must never replace flashblocks already
    /// accumulated for the confirmed payload id.
    #[tokio::test]
    async fn test_mismatched_flashblock_does_not_clobber() -> eyre::Result<()> {
        let jwt_secret = JwtSecret::random();
        let builder_client = RpcClient::new(
            Uri::from_str("http://127.0.0.1:1").unwrap(),
            jwt_secret,
            2000,
            PayloadSource::Builder,
        )?;
        let mut service =
            FlashblocksService::new(builder_client, "127.0.0.1:0".parse().unwrap()).unwrap();

        let current_id = PayloadId::new([4, 1, 1, 1, 1, 1, 1, 1]);
        let other_id = PayloadId::new([4, 2, 2, 2, 2, 2, 2, 2]);

        // Confirmed round: the builder's FCU response matched the precomputed id
        service.start_payload_building(current_id).await;
        service.confirm_payload_building(Some(current_id)).await;
        service
            .on_event(FlashblocksEngineMessage::OpFlashblockPayload(
                OpFlashblockPayload {
                    payload_id: current_id,
                    index: 0,
                    base: Some(OpFlashblockPayloadBase::default()),
                    ..Default::default()
                },
            ))
            .await;

        // A flashblock for a different payload id arrives after the id is confirmed; it must be
        // discarded without touching the current round (and not buffered — it can never become
        // part of this round)
        service
            .on_event(FlashblocksEngineMessage::OpFlashblockPayload(
                OpFlashblockPayload {
                    payload_id: other_id,
                    index: 0,
                    base: Some(OpFlashblockPayloadBase::default()),
                    ..Default::default()
                },
            ))
            .await;
        assert!(
            service
                .building_state
                .read()
                .await
                .pending_flashblocks
                .is_empty()
        );

        assert!(
            service
                .get_best_payload(PayloadVersion::V3, current_id)
                .await
                .is_ok()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_flashblocks_builder() -> eyre::Result<()> {
        let mut builder = FlashblockBuilder::new();

        // Error: First payload must have a base
        let result = builder.extend(OpFlashblockPayload {
            payload_id: PayloadId::default(),
            index: 0,
            ..Default::default()
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), FlashblocksError::MissingBasePayload);

        // Ok: First payload is correct if it has base and index 0
        let result = builder.extend(OpFlashblockPayload {
            payload_id: PayloadId::default(),
            index: 0,
            base: Some(OpFlashblockPayloadBase {
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(result.is_ok());

        // Error: First payload must have index 0
        let result = builder.extend(OpFlashblockPayload {
            payload_id: PayloadId::default(),
            index: 1,
            base: Some(OpFlashblockPayloadBase {
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), FlashblocksError::UnexpectedBasePayload);

        // Error: Second payload must have a follow-up index
        let result = builder.extend(OpFlashblockPayload {
            payload_id: PayloadId::default(),
            index: 2,
            base: None,
            ..Default::default()
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), FlashblocksError::InvalidIndex);

        // Ok: Second payload has the correct index
        let result = builder.extend(OpFlashblockPayload {
            payload_id: PayloadId::default(),
            index: 1,
            base: None,
            ..Default::default()
        });
        assert!(result.is_ok());

        Ok(())
    }
}
