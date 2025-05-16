use crate::{
    HealthHandle,
    client::rpc::RpcClient,
    debug_api::DebugServer,
    probe::{Health, Probes},
};
use alloy_primitives::{B256, Bytes};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use futures::{StreamExt as _, stream};
use metrics::counter;
use moka::future::Cache;
use opentelemetry::trace::SpanKind;
use parking_lot::Mutex;
use std::sync::Arc;

use crate::debug_api::ExecutionMode;
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, PayloadId,
    PayloadStatus,
};
use jsonrpsee::RpcModule;
use jsonrpsee::core::{RegisterMethodError, RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObject;
use jsonrpsee::types::error::INVALID_REQUEST_CODE;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};
use tokio::task::JoinHandle;
use tracing::{info, instrument};

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
