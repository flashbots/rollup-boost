use crate::AuthApiExt;
use crate::client::auth::AuthLayer;
use crate::payload::{NewPayload, OpExecutionPayloadEnvelope, PayloadSource, PayloadVersion};
use crate::version::{CARGO_PKG_VERSION, VERGEN_GIT_SHA};
use alloy_eips::BlockId;
use alloy_primitives::{Address, B256, BlockHash, Bytes, U64, U128, U256};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionPayloadBodiesV1, ExecutionPayloadEnvelopeV2, ExecutionPayloadInputV2,
    ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, JwtError, JwtSecret, PayloadId,
    PayloadStatus,
};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use alloy_rpc_types_eth::state::StateOverride;
use alloy_rpc_types_eth::{
    BlockNumberOrTag, BlockOverrides, EIP1186AccountProofResponse, Filter, Log, SyncStatus,
};
use alloy_serde::JsonStorageKey;
use clap::{Parser, arg};
use http::{HeaderMap, Uri};
use jsonrpsee::core::async_trait;
use jsonrpsee::core::middleware::layer::RpcLogger;
use jsonrpsee::http_client::transport::HttpBackend;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder, RpcService};
use jsonrpsee::types::ErrorObjectOwned;
use op_alloy_network::Optimism;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes, ProtocolVersion, SuperchainSignal,
};
use opentelemetry::trace::SpanKind;
use paste::paste;
use reth_optimism_node::OpEngineTypes;
use reth_optimism_rpc::OpEngineApiClient;
use reth_rpc_api::eth::L2EthApiExtClient;
use reth_rpc_api::{EngineEthApiClient, MinerApiClient};
use reth_rpc_eth_api::{RpcBlock, RpcReceipt, RpcTxReq};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tracing::{error, info, instrument};

use super::auth::Auth;

pub type RpcClientService = HttpClient<RpcLogger<RpcService<Auth<HttpBackend>>>>;

const INTERNAL_ERROR: i32 = 13;

pub(crate) type ClientResult<T> = Result<T, RpcClientError>;

#[derive(Error, Debug)]
pub enum RpcClientError {
    #[error(transparent)]
    Jsonrpsee(#[from] jsonrpsee::core::client::Error),
    #[error("Invalid payload: {0}")]
    InvalidPayload(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Jwt(#[from] JwtError),
}

trait Code: Sized {
    fn code(&self) -> i32;

    fn set_code(self) -> Self {
        tracing::Span::current().record("code", self.code());
        self
    }
}

impl<T, E: Code> Code for Result<T, E> {
    fn code(&self) -> i32 {
        match self {
            Ok(_) => 0,
            Err(e) => e.code(),
        }
    }
}

/// TODO: Add more robust error code system
impl Code for RpcClientError {
    fn code(&self) -> i32 {
        match self {
            RpcClientError::Jsonrpsee(e) => e.code(),
            // Status code 13 == internal error
            _ => INTERNAL_ERROR,
        }
    }
}

impl Code for jsonrpsee::core::client::Error {
    fn code(&self) -> i32 {
        match self {
            jsonrpsee::core::client::Error::Call(call) => call.code(),
            _ => INTERNAL_ERROR,
        }
    }
}

impl From<RpcClientError> for ErrorObjectOwned {
    fn from(err: RpcClientError) -> Self {
        match err {
            RpcClientError::Jsonrpsee(jsonrpsee::core::ClientError::Call(error_object)) => {
                error_object
            }
            // Status code 13 == internal error
            e => ErrorObjectOwned::owned(INTERNAL_ERROR, e.to_string(), Option::<()>::None),
        }
    }
}

/// Client interface for interacting with execution layer node's Engine API.
///
/// - **Engine API** calls are faciliated via the `auth_client` (requires JWT authentication).
///
#[derive(Clone)]
pub struct RpcClient {
    /// Handles requests to the authenticated Engine API (requires JWT authentication)
    pub auth_client: RpcClientService,
    /// Uri of the RPC server for authenticated Engine API calls
    pub auth_rpc: Uri,
    /// The source of the payload
    pub payload_source: PayloadSource,
}

impl RpcClient {
    /// Initializes a new [ExecutionClient] with JWT auth for the Engine API and without auth for general execution layer APIs.
    pub fn new(
        auth_rpc: Uri,
        auth_rpc_jwt_secret: JwtSecret,
        timeout: u64,
        payload_source: PayloadSource,
    ) -> Result<Self, RpcClientError> {
        let version = format!("{CARGO_PKG_VERSION}-{VERGEN_GIT_SHA}");
        let mut headers = HeaderMap::new();
        headers.insert("User-Agent", version.parse().unwrap());
        let auth_layer = AuthLayer::new(auth_rpc_jwt_secret);
        let auth_client = HttpClientBuilder::new()
            .set_http_middleware(tower::ServiceBuilder::new().layer(auth_layer))
            .set_headers(headers)
            .request_timeout(Duration::from_millis(timeout))
            .build(auth_rpc.to_string())?;

        Ok(Self {
            auth_client,
            auth_rpc,
            payload_source,
        })
    }

    #[instrument(
        skip_all,
        err,
        fields(
            otel.kind = ?SpanKind::Client,
            target = self.payload_source.to_string(),
            head_block_hash = %fork_choice_state.head_block_hash,
            url = %self.auth_rpc,
            code,
            payload_id
        )
    )]
    pub async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        info!("Sending fork_choice_updated_v3 to {}", self.payload_source);
        let res = <RpcClientService as OpEngineApiClient<OpEngineTypes>>::fork_choice_updated_v3(
            &self.auth_client,
            fork_choice_state,
            payload_attributes,
        )
        .await
        .set_code()?;

        if let Some(payload_id) = res.payload_id {
            tracing::Span::current().record("payload_id", payload_id.to_string());
        }

        if res.is_invalid() {
            return Err(RpcClientError::InvalidPayload(
                res.payload_status.status.to_string(),
            ))
            .set_code();
        }
        info!(
            "Successfully sent fork_choice_updated_v3 to {}",
            self.payload_source
        );

        Ok(res)
    }

