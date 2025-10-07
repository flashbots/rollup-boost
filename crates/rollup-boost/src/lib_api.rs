//! Library API for rollup-boost
//!
//! This module provides a programmatic interface for embedding rollup-boost directly
//! into a consensus layer client (like kona-node), eliminating the need for separate
//! HTTP server processes and JWT authentication overhead.
//!
//! # Overview
//!
//! rollup-boost acts as an intelligent proxy between a consensus layer (CL) and execution
//! layer (EL), providing:
//! - Block building via external builders
//! - Automatic fallback to L2 sequencer when builders are unavailable
//! - Configurable block selection policies
//! - Health monitoring of builder connections
//! - Multiple execution modes (Enabled, DryRun, Disabled)
//!
//! # Usage Example
//!
//! ```no_run
//! use rollup_boost::{RollupBoostConfigBuilder, RollupBoost, ExecutionMode};
//! use alloy_rpc_types_eth::BlockNumberOrTag;
//! use std::str::FromStr;
//! use http::Uri;
//!
//! # async fn example() -> eyre::Result<()> {
//! // Configure rollup-boost
//! let config = RollupBoostConfigBuilder::new()
//!     .l2_jwt_secret_file(std::path::Path::new("/path/to/l2_jwt.hex"))?
//!     .builder_jwt_secret_file(std::path::Path::new("/path/to/builder_jwt.hex"))?
//!     .l2_url(Uri::from_str("http://localhost:8551")?)
//!     .builder_url(Uri::from_str("http://builder.example.com:8551")?)
//!     .l2_timeout(5000)
//!     .builder_timeout(3000)
//!     .execution_mode(ExecutionMode::Enabled)
//!     .external_state_root(false)
//!     .ignore_unhealthy_builders(false)
//!     .build();
//!
//! // Create rollup-boost instance
//! let mut rollup_boost = RollupBoost::new(config)?;
//!
//! // Optional: Start background health checking
//! rollup_boost.start_health_check(60, 10)?;
//!
//! // Use Engine API methods
//! let block = rollup_boost.get_block_by_number(
//!     BlockNumberOrTag::Latest,
//!     false
//! ).await?;
//!
//! // Check health status
//! let health = rollup_boost.health();
//!
//! // Change execution mode at runtime
//! rollup_boost.set_execution_mode(ExecutionMode::DryRun);
//!
//! # Ok(())
//! # }
//! ```
//!
//! # Configuration
//!
//! Use [`RollupBoostConfigBuilder`] to construct a [`RollupBoostConfig`]:
//!
//! - **JWT Secrets**: Can be loaded from hex strings or files
//! - **URLs**: HTTP/HTTPS endpoints for L2 and builder
//! - **Timeouts**: Request timeouts in milliseconds (recommended: 3000-5000ms)
//! - **Execution Mode**: Controls builder usage (see [`ExecutionMode`])
//! - **Block Selection**: Optional policy for choosing between L2 and builder blocks
//! - **External State Root**: Whether state root computation is handled externally
//! - **Ignore Unhealthy Builders**: Skip builder calls when health checks fail
//!
//! # Execution Modes
//!
//! - **Enabled**: Full builder usage, falls back to L2 on errors
//! - **DryRun**: Requests blocks from both but always returns L2 block
//! - **Disabled**: Only uses L2, no builder calls
//!
//! # Health Checking
//!
//! Health checks are optional for library usage. Call [`RollupBoost::start_health_check`]
//! to enable periodic monitoring:
//!
//! - `health_check_interval`: Seconds between checks (recommended: 60)
//! - `max_unsafe_interval`: Maximum blocks without builder response before marking unhealthy
//!
//! Query health status with [`RollupBoost::health`]:
//! - `Healthy`: Builder is building blocks
//! - `PartialContent`: L2 is building, builder is not
//! - `ServiceUnavailable`: Neither is building blocks
//!
//! # Metrics Integration
//!
//! RollupBoost records operational metrics using the [`metrics`] crate. To export
//! these metrics, configure a global metrics recorder in your application before
//! creating the `RollupBoost` instance.
//!
//! The library records metrics but does not export them - this allows you to
//! integrate with your existing metrics infrastructure (Prometheus, StatsD,
//! Datadog, etc.).
//!
//! ## Example: Prometheus Integration
//!
//! ```no_run
//! use metrics_exporter_prometheus::PrometheusBuilder;
//! use rollup_boost::{RollupBoost, RollupBoostConfigBuilder};
//!
//! # async fn example() -> eyre::Result<()> {
//! // Set up Prometheus exporter before creating RollupBoost
//! let builder = PrometheusBuilder::new();
//! builder.install().expect("failed to install metrics recorder");
//!
//! // Now all RollupBoost metrics will be exported via Prometheus
//! let config = RollupBoostConfigBuilder::new().build();
//! let rollup_boost = RollupBoost::new(config)?;
//!
//! // Metrics endpoint available at http://localhost:9000/metrics (default)
//! # Ok(())
//! # }
//! ```
//!
//! ## Example: Custom HTTP Listener
//!
//! ```no_run
//! use metrics_exporter_prometheus::PrometheusBuilder;
//! use std::net::SocketAddr;
//!
//! # async fn example() -> eyre::Result<()> {
//! // Expose metrics on custom port
//! let addr: SocketAddr = "0.0.0.0:9090".parse()?;
//! PrometheusBuilder::new()
//!     .with_http_listener(addr)
//!     .install()
//!     .expect("failed to install recorder");
//! # Ok(())
//! # }
//! ```
//!
//! ## Recorded Metrics
//!
//! RollupBoost records the following metrics:
//!
//! - **`rpc.blocks_created`** (counter, labels: `source`)
//!   - Tracks blocks created by source (`L2` or `Builder`)
//!   - Use to monitor builder vs L2 block production ratio
//!
//! - **`rollup_boost_execution_mode`** (gauge)
//!   - Current execution mode: `0` = Disabled, `1` = DryRun, `2` = Enabled
//!   - Only updated if you call [`update_execution_mode_gauge`]
//!
//! ## No Metrics Setup
//!
//! If no metrics recorder is installed, metrics calls are no-ops with near-zero
//! overhead. This is safe for testing or if you don't need metrics.
//!
//! [`metrics`]: https://docs.rs/metrics

