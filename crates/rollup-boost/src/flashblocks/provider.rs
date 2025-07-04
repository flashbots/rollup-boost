use super::primitives::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashblocksPayloadV1,
};
use crate::RpcClient;
use crate::{
    ClientResult, EngineApiExt, NewPayload, OpExecutionPayloadEnvelope, PayloadVersion,
    payload_id_optimism,
};
use alloy_primitives::U256;
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
};
use alloy_rpc_types_engine::{ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use jsonrpsee::core::async_trait;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Utf8Bytes;
use tracing::error;

pub struct FlashblocksProvider {
    builder_client: RpcClient,
    payload_id: Arc<Mutex<PayloadId>>,
    payload_builder: Arc<Mutex<FlashblockBuilder>>,
    stream_handle: JoinHandle<Result<(), FlashblocksError>>,
}

impl FlashblocksProvider {
    pub fn new(builder_client: RpcClient, payload_rx: broadcast::Receiver<Utf8Bytes>) -> Self {
        let payload_id = Arc::new(Mutex::new(PayloadId::default()));
        let payload_builder = Arc::new(Mutex::new(FlashblockBuilder::default()));

        let stream_handle = FlashblocksProvider::handle_flashblock_stream(
            payload_id.clone(),
            payload_builder.clone(),
            payload_rx,
        );

        Self {
            builder_client,
            payload_id,
            payload_builder,
            stream_handle,
        }
    }

    fn handle_flashblock_stream(
        payload_id: Arc<Mutex<PayloadId>>,
        payload_builder: Arc<Mutex<FlashblockBuilder>>,
        mut payload_rx: broadcast::Receiver<Utf8Bytes>,
    ) -> JoinHandle<Result<(), FlashblocksError>> {
        tokio::spawn(async move {
            loop {
                let payload_bytes = payload_rx.recv().await?;
                let flashblock = serde_json::from_str::<FlashblocksPayloadV1>(&payload_bytes)?;
                // self.metrics.messages_processed.increment(1);

                let local_payload_id = payload_id.lock().unwrap();
                if *local_payload_id == flashblock.payload_id {
                    let mut payload_builder = payload_builder.lock().unwrap();
                    payload_builder
                        .extend(flashblock)
                        .expect("TODO: handle error");
                } else {
                    // self.metrics.current_payload_id_mismatch.increment(1);
                    error!(
                        message = "Payload ID mismatch",
                        payload_id = %flashblock.payload_id,
                        %local_payload_id,
                        index = flashblock.index,
                    );
                }
            }
        })
    }
}

#[async_trait]
impl EngineApiExt for FlashblocksProvider {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        // Calculate and set expected payload_id
        if let Some(attr) = &payload_attributes {
            let payload_id = payload_id_optimism(&fork_choice_state.head_block_hash, attr, 3);
            tracing::debug!(message = "Setting current payload ID", payload_id = %payload_id);
            *self.payload_id.lock().expect("TODO: handle error ") = payload_id;
        }

        let resp = self
            .builder_client
            .fork_choice_updated_v3(fork_choice_state, payload_attributes)
            .await?;

        Ok(resp)
    }

    async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus> {
        self.builder_client.new_payload(new_payload).await
    }

    async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope> {
        // Check that we have flashblocks for correct payload
        let payload = if *self.payload_id.lock().expect("TODO: handle error ") == payload_id {
            let mut builder = self.payload_builder.lock().expect("TODO: handle error ");
            // Take payload and place new one in its place in one go to avoid double locking
            std::mem::replace(&mut *builder, FlashblockBuilder::new())
                .into_envelope(version)
                .expect("TODO: handle error")
        } else {
            // We have outdated `current_payload_id` so we should fallback to get_payload
            // Clearing best_payload in here would cause situation when old `get_payload` would clear
            // currently built correct flashblocks.
            // This will self-heal on the next FCU.
            error!(target: "provider::get_payload", message = "Payload id mismatch");
            self.builder_client.get_payload(payload_id, version).await?
        };

        Ok(payload)
    }

    async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Block> {
        self.builder_client.get_block_by_number(number, full).await
    }
}

#[derive(Debug, Default)]
struct FlashblockBuilder {
    base: Option<ExecutionPayloadBaseV1>,
    flashblocks: Vec<ExecutionPayloadFlashblockDeltaV1>,
}

impl FlashblockBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn extend(&mut self, payload: FlashblocksPayloadV1) -> Result<(), FlashblocksError> {
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
        let base = self.base.ok_or(FlashblocksError::MissingPayload)?;

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
            blob_gas_used: 0,
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
                    extra_data: base.extra_data,
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
            PayloadVersion::V4 => Ok(OpExecutionPayloadEnvelope::V4(
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

#[derive(Debug, Error)]
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
    #[error(transparent)]
    RecvError(#[from] RecvError),
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
}