    #[instrument(
        skip(self),
        err,
        fields(
            otel.kind = ?SpanKind::Client,
            target = self.payload_source.to_string(),
            url = %self.auth_rpc,
            %payload_id,
        )
    )]
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> ClientResult<OpExecutionPayloadEnvelopeV3> {
        tracing::Span::current().record("payload_id", payload_id.to_string());
        info!("Sending get_payload_v3 to {}", self.payload_source);
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::get_payload_v3(
                &self.auth_client,
                payload_id,
            )
            .await
            .set_code()?,
        )
    }

    #[instrument(
        skip_all,
        err,
        fields(
            otel.kind = ?SpanKind::Client,
            target = self.payload_source.to_string(),
            url = %self.auth_rpc,
            block_hash = %payload.payload_inner.payload_inner.block_hash,
            code,
        )
    )]
    pub async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> ClientResult<PayloadStatus> {
        info!("Sending new_payload_v3 to {}", self.payload_source);

        let res = <RpcClientService as OpEngineApiClient<OpEngineTypes>>::new_payload_v3(
            &self.auth_client,
            payload,
            versioned_hashes,
            parent_beacon_block_root,
        )
        .await
        .set_code()?;

        if res.is_invalid() {
            return Err(RpcClientError::InvalidPayload(res.status.to_string()).set_code());
        }

        Ok(res)
    }

    #[instrument(
        skip(self),
        err,
        fields(
            otel.kind = ?SpanKind::Client,
            target = self.payload_source.to_string(),
            url = %self.auth_rpc,
            %payload_id,
        )
    )]
    pub async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> ClientResult<OpExecutionPayloadEnvelopeV4> {
        info!("Sending get_payload_v4 to {}", self.payload_source);
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::get_payload_v4(
                &self.auth_client,
                payload_id,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope> {
        match version {
            PayloadVersion::V3 => Ok(OpExecutionPayloadEnvelope::V3(
                self.get_payload_v3(payload_id).await.set_code()?,
            )),
            PayloadVersion::V4 => Ok(OpExecutionPayloadEnvelope::V4(
                self.get_payload_v4(payload_id).await.set_code()?,
            )),
        }
    }

    #[instrument(
        skip_all,
        err,
        fields(
            otel.kind = ?SpanKind::Client,
            target = self.payload_source.to_string(),
            url = %self.auth_rpc,
            block_hash = %payload.payload_inner.payload_inner.payload_inner.block_hash,
            code,
        )
    )]
    async fn new_payload_v4(
        &self,
        payload: OpExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> ClientResult<PayloadStatus> {
        info!("Sending new_payload_v4 to {}", self.payload_source);

        let res = <RpcClientService as OpEngineApiClient<OpEngineTypes>>::new_payload_v4(
            &self.auth_client,
            payload,
            versioned_hashes,
            parent_beacon_block_root,
            execution_requests.into(),
        )
        .await
        .set_code()?;

        if res.is_invalid() {
            return Err(RpcClientError::InvalidPayload(res.status.to_string()).set_code());
        }

        Ok(res)
    }

    pub async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus> {
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

    pub async fn block_number(&self) -> ClientResult<U256> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::block_number(&self.auth_client)
        .await
        .set_code()?)
    }

    pub async fn chain_id(&self) -> ClientResult<Option<U64>> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::chain_id(&self.auth_client)
        .await
        .set_code()?)
    }

    pub async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Option<RpcBlock<Optimism>>> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::block_by_number(&self.auth_client, number, full)
        .await
        .set_code()?)
    }

    pub async fn send_raw_transaction(&self, bytes: Bytes) -> ClientResult<B256> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::send_raw_transaction(&self.auth_client, bytes)
        .await
        .set_code()?)
    }

    pub async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> ClientResult<B256> {
        Ok(self
            .auth_client
            .send_raw_transaction_conditional(bytes, condition)
            .await
            .set_code()?)
    }

    pub async fn syncing(&self) -> ClientResult<SyncStatus> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::syncing(&self.auth_client)
        .await
        .set_code()?)
    }

    pub async fn set_extra(&self, record: Bytes) -> ClientResult<bool> {
        Ok(
            <RpcClientService as MinerApiClient>::set_extra(&self.auth_client, record)
                .await
                .set_code()?,
        )
    }

    pub async fn set_gas_price(&self, gas_price: U128) -> ClientResult<bool> {
        Ok(
            <RpcClientService as MinerApiClient>::set_gas_price(&self.auth_client, gas_price)
                .await
                .set_code()?,
        )
    }

    pub async fn set_gas_limit(&self, gas_limit: U128) -> ClientResult<bool> {
        Ok(
            <RpcClientService as MinerApiClient>::set_gas_limit(&self.auth_client, gas_limit)
                .await
                .set_code()?,
        )
    }

    pub async fn set_max_da_size(&self, max_da_size: U64, max_da_gas: U64) -> ClientResult<bool> {
        Ok(
            <RpcClientService as op_alloy_rpc_jsonrpsee::traits::MinerApiExtClient>::set_max_da_size(
                &self.auth_client,
                max_da_size,
                max_da_gas,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> ClientResult<ExecutionPayloadEnvelopeV2> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::get_payload_v2(
                &self.auth_client,
                payload_id,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::fork_choice_updated_v1(
                &self.auth_client,
                fork_choice_state,
                payload_attributes,
            )
            .await
            .set_code()?,
        )
    }
    pub async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::fork_choice_updated_v2(
                &self.auth_client,
                fork_choice_state,
                payload_attributes,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn new_payload_v2(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> ClientResult<PayloadStatus> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::new_payload_v2(
                &self.auth_client,
                payload,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> ClientResult<ExecutionPayloadBodiesV1> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::get_payload_bodies_by_hash_v1(
                &self.auth_client,
                block_hashes,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> ClientResult<ExecutionPayloadBodiesV1> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::get_payload_bodies_by_range_v1(
                &self.auth_client,
                start,
                count,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn signal_superchain_v1(
        &self,
        signal: SuperchainSignal,
    ) -> ClientResult<ProtocolVersion> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::signal_superchain_v1(
                &self.auth_client,
                signal,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> ClientResult<Vec<ClientVersionV1>> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::get_client_version_v1(
                &self.auth_client,
                client_version,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn exchange_capabilities(
        &self,
        capabilities: Vec<String>,
    ) -> ClientResult<Vec<String>> {
        Ok(
            <RpcClientService as OpEngineApiClient<OpEngineTypes>>::exchange_capabilities(
                &self.auth_client,
                capabilities,
            )
            .await
            .set_code()?,
        )
    }

    pub async fn call(
        &self,
        request: RpcTxReq<Optimism>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> ClientResult<Bytes> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::call(
            &self.auth_client,
            request,
            block_id,
            state_overrides,
            block_overrides,
        )
        .await
        .set_code()?)
    }

    pub async fn get_code(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> ClientResult<Bytes> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::get_code(&self.auth_client, address, block_id)
        .await
        .set_code()?)
    }

    pub async fn block_by_hash(
        &self,
        hash: B256,
        full: bool,
    ) -> ClientResult<Option<RpcBlock<Optimism>>> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::block_by_hash(&self.auth_client, hash, full)
        .await
        .set_code()?)
    }

    pub async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> ClientResult<Option<Vec<RpcReceipt<Optimism>>>> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::block_receipts(&self.auth_client, block_id)
        .await
        .set_code()?)
    }

    pub async fn transaction_receipt(
        &self,
        hash: B256,
    ) -> ClientResult<Option<RpcReceipt<Optimism>>> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::transaction_receipt(&self.auth_client, hash)
        .await
        .set_code()?)
    }

    pub async fn logs(&self, filter: Filter) -> ClientResult<Vec<Log>> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::logs(&self.auth_client, filter)
        .await
        .set_code()?)
    }

    pub async fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_number: Option<BlockId>,
    ) -> ClientResult<EIP1186AccountProofResponse> {
        Ok(<RpcClientService as EngineEthApiClient<
            RpcTxReq<Optimism>,
            RpcBlock<Optimism>,
            RpcReceipt<Optimism>,
        >>::get_proof(&self.auth_client, address, keys, block_number)
        .await
        .set_code()?)
    }
}