use crate::{
    client::rpc::RpcClient,
    debug_api::ExecutionMode,
    probe::Health,
    server::{EngineApiServer, RollupBoostServer},
    selection::BlockSelectionPolicy,
};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use parking_lot::Mutex;
use std::sync::Arc;

/// Configuration for rollup-boost library usage.
///
/// Use [`RollupBoostConfigBuilder`] to construct this with proper validation
/// and convenient JWT secret loading from files or hex strings.
///
/// # Security Note
///
/// JWT secrets should be loaded from secure files, not hardcoded. The default
/// implementation uses all-zero secrets which are only suitable for testing.
#[derive(Debug, Clone)]
pub struct RollupBoostConfig {
    /// L2 client JWT secret (resolved from file or hex)
    pub l2_jwt_secret: alloy_rpc_types_engine::JwtSecret,
    /// Builder client JWT secret (resolved from file or hex)
    pub builder_jwt_secret: alloy_rpc_types_engine::JwtSecret,
    /// L2 client URL
    pub l2_url: http::Uri,
    /// Builder client URL
    pub builder_url: http::Uri,
    /// L2 client timeout in milliseconds
    pub l2_timeout: u64,
    /// Builder client timeout in milliseconds
    pub builder_timeout: u64,
    /// Execution mode for rollup-boost
    pub execution_mode: ExecutionMode,
    /// Block selection policy for choosing between L2 and builder payloads
    pub block_selection_policy: Option<BlockSelectionPolicy>,
    /// Whether to use external state root computation
    pub external_state_root: bool,
    /// Whether to ignore unhealthy builders when making calls
    pub ignore_unhealthy_builders: bool,
}

impl Default for RollupBoostConfig {
    fn default() -> Self {
        Self {
            l2_jwt_secret: alloy_rpc_types_engine::JwtSecret::from_hex("0x0000000000000000000000000000000000000000000000000000000000000000").unwrap(),
            builder_jwt_secret: alloy_rpc_types_engine::JwtSecret::from_hex("0x0000000000000000000000000000000000000000000000000000000000000000").unwrap(),
            l2_url: "http://127.0.0.1:8551".parse().unwrap(),
            builder_url: "http://127.0.0.1:8551".parse().unwrap(),
            l2_timeout: 1000,
            builder_timeout: 1000,
            execution_mode: ExecutionMode::Enabled,
            block_selection_policy: None,
            external_state_root: false,
            ignore_unhealthy_builders: false,
        }
    }
}

