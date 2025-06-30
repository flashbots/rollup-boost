use super::outbound::WebSocketPublisher;
use super::primitives::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashblocksPayloadV1,
};
use crate::RpcClient;
use crate::flashblocks::metrics::FlashblocksServiceMetrics;
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
use core::net::SocketAddr;
use jsonrpsee::core::async_trait;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::{error, info};

pub struct FlashblocksProvider {
    pub builder_client: Arc<RpcClient>,
}

impl FlashblocksProvider {
    pub fn new(builder_client: Arc<RpcClient>) -> Self {
        Self { builder_client }
    }
}

#[async_trait]
impl EngineApiExt for FlashblocksProvider {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        todo!()
    }

    async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus> {
        self.builder_client.new_payload(new_payload).await
    }

    async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope> {
        todo!()
    }

    async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Block> {
        self.builder_client.get_block_by_number(number, full).await
    }
}