#[async_trait]
impl AuthApiExt for RpcClient {
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        self.fork_choice_updated_v3(fork_choice_state, payload_attributes)
            .await
    }

    async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus> {
        self.new_payload(new_payload).await
    }

    async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope> {
        self.get_payload(payload_id, version).await
    }

    async fn send_raw_transaction(&self, bytes: Bytes) -> ClientResult<B256> {
        self.send_raw_transaction(bytes).await
    }

    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> ClientResult<B256> {
        self.send_raw_transaction_conditional(bytes, condition)
            .await
    }

    async fn syncing(&self) -> ClientResult<SyncStatus> {
        self.syncing().await
    }

    async fn chain_id(&self) -> ClientResult<Option<U64>> {
        self.chain_id().await
    }

    async fn block_number(&self) -> ClientResult<U256> {
        self.block_number().await
    }

    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Option<RpcBlock<Optimism>>> {
        self.block_by_number(number, full).await
    }

    async fn set_extra(&self, record: Bytes) -> ClientResult<bool> {
        self.set_extra(record).await
    }

    async fn set_gas_price(&self, gas_price: U128) -> ClientResult<bool> {
        self.set_gas_price(gas_price).await
    }

    async fn set_gas_limit(&self, gas_limit: U128) -> ClientResult<bool> {
        self.set_gas_limit(gas_limit).await
    }

    async fn set_max_da_size(&self, max_da_size: U64, max_da_gas: U64) -> ClientResult<bool> {
        self.set_max_da_size(max_da_size, max_da_gas).await
    }

    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> ClientResult<ExecutionPayloadEnvelopeV2> {
        self.get_payload_v2(payload_id).await
    }

    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        self.fork_choice_updated_v1(fork_choice_state, payload_attributes)
            .await
    }

    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated> {
        self.fork_choice_updated_v2(fork_choice_state, payload_attributes)
            .await
    }

    async fn new_payload_v2(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> ClientResult<PayloadStatus> {
        self.new_payload_v2(payload).await
    }

    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> ClientResult<ExecutionPayloadBodiesV1> {
        self.get_payload_bodies_by_hash_v1(block_hashes).await
    }

    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> ClientResult<ExecutionPayloadBodiesV1> {
        self.get_payload_bodies_by_range_v1(start, count).await
    }

    async fn signal_superchain_v1(
        &self,
        signal: SuperchainSignal,
    ) -> ClientResult<ProtocolVersion> {
        self.signal_superchain_v1(signal).await
    }

    async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> ClientResult<Vec<ClientVersionV1>> {
        self.get_client_version_v1(client_version).await
    }

    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> ClientResult<Vec<String>> {
        self.exchange_capabilities(capabilities).await
    }

    async fn call(
        &self,
        request: RpcTxReq<Optimism>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> ClientResult<Bytes> {
        self.call(request, block_id, state_overrides, block_overrides)
            .await
    }

    async fn get_code(&self, address: Address, block_id: Option<BlockId>) -> ClientResult<Bytes> {
        self.get_code(address, block_id).await
    }

    async fn block_by_hash(
        &self,
        hash: B256,
        full: bool,
    ) -> ClientResult<Option<RpcBlock<Optimism>>> {
        self.block_by_hash(hash, full).await
    }

    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> ClientResult<Option<Vec<RpcReceipt<Optimism>>>> {
        self.block_receipts(block_id).await
    }

    async fn transaction_receipt(&self, hash: B256) -> ClientResult<Option<RpcReceipt<Optimism>>> {
        self.transaction_receipt(hash).await
    }

    async fn logs(&self, filter: Filter) -> ClientResult<Vec<Log>> {
        self.logs(filter).await
    }

    async fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_number: Option<BlockId>,
    ) -> ClientResult<EIP1186AccountProofResponse> {
        self.get_proof(address, keys, block_number).await
    }
}