/// Builder for creating [`RollupBoostConfig`] with convenient JWT secret loading.
///
/// This builder provides a fluent interface for constructing configuration,
/// with special methods for loading JWT secrets from files or hex strings.
///
/// # Example
///
/// ```no_run
/// use rollup_boost::RollupBoostConfigBuilder;
/// use std::path::Path;
/// use http::Uri;
/// use std::str::FromStr;
///
/// # fn example() -> eyre::Result<()> {
/// let config = RollupBoostConfigBuilder::new()
///     .l2_jwt_secret_file(Path::new("/secrets/l2.hex"))?
///     .builder_jwt_secret_hex("0x1234...")?
///     .l2_url(Uri::from_str("http://localhost:8551")?)
///     .builder_url(Uri::from_str("http://builder:8551")?)
///     .build();
/// # Ok(())
/// # }
/// ```
pub struct RollupBoostConfigBuilder {
    config: RollupBoostConfig,
}

impl RollupBoostConfigBuilder {
    /// Create a new builder with default configuration.
    ///
    /// Default values are suitable for testing but should be overridden for production:
    /// - JWT secrets: All zeros (insecure, test-only)
    /// - URLs: localhost:8551 for both L2 and builder
    /// - Timeouts: 1000ms (may be too short for production)
    /// - Execution mode: Enabled
    /// - External state root: false
    /// - Ignore unhealthy builders: false
    pub fn new() -> Self {
        Self {
            config: RollupBoostConfig::default(),
        }
    }

    /// Set L2 JWT secret from hex string
    pub fn l2_jwt_secret_hex(mut self, secret: &str) -> eyre::Result<Self> {
        self.config.l2_jwt_secret = alloy_rpc_types_engine::JwtSecret::from_hex(secret)?;
        Ok(self)
    }

    /// Set L2 JWT secret from file path
    pub fn l2_jwt_secret_file(mut self, path: &std::path::Path) -> eyre::Result<Self> {
        self.config.l2_jwt_secret = alloy_rpc_types_engine::JwtSecret::from_file(path)?;
        Ok(self)
    }

    /// Set Builder JWT secret from hex string
    pub fn builder_jwt_secret_hex(mut self, secret: &str) -> eyre::Result<Self> {
        self.config.builder_jwt_secret = alloy_rpc_types_engine::JwtSecret::from_hex(secret)?;
        Ok(self)
    }

    /// Set Builder JWT secret from file path
    pub fn builder_jwt_secret_file(mut self, path: &std::path::Path) -> eyre::Result<Self> {
        self.config.builder_jwt_secret = alloy_rpc_types_engine::JwtSecret::from_file(path)?;
        Ok(self)
    }

    /// Set L2 URL
    pub fn l2_url(mut self, url: http::Uri) -> Self {
        self.config.l2_url = url;
        self
    }

    /// Set Builder URL
    pub fn builder_url(mut self, url: http::Uri) -> Self {
        self.config.builder_url = url;
        self
    }

    /// Set L2 timeout
    pub fn l2_timeout(mut self, timeout: u64) -> Self {
        self.config.l2_timeout = timeout;
        self
    }

    /// Set Builder timeout
    pub fn builder_timeout(mut self, timeout: u64) -> Self {
        self.config.builder_timeout = timeout;
        self
    }

    /// Set execution mode
    pub fn execution_mode(mut self, mode: ExecutionMode) -> Self {
        self.config.execution_mode = mode;
        self
    }

    /// Set block selection policy
    pub fn block_selection_policy(mut self, policy: BlockSelectionPolicy) -> Self {
        self.config.block_selection_policy = Some(policy);
        self
    }

    /// Set external state root computation
    pub fn external_state_root(mut self, enabled: bool) -> Self {
        self.config.external_state_root = enabled;
        self
    }

