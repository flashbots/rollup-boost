use crate::EngineApiExt;
use crate::client::auth::AuthLayer;
use crate::client::http::HttpClient as RollupBoostHttpClient;
use crate::payload::{NewPayload, OpExecutionPayloadEnvelope, PayloadSource, PayloadVersion};
use crate::server::EngineApiClient;
use crate::version::{CARGO_PKG_VERSION, VERGEN_GIT_SHA};
use alloy_primitives::{B256, Bytes};
use alloy_rpc_types_engine::{
    ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, JwtError, JwtSecret, PayloadId,
    PayloadStatus,
};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use clap::Parser;
use eyre::bail;
use http::{HeaderMap, Uri};
use jsonrpsee::core::async_trait;
use jsonrpsee::core::middleware::layer::RpcLogger;
use jsonrpsee::http_client::transport::HttpBackend;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder, RpcService};
use jsonrpsee::types::ErrorObjectOwned;
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
    OpPayloadAttributes,
};
use opentelemetry::trace::SpanKind;
use paste::paste;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, instrument};

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
#[derive(Clone, Debug)]
pub struct RpcClient {
    /// Handles requests to the authenticated Engine API (requires JWT authentication)
    auth_client: RpcClientService,
    /// Uri of the RPC server for authenticated Engine API calls
    auth_rpc: Uri,
    /// The source of the payload
    payload_source: PayloadSource,
    /// Optional shadow builder client for async mirroring (fire-and-forget)
    shadow: Option<Arc<RpcClient>>,
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
            shadow: None,
        })
    }

    /// Set the shadow builder client for async mirroring.
    /// When set, all Engine API calls will be asynchronously forwarded to the shadow builder
    /// in a fire-and-forget manner. Errors from the shadow builder are logged but not propagated.
    pub fn with_shadow(mut self, shadow: RpcClient) -> Self {
        self.shadow = Some(Arc::new(shadow));
        self
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
        // Mirror to shadow builder (fire-and-forget)
        if let Some(shadow) = &self.shadow {
            let shadow_client = shadow.auth_client.clone();
            let fork_choice_state = fork_choice_state;
            let payload_attributes = payload_attributes.clone();
            tokio::spawn(async move {
                if let Err(e) = shadow_client
                    .fork_choice_updated_v3(fork_choice_state, payload_attributes)
                    .await
                {
                    debug!(target: "shadow_builder", error = %e, "shadow fork_choice_updated_v3 failed");
                }
            });
        }

        info!("Sending fork_choice_updated_v3 to {}", self.payload_source);
        let res = self
            .auth_client
            .fork_choice_updated_v3(fork_choice_state, payload_attributes.clone())
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
        // Mirror to shadow builder (fire-and-forget)
        if let Some(shadow) = &self.shadow {
            let shadow_client = shadow.auth_client.clone();
            tokio::spawn(async move {
                if let Err(e) = shadow_client.get_payload_v3(payload_id).await {
                    debug!(target: "shadow_builder", error = %e, "shadow get_payload_v3 failed");
                }
            });
        }

        tracing::Span::current().record("payload_id", payload_id.to_string());
        info!("Sending get_payload_v3 to {}", self.payload_source);
        Ok(self
            .auth_client
            .get_payload_v3(payload_id)
            .await
            .set_code()?)
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
        // Mirror to shadow builder (fire-and-forget)
        if let Some(shadow) = &self.shadow {
            let shadow_client = shadow.auth_client.clone();
            let payload = payload.clone();
            let versioned_hashes = versioned_hashes.clone();
            tokio::spawn(async move {
                if let Err(e) = shadow_client
                    .new_payload_v3(payload, versioned_hashes, parent_beacon_block_root)
                    .await
                {
                    debug!(target: "shadow_builder", error = %e, "shadow new_payload_v3 failed");
                }
            });
        }

        info!("Sending new_payload_v3 to {}", self.payload_source);

        let res = self
            .auth_client
            .new_payload_v3(payload, versioned_hashes, parent_beacon_block_root)
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
        // Mirror to shadow builder (fire-and-forget)
        if let Some(shadow) = &self.shadow {
            let shadow_client = shadow.auth_client.clone();
            tokio::spawn(async move {
                if let Err(e) = shadow_client.get_payload_v4(payload_id).await {
                    debug!(target: "shadow_builder", error = %e, "shadow get_payload_v4 failed");
                }
            });
        }

        info!("Sending get_payload_v4 to {}", self.payload_source);
        Ok(self
            .auth_client
            .get_payload_v4(payload_id)
            .await
            .set_code()?)
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
        // Mirror to shadow builder (fire-and-forget)
        if let Some(shadow) = &self.shadow {
            let shadow_client = shadow.auth_client.clone();
            let payload = payload.clone();
            let versioned_hashes = versioned_hashes.clone();
            let execution_requests = execution_requests.clone();
            tokio::spawn(async move {
                if let Err(e) = shadow_client
                    .new_payload_v4(
                        payload,
                        versioned_hashes,
                        parent_beacon_block_root,
                        execution_requests,
                    )
                    .await
                {
                    debug!(target: "shadow_builder", error = %e, "shadow new_payload_v4 failed");
                }
            });
        }

        info!("Sending new_payload_v4 to {}", self.payload_source);

        let res = self
            .auth_client
            .new_payload_v4(
                payload,
                versioned_hashes,
                parent_beacon_block_root,
                execution_requests,
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

    pub async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Block> {
        Ok(self
            .auth_client
            .get_block_by_number(number, full)
            .await
            .set_code()?)
    }
}

#[async_trait]
impl EngineApiExt for RpcClient {
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

    async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Block> {
        self.get_block_by_number(number, full).await
    }
}

#[derive(Debug, Clone)]
pub struct ClientArgs {
    /// Auth server address
    pub url: Uri,

    /// Hex encoded JWT secret to use for the authenticated engine-API RPC server.
    pub jwt_token: Option<JwtSecret>,

    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
    pub jwt_path: Option<PathBuf>,

    /// Timeout for http calls in milliseconds
    pub timeout: u64,
}

impl ClientArgs {
    fn get_auth_jwt(&self) -> eyre::Result<JwtSecret> {
        if let Some(secret) = self.jwt_token {
            Ok(secret)
        } else if let Some(path) = self.jwt_path.as_ref() {
            Ok(JwtSecret::from_file(path)?)
        } else {
            bail!("Missing Client JWT secret");
        }
    }

    pub fn new_rpc_client(&self, payload_source: PayloadSource) -> eyre::Result<RpcClient> {
        RpcClient::new(
            self.url.clone(),
            self.get_auth_jwt()?,
            self.timeout,
            payload_source,
        )
        .map_err(eyre::Report::from)
    }

    pub fn new_http_client(
        &self,
        payload_source: PayloadSource,
    ) -> eyre::Result<RollupBoostHttpClient> {
        Ok(RollupBoostHttpClient::new(
            self.url.clone(),
            self.get_auth_jwt()?,
            payload_source,
            self.timeout,
        ))
    }
}

/// Generates Clap argument structs with a prefix to create a unique namespace when specifying RPC client config via the CLI.
macro_rules! define_client_args {
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


                impl From<$name> for ClientArgs {
                    fn from(args: $name) -> Self {
                        ClientArgs {
                            url: args.[<$prefix _url>].clone(),
                            jwt_token: args.[<$prefix _jwt_token>].clone(),
                            jwt_path: args.[<$prefix _jwt_path>],
                            timeout: args.[<$prefix _timeout>],
                        }
                    }
                }
            }
        )*
    };
}

