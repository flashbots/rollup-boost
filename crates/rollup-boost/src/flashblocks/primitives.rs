use alloy_consensus::Block;
use alloy_eips::Decodable2718;
use alloy_primitives::{Address, B256, Bloom, Bytes, U256};
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
    PayloadId,
};
use alloy_rpc_types_eth::Withdrawal;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{OpExecutionPayloadEnvelope, PayloadVersion};

/// Represents the modified portions of an execution payload within a flashblock.
/// This structure contains only the fields that can be updated during block construction,
/// such as state root, receipts, logs, and new transactions. Other immutable block fields
/// like parent hash and block number are excluded since they remain constant throughout
/// the block's construction.
#[derive(Clone, Debug, PartialEq, Default, Deserialize, Serialize)]
pub struct ExecutionPayloadFlashblockDeltaV1 {
    /// The state root of the block.
    pub state_root: B256,
    /// The receipts root of the block.
    pub receipts_root: B256,
    /// The logs bloom of the block.
    pub logs_bloom: Bloom,
    /// The gas used of the block.
    #[serde(with = "alloy_serde::quantity")]
    pub gas_used: u64,
    /// The block hash of the block.
    pub block_hash: B256,
    /// The transactions of the block.
    pub transactions: Vec<Bytes>,
    /// Array of [`Withdrawal`] enabled with V2
    pub withdrawals: Vec<Withdrawal>,
    /// The withdrawals root of the block.
    pub withdrawals_root: B256,
}

/// Represents the base configuration of an execution payload that remains constant
/// throughout block construction. This includes fundamental block properties like
/// parent hash, block number, and other header fields that are determined at
/// block creation and cannot be modified.
#[derive(Clone, Debug, PartialEq, Default, Deserialize, Serialize)]
pub struct ExecutionPayloadBaseV1 {
    /// Ecotone parent beacon block root
    pub parent_beacon_block_root: B256,
    /// The parent hash of the block.
    pub parent_hash: B256,
    /// The fee recipient of the block.
    pub fee_recipient: Address,
    /// The previous randao of the block.
    pub prev_randao: B256,
    /// The block number.
    #[serde(with = "alloy_serde::quantity")]
    pub block_number: u64,
    /// The gas limit of the block.
    #[serde(with = "alloy_serde::quantity")]
    pub gas_limit: u64,
    /// The timestamp of the block.
    #[serde(with = "alloy_serde::quantity")]
    pub timestamp: u64,
    /// The extra data of the block.
    pub extra_data: Bytes,
    /// The base fee per gas of the block.
    pub base_fee_per_gas: U256,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize, Serialize)]
pub struct FlashblocksPayloadV1 {
    /// The payload id of the flashblock
    pub payload_id: PayloadId,
    /// The index of the flashblock in the block
    pub index: u64,
    /// The base execution payload configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<ExecutionPayloadBaseV1>,
    /// The delta/diff containing modified portions of the execution payload
    pub diff: ExecutionPayloadFlashblockDeltaV1,
    /// Additional metadata associated with the flashblock
    pub metadata: Value,
}

impl FlashblocksPayloadV1 {
    /// Reduces the [`FlashblocksPayloadV1`] into an aggregated payload where the delta
    /// represents the final execution payload.
    ///
    /// This is useful for sealing a built block from an aggregated payload.
    /// See: [`FlashblocksPayloadV1::into_envelope`]
    pub fn reduce_all(payloads: Vec<Self>) -> Option<Self> {
        let mut iter = payloads.into_iter();
        let mut acc = iter.next()?;

        for next in iter {
            debug_assert_eq!(
                acc.payload_id, next.payload_id,
                "all flashblocks should have the same payload_id"
            );

            if acc.base.is_none() && next.base.is_some() {
                acc.base = next.base;
            }

            acc.diff.gas_used += next.diff.gas_used;

            acc.diff.transactions.extend(next.diff.transactions);
            acc.diff.withdrawals.extend(next.diff.withdrawals);

            acc.diff.state_root = next.diff.state_root;
            acc.diff.receipts_root = next.diff.receipts_root;
            acc.diff.logs_bloom = next.diff.logs_bloom;
            acc.diff.block_hash = next.diff.block_hash;
            acc.diff.withdrawals_root = next.diff.withdrawals_root;
        }

        Some(acc)
    }

    /// Converts an aggregated [`FlashblocksPayloadV1`] into an [`OpExecutionPayloadEnvelope`].
    pub fn into_envelope(self, version: PayloadVersion) -> Option<OpExecutionPayloadEnvelope> {
        let base = self.base?;
        let execution_payload = ExecutionPayloadV3 {
            blob_gas_used: 0,
            excess_blob_gas: 0,
            payload_inner: ExecutionPayloadV2 {
                withdrawals: self.diff.withdrawals,
                payload_inner: ExecutionPayloadV1 {
                    parent_hash: base.parent_hash,
                    fee_recipient: base.fee_recipient,
                    state_root: self.diff.state_root,
                    receipts_root: self.diff.receipts_root,
                    logs_bloom: self.diff.logs_bloom,
                    prev_randao: base.prev_randao,
                    block_number: base.block_number,
                    gas_limit: base.gas_limit,
                    gas_used: self.diff.gas_used,
                    timestamp: base.timestamp,
                    extra_data: base.extra_data.clone(),
                    base_fee_per_gas: base.base_fee_per_gas,
                    block_hash: self.diff.block_hash,
                    transactions: self.diff.transactions,
                },
            },
        };

        match version {
            PayloadVersion::V3 => Some(OpExecutionPayloadEnvelope::V3(
                OpExecutionPayloadEnvelopeV3 {
                    parent_beacon_block_root: base.parent_beacon_block_root,
                    block_value: U256::ZERO,
                    blobs_bundle: BlobsBundleV1::default(),
                    should_override_builder: false,
                    execution_payload,
                },
            )),
            PayloadVersion::V4 => Some(OpExecutionPayloadEnvelope::V4(
                OpExecutionPayloadEnvelopeV4 {
                    parent_beacon_block_root: base.parent_beacon_block_root,
                    block_value: U256::ZERO,
                    blobs_bundle: BlobsBundleV1::default(),
                    should_override_builder: false,
                    execution_payload: OpExecutionPayloadV4 {
                        withdrawals_root: self.diff.withdrawals_root,
                        payload_inner: execution_payload,
                    },
                    execution_requests: vec![],
                },
            )),
        }
    }

    /// Converts an aggregated [`FlashblocksPayloadV1`] into a [`Block<T>`]
    pub fn try_into_block<T: Decodable2718>(self, version: PayloadVersion) -> Option<Block<T>> {
        let envelope = ExecutionPayload::from(self.into_envelope(version)?);
        envelope.try_into_block().map(|block| block).ok()
    }
}