    /// Set whether to ignore unhealthy builders
    pub fn ignore_unhealthy_builders(mut self, ignore: bool) -> Self {
        self.config.ignore_unhealthy_builders = ignore;
        self
    }

    /// Build the final configuration
    pub fn build(self) -> RollupBoostConfig {
        self.config
    }
}

/// Main library interface for rollup-boost.
///
/// This struct provides the primary API for embedding rollup-boost functionality
/// directly into a consensus layer client. It wraps the internal server logic
/// and exposes Engine API methods as library calls.
///
/// # Lifecycle
///
/// 1. Create instance with [`RollupBoost::new`]
/// 2. Optionally start health checking with [`RollupBoost::start_health_check`]
/// 3. Call Engine API methods as needed
/// 4. Instance automatically cleans up on drop (stops health checks)
///
/// # Thread Safety
///
/// `RollupBoost` is `Send` and can be safely shared across threads. Internal
/// state uses `Arc` and `Mutex` for thread-safe access.
///
/// # Example
///
/// ```no_run
/// use rollup_boost::{RollupBoost, RollupBoostConfigBuilder, ExecutionMode};
/// use http::Uri;
/// use std::str::FromStr;
///
/// # async fn example() -> eyre::Result<()> {
/// let config = RollupBoostConfigBuilder::new()
///     .l2_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
///     .builder_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
///     .l2_url(Uri::from_str("http://localhost:8551")?)
///     .builder_url(Uri::from_str("http://builder:8551")?)
///     .build();
///
/// let rollup_boost = RollupBoost::new(config)?;
///
/// // Use Engine API
/// let mode = rollup_boost.execution_mode();
/// let health = rollup_boost.health();
/// # Ok(())
/// # }
/// ```
pub struct RollupBoost {
    server: RollupBoostServer<RpcClient>,
    health_handle: Option<tokio::task::JoinHandle<()>>,
}

impl RollupBoost {
    /// Create a new rollup-boost instance with the given configuration.
    ///
    /// This initializes RPC clients for both L2 and builder endpoints,
    /// sets up internal state tracking, and prepares the Engine API proxy.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - RPC client initialization fails (invalid URLs, network issues)
    /// - JWT secret validation fails
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rollup_boost::{RollupBoost, RollupBoostConfigBuilder};
    /// use http::Uri;
    /// use std::str::FromStr;
    ///
    /// # fn example() -> eyre::Result<()> {
    /// let config = RollupBoostConfigBuilder::new()
    ///     .l2_url(Uri::from_str("http://localhost:8551")?)
    ///     .builder_url(Uri::from_str("http://builder:8551")?)
    ///     .build();
    ///
    /// let rollup_boost = RollupBoost::new(config)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: RollupBoostConfig) -> eyre::Result<Self> {
        let l2_client = RpcClient::new(
            config.l2_url.clone(),
            config.l2_jwt_secret,
            config.l2_timeout,
            crate::payload::PayloadSource::L2,
        )?;

        let builder_client = RpcClient::new(
            config.builder_url.clone(),
            config.builder_jwt_secret,
            config.builder_timeout,
            crate::payload::PayloadSource::Builder,
        )?;

        let (_probe_layer, probes) = crate::probe::ProbeLayer::new();
        let execution_mode = Arc::new(Mutex::new(config.execution_mode));

        // For library usage, set initial health to Healthy since we don't run health checks
        probes.set_health(Health::Healthy);

        let server = RollupBoostServer::new(
            l2_client,
            Arc::new(builder_client),
            execution_mode,
            config.block_selection_policy,
            probes,
            config.external_state_root,
            config.ignore_unhealthy_builders,
        );