/// Generates Clap argument structs with a prefix to create a unique namespace when specifying RPC client config via the CLI.
macro_rules! define_rpc_args {
    ($(($name:ident, $prefix:ident)),*) => {
        $(
            paste! {
                #[derive(Parser, Debug, Clone, PartialEq, Eq)]
                pub struct $name {
                    /// Auth server address
                    #[arg(long, env, default_value = "127.0.0.1:8551")]
                    pub [<$prefix _url>]: Uri,

                    /// Hex encoded JWT secret to use for the authenticated engine-API RPC server.
                    #[arg(long, env, value_name = "HEX")]
                    pub [<$prefix _jwt_token>]: Option<JwtSecret>,

                    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
                    #[arg(long, env, value_name = "PATH")]
                    pub [<$prefix _jwt_path>]: Option<PathBuf>,

                    /// Timeout for http calls in milliseconds
                    #[arg(long, env, default_value_t = 1000)]
                    pub [<$prefix _timeout>]: u64,
                }
            }
        )*
    };
}

define_rpc_args!((BuilderArgs, builder), (L2ClientArgs, l2));

#[cfg(test)]
pub mod tests {
    use assert_cmd::Command;
    use http::Uri;
    use jsonrpsee::core::client::ClientT;
    use parking_lot::Mutex;

