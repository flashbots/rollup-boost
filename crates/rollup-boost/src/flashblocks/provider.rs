use super::primitives::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashblocksPayloadV1,
};
use crate::RpcClient;
use crate::{ClientResult, EngineApiExt, NewPayload, OpExecutionPayloadEnvelope, PayloadVersion};
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
use parking_lot::Mutex;
use reth_optimism_payload_builder::payload_id_optimism;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::broadcast::error::RecvError;
use tracing::error;

pub struct FlashblocksProvider {
    pub payload_id: Arc<Mutex<PayloadId>>,
    pub payload_builder: Arc<Mutex<FlashblockBuilder>>,
    builder_client: RpcClient,
}

impl FlashblocksProvider {
    pub fn new(builder_client: RpcClient) -> Self {
        let payload_id = Arc::new(Mutex::new(PayloadId::default()));
        let payload_builder = Arc::new(Mutex::new(FlashblockBuilder::default()));

        Self {
            builder_client,
            payload_id,
            payload_builder,
        }
    }

    fn take_payload(
        &self,
        version: PayloadVersion,
        payload_id: PayloadId,
    ) -> Result<OpExecutionPayloadEnvelope, FlashblocksError> {
        // Check that we have flashblocks for correct payload
        if *self.payload_id.lock() != payload_id {
            // We have outdated `current_payload_id` so we should fallback to get_payload
            // Clearing best_payload in here would cause situation when old `get_payload` would clear
            // currently built correct flashblocks.
            // This will self-heal on the next FCU.
            return Err(FlashblocksError::MissingPayload);
        }
        // consume the best payload and reset the builder
        let payload = {
            let mut builder = self.payload_builder.lock();
            // Take payload and place new one in its place in one go to avoid double locking
            std::mem::replace(&mut *builder, FlashblockBuilder::new()).into_envelope(version)?
        };

        Ok(payload)
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
            *self.payload_id.lock() = payload_id;
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
        match self.take_payload(version, payload_id) {
            Ok(payload) => Ok(payload),
            Err(e) => {
                error!("Failed to get flashblocks payload, falling back to builder: {e}");
                self.builder_client.get_payload(payload_id, version).await
            }
        }
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
pub struct FlashblockBuilder {
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

        let transactions = self
            .flashblocks
            .iter()
            .flat_map(|diff| diff.transactions.clone())
            .collect();

        let withdrawals = self
            .flashblocks
            .iter()
            .flat_map(|diff| diff.withdrawals.clone())
            .collect();

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

#[cfg(test)]
mod tests {

    #[test]
    fn test_take_payload() {
        todo!()
    }

    #[test]
    fn test_fork_choice_updated() {
        todo!()
    }

    #[test]
    fn test_get_payload() {
        todo!()
    }

    #[test]
    // TODO: separate into separate tests for failure cases
    fn test_extend() {}

    #[test]
    fn test_extend_missing_base_payload() {}

    #[test]
    fn test_extend_unexpected_base_payload() {}

    #[test]
    fn test_into_envelope() {}
}