        Ok(Self {
            server,
            health_handle: None,
        })
    }

    /// Start background health checking of builder connectivity.
    ///
    /// This spawns a background task that periodically checks if the builder
    /// is responding and building blocks. Health status can be queried with
    /// [`RollupBoost::health`].
    ///
    /// # Parameters
    ///
    /// - `health_check_interval`: Seconds between health checks (recommended: 60)
    /// - `max_unsafe_interval`: Maximum number of blocks without builder response
    ///   before marking as unhealthy (recommended: 10)
    ///
    /// # Note
    ///
    /// Health checking is optional for library usage. If not started, health
    /// status will reflect real-time API call results rather than background
    /// monitoring.
    ///
    /// Only one health check task can run at a time. Calling this multiple
    /// times will replace the previous task.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rollup_boost::{RollupBoost, RollupBoostConfigBuilder};
    ///
    /// # async fn example() -> eyre::Result<()> {
    /// # let config = RollupBoostConfigBuilder::new().build();
    /// let mut rollup_boost = RollupBoost::new(config)?;
    ///
    /// // Check every 60 seconds, mark unhealthy after 10 blocks without response
    /// rollup_boost.start_health_check(60, 10)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn start_health_check(
        &mut self,
        health_check_interval: u64,
        max_unsafe_interval: u64,
    ) -> eyre::Result<()> {
        let handle = self.server.spawn_health_check(health_check_interval, max_unsafe_interval);
        self.health_handle = Some(handle);
        Ok(())
    }

    /// Stop background health checking if it was started.
    ///
    /// This aborts the health check task. The instance will continue to function
    /// normally, but health status will no longer be updated in the background.
    ///
    /// This is called automatically when the `RollupBoost` instance is dropped,
    /// so explicit calls are only needed if you want to stop health checking
    /// while keeping the instance alive.
    pub async fn stop_health_check(&mut self) {
        if let Some(handle) = self.health_handle.take() {
            handle.abort();
        }
    }

    /// Get current execution mode.
    ///
    /// Returns the active [`ExecutionMode`]:
    /// - `Enabled`: Using builder with L2 fallback
    /// - `DryRun`: Testing both but always using L2
    /// - `Disabled`: Only using L2
    pub fn execution_mode(&self) -> ExecutionMode {
        self.server.execution_mode()
    }

    /// Set execution mode at runtime.
    ///
    /// This allows dynamic switching between execution modes without
    /// restarting the service. Useful for:
    /// - Temporarily disabling builder during maintenance
    /// - Testing builder integration in DryRun mode
    /// - Emergency fallback to L2-only operation
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rollup_boost::{RollupBoost, RollupBoostConfigBuilder, ExecutionMode};
    ///
    /// # fn example() -> eyre::Result<()> {
    /// # let config = RollupBoostConfigBuilder::new().build();
    /// let rollup_boost = RollupBoost::new(config)?;
    ///
    /// // Switch to DryRun for testing
    /// rollup_boost.set_execution_mode(ExecutionMode::DryRun);
    ///
    /// // Later, re-enable builder
    /// rollup_boost.set_execution_mode(ExecutionMode::Enabled);
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_execution_mode(&self, mode: ExecutionMode) {
        self.server.set_execution_mode(mode);
    }

    /// Get current health status.
    ///
    /// Returns the current [`Health`] status:
    /// - `Healthy`: Builder is building blocks successfully
    /// - `PartialContent`: L2 is building, but builder is not
    /// - `ServiceUnavailable`: Neither L2 nor builder is responding
    ///
    /// If background health checking is not started, this reflects the
    /// most recent API call results. With background health checking,
    /// this provides continuous monitoring.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rollup_boost::{RollupBoost, RollupBoostConfigBuilder, Health};
    ///
    /// # fn example() -> eyre::Result<()> {
    /// # let config = RollupBoostConfigBuilder::new().build();
    /// let rollup_boost = RollupBoost::new(config)?;
    ///
    /// match rollup_boost.health() {
    ///     Health::Healthy => println!("All systems operational"),
    ///     Health::PartialContent => println!("Builder unavailable, using L2"),
    ///     Health::ServiceUnavailable => println!("Critical: No block production"),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn health(&self) -> Health {
        self.server.probes().health()
    }

    // Engine API methods
    //
    // These methods implement the Ethereum Engine API, providing block building
    // and payload management functionality. They proxy requests to both L2 and
    // builder endpoints based on the configured execution mode.

    /// Send forkchoice update to both L2 and builder clients.
    ///
    /// This Engine API method updates the fork choice state and optionally
    /// requests payload building. It's sent to both L2 and builder to keep
    /// them synchronized.
    ///
    /// # Parameters
    ///
    /// - `fork_choice_state`: Current head, safe, and finalized block hashes
    /// - `payload_attributes`: Optional attributes for building a new payload
    ///
    /// # Returns
    ///
    /// Returns the forkchoice update response, including payload ID if
    /// payload attributes were provided.
    ///
    /// # Errors
    ///
    /// Returns an error if both L2 and builder fail to respond, or if
    /// responses are inconsistent.
    pub async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> eyre::Result<ForkchoiceUpdated> {
        Ok(self.server.fork_choice_updated_v3(fork_choice_state, payload_attributes).await?)
    }

    /// Get execution payload V3 from either L2 or builder.
    ///
    /// This retrieves a previously requested payload. Based on the execution
    /// mode and block selection policy, this may return:
    /// - Builder payload (if available and execution mode is Enabled)
    /// - L2 payload (as fallback or if execution mode is Disabled)
    ///
    /// # Parameters
    ///
    /// - `payload_id`: Identifier returned from previous `fork_choice_updated` call
    ///
    /// # Returns
    ///
    /// Returns the execution payload envelope containing the block data.
    ///
    /// # Errors
    ///
    /// Returns an error if neither L2 nor builder can provide the payload.
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV3> {
        Ok(self.server.get_payload_v3(payload_id).await?)
    }

    /// Get execution payload V4 from either L2 or builder.
    ///
    /// Similar to `get_payload_v3` but for the V4 Engine API version,
    /// which includes additional execution requests data.
    ///
    /// # Parameters
    ///
    /// - `payload_id`: Identifier returned from previous `fork_choice_updated` call
    ///
    /// # Returns
    ///
    /// Returns the execution payload envelope with V4-specific data.
    pub async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> eyre::Result<op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV4> {
        Ok(self.server.get_payload_v4(payload_id).await?)
    }

    /// Submit new payload V3 to L2 client for validation.
    ///
    /// This sends a new execution payload to the L2 client for validation
    /// and inclusion in the chain. If execution mode is Enabled, it also
    /// forwards to the builder asynchronously to keep it synchronized.
    ///
    /// # Parameters
    ///
    /// - `payload`: The execution payload to validate
    /// - `versioned_hashes`: Blob versioned hashes for EIP-4844
    /// - `parent_beacon_block_root`: Parent beacon block root for EIP-4788
    ///
    /// # Returns
    ///
    /// Returns the payload validation status (VALID, INVALID, SYNCING, etc.)
    pub async fn new_payload_v3(
        &self,
        payload: alloy_rpc_types_engine::ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> eyre::Result<PayloadStatus> {
        Ok(self.server.new_payload_v3(payload, versioned_hashes, parent_beacon_block_root).await?)
    }

    /// Submit new payload V4 to L2 client for validation.
    ///
    /// Similar to `new_payload_v3` but for the V4 Engine API version,
    /// which includes execution requests.
    ///
    /// # Parameters
    ///
    /// - `payload`: The execution payload to validate
    /// - `versioned_hashes`: Blob versioned hashes for EIP-4844
    /// - `parent_beacon_block_root`: Parent beacon block root for EIP-4788
    /// - `execution_requests`: Execution layer requests (EIP-7685)
    pub async fn new_payload_v4(
        &self,
        payload: op_alloy_rpc_types_engine::OpExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<alloy_primitives::Bytes>,
    ) -> eyre::Result<PayloadStatus> {
        Ok(self.server.new_payload_v4(payload, versioned_hashes, parent_beacon_block_root, execution_requests).await?)
    }

    /// Get block by number from L2 client.
    ///
    /// This is a standard Ethereum JSON-RPC method that retrieves block data
    /// by number or tag. It always queries the L2 client (not the builder).
    ///
    /// # Parameters
    ///
    /// - `number`: Block number or tag (Latest, Earliest, Pending, etc.)
    /// - `full`: If true, include full transaction objects; if false, only hashes
    ///
    /// # Returns
    ///
    /// Returns the requested block data.
    ///
    /// # Errors
    ///
    /// Returns an error if the L2 client fails to respond or the block doesn't exist.
    pub async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> eyre::Result<Block> {
        Ok(self.server.get_block_by_number(number, full).await?)
    }
}

impl Drop for RollupBoost {
    fn drop(&mut self) {
        if let Some(handle) = &self.health_handle {
            handle.abort();
        }
    }
}
