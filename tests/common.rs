use alloy_primitives::{Bytes, B256, U128, U64};
use alloy_rpc_types_engine::{
    ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
};
use jsonrpsee::core::{async_trait, RpcResult};
use op_alloy_rpc_jsonrpsee::traits::MinerApiExtServer;
use op_alloy_rpc_types_engine::{OpExecutionPayloadEnvelopeV3, OpPayloadAttributes};
use rollup_boost::server::{EngineApiServer, EthApiServer, MinerApiServer};

// TODO: think through best place for these tests

pub struct MockExecutionClient {}

#[async_trait]
impl EngineApiServer for MockExecutionClient {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        todo!()
    }

    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<OpExecutionPayloadEnvelopeV3> {
        todo!()
    }

    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        todo!()
    }
}

#[async_trait]
impl EthApiServer for MockExecutionClient {
    async fn send_raw_transaction(&self, bytes: Bytes) -> RpcResult<B256> {
        todo!()
    }
}

#[async_trait]
impl MinerApiServer for MockExecutionClient {
    async fn set_extra(&self, record: Bytes) -> RpcResult<bool> {
        todo!()
    }

    async fn set_gas_price(&self, gas_price: U128) -> RpcResult<bool> {
        todo!()
    }

    async fn set_gas_limit(&self, gas_price: U128) -> RpcResult<bool> {
        todo!()
    }
}

#[async_trait]
impl MinerApiExtServer for MockExecutionClient {
    async fn set_max_da_size(&self, max_tx_size: U64, max_block_size: U64) -> RpcResult<bool> {
        todo!()
    }
}