define_client_args!(
    (BuilderArgs, builder),
    (L2ClientArgs, l2),
    (ShadowBuilderArgs, shadow_builder)
);

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

    mod shadow_tests {
        use super::*;
        use alloy_primitives::{B256, U256};
        use alloy_rpc_types_engine::{
            ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ForkchoiceState,
            ForkchoiceUpdated, PayloadId, PayloadStatus, PayloadStatusEnum,
        };
        use op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV3;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        /// Spawn a mock Engine API server that tracks received requests
        async fn spawn_mock_engine_server(
            port: u16,
            fcu_counter: Arc<AtomicUsize>,
            get_payload_counter: Arc<AtomicUsize>,
            new_payload_counter: Arc<AtomicUsize>,
        ) -> ServerHandle {
            let secret = JwtSecret::from_hex(SECRET).unwrap();
            let addr = format!("127.0.0.1:{port}");
            let layer = AuthLayer::new(secret);
            let middleware = tower::ServiceBuilder::new().layer(layer);

            let server = ServerBuilder::default()
                .set_http_middleware(middleware)
                .build(addr.parse::<SocketAddr>().unwrap())
                .await
                .unwrap();

            let mut module = RpcModule::new(());

            // Register fork_choice_updated_v3
            let fcu_counter_clone = fcu_counter.clone();
            module
                .register_method("engine_forkchoiceUpdatedV3", move |_params, _, _| {
                    fcu_counter_clone.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ErrorObjectOwned>(ForkchoiceUpdated::new(PayloadStatus::from_status(
                        PayloadStatusEnum::Valid,
                    )))
                })
                .unwrap();

            // Register get_payload_v3
            let get_payload_counter_clone = get_payload_counter.clone();
            module
                .register_method("engine_getPayloadV3", move |_params, _, _| {
                    get_payload_counter_clone.fetch_add(1, Ordering::SeqCst);
                    let response = r#"{"executionPayload":{"parentHash":"0xe927a1448525fb5d32cb50ee1408461a945ba6c39bd5cf5621407d500ecc8de9","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x10f8a0830000e8edef6d00cc727ff833f064b1950afd591ae41357f97e543119","receiptsRoot":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0xe0d8b4521a7da1582a713244ffb6a86aa1726932087386e2dc7973f43fc6cb24","blockNumber":"0x1","gasLimit":"0x2ffbd2","gasUsed":"0x0","timestamp":"0x1235","extraData":"0xd883010d00846765746888676f312e32312e30856c696e7578","baseFeePerGas":"0x342770c0","blockHash":"0x44d0fa5f2f73a938ebb96a2a21679eb8dea3e7b7dd8fd9f35aa756dda8bf0a8a","transactions":[],"withdrawals":[],"blobGasUsed":"0x0","excessBlobGas":"0x0"},"blockValue":"0x0","blobsBundle":{"commitments":[],"proofs":[],"blobs":[]},"shouldOverrideBuilder":false,"parentBeaconBlockRoot":"0xdead00000000000000000000000000000000000000000000000000000000beef"}"#;
                    let envelope: OpExecutionPayloadEnvelopeV3 =
                        serde_json::from_str(response).unwrap();
                    Ok::<_, ErrorObjectOwned>(envelope)
                })
                .unwrap();

            // Register new_payload_v3
            let new_payload_counter_clone = new_payload_counter.clone();
            module
                .register_method("engine_newPayloadV3", move |_params, _, _| {
                    new_payload_counter_clone.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ErrorObjectOwned>(PayloadStatus::from_status(PayloadStatusEnum::Valid))
                })
                .unwrap();

            server.start(module)
        }

        fn create_rpc_client(port: u16) -> RpcClient {
            let secret = JwtSecret::from_hex(SECRET).unwrap();
            let uri = Uri::from_str(&format!("http://127.0.0.1:{port}")).unwrap();
            RpcClient::new(uri, secret, 5000, PayloadSource::Builder).unwrap()
        }

        #[tokio::test]
        async fn test_shadow_mirrors_engine_api_calls() {
            // Setup primary and shadow servers
            let primary_port = get_available_port();
            let shadow_port = get_available_port();

            let primary_fcu = Arc::new(AtomicUsize::new(0));
            let primary_get = Arc::new(AtomicUsize::new(0));
            let primary_new = Arc::new(AtomicUsize::new(0));

            let shadow_fcu = Arc::new(AtomicUsize::new(0));
            let shadow_get = Arc::new(AtomicUsize::new(0));
            let shadow_new = Arc::new(AtomicUsize::new(0));

            let primary_server = spawn_mock_engine_server(
                primary_port,
                primary_fcu.clone(),
                primary_get.clone(),
                primary_new.clone(),
            )
            .await;

            let shadow_server = spawn_mock_engine_server(
                shadow_port,
                shadow_fcu.clone(),
                shadow_get.clone(),
                shadow_new.clone(),
            )
            .await;

            // Create primary client with shadow
            let primary_client = create_rpc_client(primary_port);
            let shadow_client = create_rpc_client(shadow_port);
            let client = primary_client.with_shadow(shadow_client);

            // Test fork_choice_updated_v3
            let fcs = ForkchoiceState {
                head_block_hash: B256::ZERO,
                safe_block_hash: B256::ZERO,
                finalized_block_hash: B256::ZERO,
            };
            let result = client.fork_choice_updated_v3(fcs, None).await;
            assert!(result.is_ok());

            // Test get_payload_v3
            let payload_id = PayloadId::new([1, 2, 3, 4, 5, 6, 7, 8]);
            let result = client.get_payload_v3(payload_id).await;
            assert!(result.is_ok());

            // Test new_payload_v3
            let payload = ExecutionPayloadV3 {
                payload_inner: ExecutionPayloadV2 {
                    payload_inner: ExecutionPayloadV1 {
                        base_fee_per_gas: U256::from(7u64),
                        block_number: 1,
                        block_hash: B256::ZERO,
                        logs_bloom: Default::default(),
                        extra_data: Default::default(),
                        gas_limit: 0x1c9c380,
                        gas_used: 0,
                        timestamp: 0,
                        fee_recipient: Default::default(),
                        parent_hash: Default::default(),
                        prev_randao: Default::default(),
                        receipts_root: Default::default(),
                        state_root: Default::default(),
                        transactions: vec![],
                    },
                    withdrawals: vec![],
                },
                blob_gas_used: 0,
                excess_blob_gas: 0,
            };
            let result = client.new_payload_v3(payload, vec![], B256::ZERO).await;
            assert!(result.is_ok());

            // Wait for async shadow calls to complete
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Verify both primary and shadow received all calls
            assert_eq!(primary_fcu.load(Ordering::SeqCst), 1);
            assert_eq!(shadow_fcu.load(Ordering::SeqCst), 1);
            assert_eq!(primary_get.load(Ordering::SeqCst), 1);
            assert_eq!(shadow_get.load(Ordering::SeqCst), 1);
            assert_eq!(primary_new.load(Ordering::SeqCst), 1);
            assert_eq!(shadow_new.load(Ordering::SeqCst), 1);

            primary_server.stop().unwrap();
            shadow_server.stop().unwrap();
        }

        #[tokio::test]
        async fn test_shadow_error_does_not_affect_primary() {
            // Only start primary server - shadow server will be unavailable
            let primary_port = get_available_port();
            let shadow_port = get_available_port(); // No server on this port

            let primary_fcu = Arc::new(AtomicUsize::new(0));
            let primary_get = Arc::new(AtomicUsize::new(0));
            let primary_new = Arc::new(AtomicUsize::new(0));

            let primary_server = spawn_mock_engine_server(
                primary_port,
                primary_fcu.clone(),
                primary_get.clone(),
                primary_new.clone(),
            )
            .await;

            // Create client with shadow pointing to non-existent server
            let primary_client = create_rpc_client(primary_port);
            let shadow_client = create_rpc_client(shadow_port);
            let client = primary_client.with_shadow(shadow_client);

            // Make call - should succeed even though shadow will fail
            let fcs = ForkchoiceState {
                head_block_hash: B256::ZERO,
                safe_block_hash: B256::ZERO,
                finalized_block_hash: B256::ZERO,
            };

            let result = client.fork_choice_updated_v3(fcs, None).await;
            assert!(
                result.is_ok(),
                "Primary call should succeed despite shadow failure"
            );

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Verify primary received the call
            assert_eq!(primary_fcu.load(Ordering::SeqCst), 1);

            primary_server.stop().unwrap();
        }
    }
}
