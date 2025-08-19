use super::primitives::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashblocksPayloadV1,
};
use crate::{FlashblocksError, OpExecutionPayloadEnvelope, PayloadVersion};
use alloy_primitives::U256;
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
};
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
};

#[derive(Clone, Debug, Default)]
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
