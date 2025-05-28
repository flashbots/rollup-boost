use alloy_primitives::{B256, Bytes};
use alloy_rpc_types_engine::{
    ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use jsonrpsee::core::async_trait;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};

use crate::{ClientResult, NewPayload, OpExecutionPayloadEnvelope, PayloadVersion};

#[async_trait]
pub trait EngineApiExt {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated>;

    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> ClientResult<PayloadStatus>;

    async fn new_payload_v4(
        &self,
        payload: OpExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> ClientResult<PayloadStatus>;

    async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus> {
        match new_payload {
            NewPayload::V3(new_payload) => {
                self.new_payload_v3(
                    new_payload.payload,
                    new_payload.versioned_hashes,
                    new_payload.parent_beacon_block_root,
                )
                .await
            }
            NewPayload::V4(new_payload) => {
                self.new_payload_v4(
                    new_payload.payload,
                    new_payload.versioned_hashes,
                    new_payload.parent_beacon_block_root,
                    new_payload.execution_requests,
                )
                .await
            }
        }
    }

    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> ClientResult<OpExecutionPayloadEnvelopeV3>;

    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> ClientResult<OpExecutionPayloadEnvelopeV4>;

    async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope> {
        match version {
            PayloadVersion::V3 => Ok(OpExecutionPayloadEnvelope::V3(
                self.get_payload_v3(payload_id).await?,
            )),
            PayloadVersion::V4 => Ok(OpExecutionPayloadEnvelope::V4(
                self.get_payload_v4(payload_id).await?,
            )),
        }
    }

    async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Block>;
}