    use crate::payload::PayloadSource;
    use alloy_rpc_types_engine::JwtSecret;
    use jsonrpsee::core::client::Error as ClientError;
    use jsonrpsee::server::{ServerBuilder, ServerHandle};
    use jsonrpsee::{RpcModule, rpc_params};
    use predicates::prelude::*;
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::net::TcpListener;
    use std::result::Result;
    use std::str::FromStr;
    use std::sync::LazyLock;

    use super::*;

    const AUTH_ADDR: &str = "127.0.0.1";
    const SECRET: &str = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";

    pub fn get_available_port() -> u16 {
        static CLAIMED_PORTS: LazyLock<Mutex<HashSet<u16>>> =
            LazyLock::new(|| Mutex::new(HashSet::new()));
        loop {
            let port: u16 = rand::random_range(1000..20000);
            if TcpListener::bind(("127.0.0.1", port)).is_ok() && CLAIMED_PORTS.lock().insert(port) {
                return port;
            }
        }
    }

    #[test]
    fn test_invalid_args() {
        let mut cmd = Command::cargo_bin("rollup-boost").unwrap();
        cmd.arg("--invalid-arg");

        cmd.assert().failure().stderr(predicate::str::contains(
            "error: unexpected argument '--invalid-arg' found",
        ));
    }

    #[tokio::test]
    async fn valid_jwt() {
        let port = get_available_port();
        let secret = JwtSecret::from_hex(SECRET).unwrap();
        let auth_rpc = Uri::from_str(&format!("http://{}:{}", AUTH_ADDR, port)).unwrap();
        let client = RpcClient::new(auth_rpc, secret, 1000, PayloadSource::L2).unwrap();
        let response = send_request(client.auth_client, port).await;
        assert!(response.is_ok());
        assert_eq!(response.unwrap(), "You are the dark lord");
    }

    async fn send_request(client: RpcClientService, port: u16) -> Result<String, ClientError> {
        let server = spawn_server(port).await;

        let response = client
            .request::<String, _>("greet_melkor", rpc_params![])
            .await;

        server.stop().unwrap();
        server.stopped().await;

        response
    }

    /// Spawn a new RPC server equipped with a `JwtLayer` auth middleware.
    async fn spawn_server(port: u16) -> ServerHandle {
        let secret = JwtSecret::from_hex(SECRET).unwrap();
        let addr = format!("{AUTH_ADDR}:{port}");
        let layer = AuthLayer::new(secret);
        let middleware = tower::ServiceBuilder::new().layer(layer);

        // Create a layered server
        let server = ServerBuilder::default()
            .set_http_middleware(middleware)
            .build(addr.parse::<SocketAddr>().unwrap())
            .await
            .unwrap();

        // Create a mock rpc module
        let mut module = RpcModule::new(());
        module
            .register_method("greet_melkor", |_, _, _| "You are the dark lord")
            .unwrap();

        server.start(module)
    }
}
