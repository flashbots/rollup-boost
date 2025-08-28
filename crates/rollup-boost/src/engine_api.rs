use alloy_eips::BlockId;
use alloy_primitives::{Address, B256, BlockHash, Bytes, U64, U128, U256};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionPayloadBodiesV1, ExecutionPayloadEnvelopeV2, ExecutionPayloadInputV2,
    ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
};
use alloy_rpc_types_eth::state::StateOverride;
use alloy_rpc_types_eth::{
    BlockNumberOrTag, BlockOverrides, EIP1186AccountProofResponse, Filter, Log, SyncStatus,
    erc4337::TransactionConditional,
};
use alloy_serde::JsonStorageKey;
use jsonrpsee::core::async_trait;
use op_alloy_network::Optimism;
use op_alloy_rpc_types_engine::{OpPayloadAttributes, ProtocolVersion, SuperchainSignal};
use reth_rpc_eth_api::{RpcBlock, RpcReceipt, RpcTxReq};

use crate::{ClientResult, NewPayload, OpExecutionPayloadEnvelope, PayloadVersion};

#[async_trait]
pub trait AuthApiExt: Send + Sync + 'static {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated>;

    async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus>;

    async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope>;

    /// Proxy to eth_sendRawTransaction, sent to builder and l2
    async fn send_raw_transaction(&self, bytes: Bytes) -> ClientResult<B256>;

    /// Proxy to eth_sendRawTransactionConditional, sent to builder and l2
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> ClientResult<B256>;

    /// Proxy to eth_syncing, sent to l2
    async fn syncing(&self) -> ClientResult<SyncStatus>;

    /// Proxy to eth_chainId, sent to l2
    async fn chain_id(&self) -> ClientResult<Option<U64>>;

    /// Proxy to eth_blockNumber, sent to l2
    async fn block_number(&self) -> ClientResult<U256>;

    /// Proxy to eth_getBlockByNumber, sent to l2
    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Option<RpcBlock<Optimism>>>;

    /// Proxy to miner_setExtra, sent to builder and l2
    async fn set_extra(&self, record: Bytes) -> ClientResult<bool>;

    /// Proxy to miner_setGasPrice, sent to builder and l2
    async fn set_gas_price(&self, gas_price: U128) -> ClientResult<bool>;

    /// Proxy to miner_setGasLimit, sent to builder and l2
    async fn set_gas_limit(&self, gas_limit: U128) -> ClientResult<bool>;

    /// Proxy to miner_setMaxDASize, sent to builder and l2
    async fn set_max_da_size(&self, max_tx_size: U64, max_block_size: U64) -> ClientResult<bool>;

    /// Proxy to engine_getPayloadV2, sent to l2
    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> ClientResult<ExecutionPayloadEnvelopeV2>;

    /// Proxy to engine_forkChoiceUpdatedV1, sent to l2
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated>;

    /// Proxy to engine_forkChoiceUpdatedV2, sent to l2
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated>;

    /// Proxy to engine_newPayloadV2, sent to l2
    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2)
    -> ClientResult<PayloadStatus>;

    /// Proxy to engine_getPayloadBodiesByHashV1, sent to l2
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> ClientResult<ExecutionPayloadBodiesV1>;

    /// Proxy to engine_getPayloadBodiesByRangeV1, sent to l2
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> ClientResult<ExecutionPayloadBodiesV1>;

    /// Proxy to engine_signalSuperchainV1, sent to l2
    async fn signal_superchain_v1(&self, signal: SuperchainSignal)
    -> ClientResult<ProtocolVersion>;

    /// Proxy to engine_getClientVersionV1, sent to l2
    async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> ClientResult<Vec<ClientVersionV1>>;

    /// Proxy to engine_exchangeCapabilities, sent to l2
    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> ClientResult<Vec<String>>;

    async fn call(
        &self,
        request: RpcTxReq<Optimism>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> ClientResult<Bytes>;

    async fn get_code(&self, address: Address, block_id: Option<BlockId>) -> ClientResult<Bytes>;

    async fn block_by_hash(
        &self,
        hash: B256,
        full: bool,
    ) -> ClientResult<Option<RpcBlock<Optimism>>>;

    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> ClientResult<Option<Vec<RpcReceipt<Optimism>>>>;

    async fn transaction_receipt(&self, hash: B256) -> ClientResult<Option<RpcReceipt<Optimism>>>;

    async fn logs(&self, filter: Filter) -> ClientResult<Vec<Log>>;

    async fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_number: Option<BlockId>,
    ) -> ClientResult<EIP1186AccountProofResponse>;
}
