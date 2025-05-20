use alloy_primitives::{B256, Bytes};
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, PayloadId,
    PayloadStatus,
};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use jsonrpsee::{
    core::{RpcResult, async_trait},
    proc_macros::rpc,
};
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};

use crate::ClientResult;

#[rpc(server, client)]
pub trait EngineApi {
    #[method(name = "engine_forkchoiceUpdatedV3")]
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "engine_getPayloadV3")]
    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelopeV3>;

    #[method(name = "engine_newPayloadV3")]
    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus>;

    #[method(name = "engine_getPayloadV4")]
    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelopeV4>;

    #[method(name = "engine_newPayloadV4")]
    async fn new_payload_v4(
        &self,
        payload: OpExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> RpcResult<PayloadStatus>;

    #[method(name = "eth_getBlockByNumber")]
    async fn get_block_by_number(&self, number: BlockNumberOrTag, full: bool) -> RpcResult<Block>;
}

#[derive(Debug, Clone)]
pub struct NewPayloadV3 {
    pub payload: ExecutionPayloadV3,
    pub versioned_hashes: Vec<B256>,
    pub parent_beacon_block_root: B256,
}

#[derive(Debug, Clone)]
pub struct NewPayloadV4 {
    pub payload: OpExecutionPayloadV4,
    pub versioned_hashes: Vec<B256>,
    pub parent_beacon_block_root: B256,
    pub execution_requests: Vec<Bytes>,
}

#[derive(Debug, Clone)]
pub enum NewPayload {
    V3(NewPayloadV3),
    V4(NewPayloadV4),
}

impl NewPayload {
    pub fn version(&self) -> Version {
        match self {
            NewPayload::V3(_) => Version::V3,
            NewPayload::V4(_) => Version::V4,
        }
    }
}

impl From<OpExecutionPayloadEnvelope> for NewPayload {
    fn from(envelope: OpExecutionPayloadEnvelope) -> Self {
        match envelope {
            OpExecutionPayloadEnvelope::V3(v3) => NewPayload::V3(NewPayloadV3 {
                payload: v3.execution_payload,
                versioned_hashes: vec![],
                parent_beacon_block_root: v3.parent_beacon_block_root,
            }),
            OpExecutionPayloadEnvelope::V4(v4) => NewPayload::V4(NewPayloadV4 {
                payload: v4.execution_payload,
                versioned_hashes: vec![],
                parent_beacon_block_root: v4.parent_beacon_block_root,
                execution_requests: v4.execution_requests,
            }),
        }
    }
}

impl From<NewPayload> for ExecutionPayload {
    fn from(new_payload: NewPayload) -> Self {
        match new_payload {
            NewPayload::V3(v3) => ExecutionPayload::from(v3.payload),
            NewPayload::V4(v4) => ExecutionPayload::from(v4.payload.payload_inner),
        }
    }
}

#[derive(Debug, Clone)]
pub enum OpExecutionPayloadEnvelope {
    V3(OpExecutionPayloadEnvelopeV3),
    V4(OpExecutionPayloadEnvelopeV4),
}

impl OpExecutionPayloadEnvelope {
    pub fn version(&self) -> Version {
        match self {
            OpExecutionPayloadEnvelope::V3(_) => Version::V3,
            OpExecutionPayloadEnvelope::V4(_) => Version::V4,
        }
    }
}

impl From<OpExecutionPayloadEnvelope> for ExecutionPayload {
    fn from(envelope: OpExecutionPayloadEnvelope) -> Self {
        match envelope {
            OpExecutionPayloadEnvelope::V3(v3) => ExecutionPayload::from(v3.execution_payload),
            OpExecutionPayloadEnvelope::V4(v4) => {
                ExecutionPayload::from(v4.execution_payload.payload_inner)
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Version {
    V3,
    V4,
}

impl Version {
    pub fn as_str(&self) -> &'static str {
        match self {
            Version::V3 => "v3",
            Version::V4 => "v4",
        }
    }
}

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
        version: Version,
    ) -> ClientResult<OpExecutionPayloadEnvelope> {
        match version {
            Version::V3 => Ok(OpExecutionPayloadEnvelope::V3(
                self.get_payload_v3(payload_id).await?,
            )),
            Version::V4 => Ok(OpExecutionPayloadEnvelope::V4(
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
