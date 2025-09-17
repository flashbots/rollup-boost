use alloy_rpc_types_engine::JwtSecret;
use clap::Parser;
use eyre::bail;
use jsonrpsee::{RpcModule, server::Server};
use parking_lot::Mutex;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::signal::unix::{SignalKind, signal as unix_signal};
use tracing::{Level, info};

use crate::{
    BlockSelectionPolicy, FlashblocksArgs, FlashblocksKeys, ProxyLayer, RollupBoostServer,
    RpcClient,
    client::rpc::{BuilderArgs, L2ClientArgs},
    debug_api::ExecutionMode,
    get_version, init_metrics,
    payload::PayloadSource,
    probe::ProbeLayer,
};

#[derive(Clone, Parser, Debug)]
#[clap(author, version = get_version(), about)]
pub struct RollupBoostArgs {
    #[clap(flatten)]
    pub builder: BuilderArgs,

    #[clap(flatten)]
    pub l2_client: L2ClientArgs,

    /// Duration in seconds between async health checks on the builder
    #[arg(long, env, default_value = "60")]
    pub health_check_interval: u64,

    /// Max duration in seconds between the unsafe head block of the builder and the current time
    #[arg(long, env, default_value = "10")]
    pub max_unsafe_interval: u64,

    /// Host to run the server on
    #[arg(long, env, default_value = "127.0.0.1")]
    pub rpc_host: String,

    /// Port to run the server on
    #[arg(long, env, default_value = "8081")]
    pub rpc_port: u16,

    // Enable tracing
    #[arg(long, env, default_value = "false")]
    pub tracing: bool,

    // Enable Prometheus metrics
    #[arg(long, env, default_value = "false")]
    pub metrics: bool,

    /// Host to run the metrics server on
    #[arg(long, env, default_value = "127.0.0.1")]
    pub metrics_host: String,

    /// Port to run the metrics server on
    #[arg(long, env, default_value = "9090")]
    pub metrics_port: u16,

    /// OTLP endpoint
    #[arg(long, env, default_value = "http://localhost:4317")]
    pub otlp_endpoint: String,

    /// Log level
    #[arg(long, env, default_value = "info")]
    pub log_level: Level,

    /// Log format
    #[arg(long, env, default_value = "text")]
    pub log_format: LogFormat,

    /// Redirect logs to a file
    #[arg(long, env)]
    pub log_file: Option<PathBuf>,

    /// Host to run the debug server on
    #[arg(long, env, default_value = "127.0.0.1")]
    pub debug_host: String,

    /// Debug server port
    #[arg(long, env, default_value = "5555")]
    pub debug_server_port: u16,

    /// Execution mode to start rollup boost with
    #[arg(long, env, default_value = "enabled")]
    pub execution_mode: ExecutionMode,

    #[arg(long, env)]
    pub block_selection_policy: Option<BlockSelectionPolicy>,

    /// Should we use the l2 client for computing state root
    #[arg(long, env, default_value = "false")]
    pub external_state_root: bool,

    /// Allow all engine API calls to builder even when marked as unhealthy
    /// This is default true assuming no builder CL set up
    #[arg(long, env, default_value = "false")]
    pub ignore_unhealthy_builders: bool,

    #[clap(flatten)]
    pub flashblocks: Option<FlashblocksArgs>,
}

impl RollupBoostArgs {
    pub async fn run(self) -> eyre::Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        init_metrics(&self)?;

        let debug_addr = format!("{}:{}", self.debug_host, self.debug_server_port);
        let l2_client_args = self.l2_client;

        let l2_auth_jwt = if let Some(secret) = l2_client_args.l2_jwt_token {
            secret
        } else if let Some(path) = l2_client_args.l2_jwt_path.as_ref() {
            JwtSecret::from_file(path)?
        } else {
            bail!("Missing L2 Client JWT secret");
        };

        let l2_client = RpcClient::new(
            l2_client_args.l2_url.clone(),
            l2_auth_jwt,
            l2_client_args.l2_timeout,
            PayloadSource::L2,
            None,
        )?;

        let builder_args = self.builder;
        let builder_auth_jwt = if let Some(secret) = builder_args.builder_jwt_token {
            secret
        } else if let Some(path) = builder_args.builder_jwt_path.as_ref() {
            JwtSecret::from_file(path)?
        } else {
            bail!("Missing Builder JWT secret");
        };

        let flashblocks_keys = self.flashblocks.as_ref().map(|fb| FlashblocksKeys {
            authorization_sk: fb.flashblocks_authorizer_sk.clone(),
            builder_pk: fb.flashblocks_builder_vk.clone(),
        });

        let builder_client = RpcClient::new(
            builder_args.builder_url.clone(),
            builder_auth_jwt,
            builder_args.builder_timeout,
            PayloadSource::Builder,
            flashblocks_keys,
        )?;

        let (probe_layer, probes) = ProbeLayer::new();
        let execution_mode = Arc::new(Mutex::new(self.execution_mode));

<<<<<<< HEAD
        let (rpc_module, health_handle): (RpcModule<()>, _) = {
=======
        let (rpc_module, health_handle): (RpcModule<()>, _) = if self.flashblocks.flashblocks {
            let flashblocks_args = self.flashblocks;
            let inbound_url = flashblocks_args.flashblocks_builder_url;
            let outbound_addr = SocketAddr::new(
                IpAddr::from_str(&flashblocks_args.flashblocks_host)?,
                flashblocks_args.flashblocks_port,
            );

            let builder_client = Arc::new(Flashblocks::run(
                builder_client.clone(),
                inbound_url,
                outbound_addr,
                flashblocks_args.flashblock_builder_ws_reconnect_ms,
            )?);

            let rollup_boost = RollupBoostServer::new(
                l2_client,
                builder_client,
                execution_mode.clone(),
                self.block_selection_policy,
                probes.clone(),
                self.external_state_root,
                self.ignore_unhealthy_builders,
            );

            let health_handle = rollup_boost
                .spawn_health_check(self.health_check_interval, self.max_unsafe_interval);

            // Spawn the debug server
            rollup_boost.start_debug_server(debug_addr.as_str()).await?;
            (rollup_boost.try_into()?, health_handle)
        } else {
>>>>>>> main
            let rollup_boost = RollupBoostServer::new(
                l2_client,
                Arc::new(builder_client),
                execution_mode.clone(),
                self.block_selection_policy,
                probes.clone(),
                self.external_state_root,
                self.ignore_unhealthy_builders,
            );

            let health_handle = rollup_boost
                .spawn_health_check(self.health_check_interval, self.max_unsafe_interval);

            // Spawn the debug server
            rollup_boost.start_debug_server(debug_addr.as_str()).await?;
            (rollup_boost.try_into()?, health_handle)
        };

        // Build and start the server
        info!("Starting server on :{}", self.rpc_port);

        let http_middleware =
            tower::ServiceBuilder::new()
                .layer(probe_layer)
                .layer(ProxyLayer::new(
                    l2_client_args.l2_url,
                    l2_auth_jwt,
                    l2_client_args.l2_timeout,
                    builder_args.builder_url,
                    builder_auth_jwt,
                    builder_args.builder_timeout,
                ));

        let server = Server::builder()
            .set_http_middleware(http_middleware)
            .build(format!("{}:{}", self.rpc_host, self.rpc_port).parse::<SocketAddr>()?)
            .await?;
        let handle = server.start(rpc_module);

        let stop_handle = handle.clone();

        // Capture SIGINT and SIGTERM
        let mut sigint = unix_signal(SignalKind::interrupt())?;
        let mut sigterm = unix_signal(SignalKind::terminate())?;

        tokio::select! {
            _ = handle.stopped() => {
                // The server has already shut down by itself
                info!("Server stopped");
            }
            _ = health_handle => {
                info!("Health check task stopped");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT, shutting down gracefully...");
                let _ = stop_handle.stop();
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down gracefully...");
                let _ = stop_handle.stop();
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum LogFormat {
    Json,
    Text,
}

impl std::str::FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(LogFormat::Json),
            "text" => Ok(LogFormat::Text),
            _ => Err("Invalid log format".into()),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::result::Result;

    use super::*;

    const SECRET: &str = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";
    const FLASHBLOCKS_SK: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const FLASHBLOCKS_VK: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    #[test]
    fn test_parse_args_minimal() -> Result<(), Box<dyn std::error::Error>> {
        let args = RollupBoostArgs::try_parse_from(["rollup-boost"])?;

        assert!(!args.tracing);
        assert!(!args.metrics);
        assert_eq!(args.rpc_host, "127.0.0.1");
        assert_eq!(args.rpc_port, 8081);
        assert!(args.flashblocks.is_none());

        Ok(())
    }

    #[test]
    fn test_parse_args_missing_flashblocks_flag() -> Result<(), Box<dyn std::error::Error>> {
        let args = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--flashblocks-authorizer-sk",
            FLASHBLOCKS_SK,
            "--flashblocks-builder-vk",
            FLASHBLOCKS_VK,
        ]);

        assert!(
            args.is_err(),
            "flashblocks args should be invalid without --flashblocks flag"
        );

        Ok(())
    }

    #[test]
    fn test_parse_args_with_flashblocks_flag() -> Result<(), Box<dyn std::error::Error>> {
        let args = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--flashblocks",
            "--flashblocks-authorizer-sk",
            FLASHBLOCKS_SK,
            "--flashblocks-builder-vk",
            FLASHBLOCKS_VK,
        ])?;

        let flashblocks = args
            .flashblocks
            .expect("flashblocks should be Some when flag is passed");
        assert!(flashblocks.flashblocks);

        Ok(())
    }

    #[test]
    fn test_parse_args_with_flashblocks_custom_values() -> Result<(), Box<dyn std::error::Error>> {
        let args = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--flashblocks",
            "--flashblocks-authorizer-sk",
            FLASHBLOCKS_SK,
            "--flashblocks-builder-vk",
            FLASHBLOCKS_VK,
        ])?;

        let flashblocks = args
            .flashblocks
            .expect("flashblocks should be Some when flag is passed");
        assert!(flashblocks.flashblocks);

        Ok(())
    }

    #[test]
    fn test_parse_args_with_all_options() -> Result<(), Box<dyn std::error::Error>> {
        let args = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--health-check-interval",
            "120",
            "--max-unsafe-interval",
            "20",
            "--rpc-host",
            "0.0.0.0",
            "--rpc-port",
            "9090",
            "--tracing",
            "--metrics",
            "--metrics-host",
            "192.168.1.1",
            "--metrics-port",
            "8080",
            "--log-level",
            "debug",
            "--log-format",
            "json",
            "--debug-host",
            "localhost",
            "--debug-server-port",
            "6666",
            "--execution-mode",
            "disabled",
            "--flashblocks",
            "--flashblocks-authorizer-sk",
            FLASHBLOCKS_SK,
            "--flashblocks-builder-vk",
            FLASHBLOCKS_VK,
        ])?;

        assert_eq!(args.health_check_interval, 120);
        assert_eq!(args.max_unsafe_interval, 20);
        assert_eq!(args.rpc_host, "0.0.0.0");
        assert_eq!(args.rpc_port, 9090);
        assert!(args.tracing);
        assert!(args.metrics);
        assert_eq!(args.metrics_host, "192.168.1.1");
        assert_eq!(args.metrics_port, 8080);
        assert_eq!(args.log_level, Level::DEBUG);
        assert_eq!(args.debug_host, "localhost");
        assert_eq!(args.debug_server_port, 6666);

        let flashblocks = args
            .flashblocks
            .expect("flashblocks should be Some when flag is passed");
        assert!(flashblocks.flashblocks);

        Ok(())
    }

    #[test]
    fn test_parse_args_missing_jwt_succeeds_at_parse_time() {
        // JWT validation happens at runtime, not parse time, so this should succeed
        let result =
            RollupBoostArgs::try_parse_from(["rollup-boost", "--builder-jwt-token", SECRET]);

        assert!(result.is_ok());
        let args = result.unwrap();
        assert!(args.builder.builder_jwt_token.is_some());
        assert!(args.l2_client.l2_jwt_token.is_none());
    }

    #[test]
    fn test_parse_args_invalid_flashblocks_sk() {
        let result = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--flashblocks",
            "--flashblocks-authorizer-sk",
            "invalid_hex",
            "--flashblocks-builder-vk",
            FLASHBLOCKS_VK,
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_args_invalid_flashblocks_vk() {
        let result = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--flashblocks",
            "--flashblocks-authorizer-sk",
            FLASHBLOCKS_SK,
            "--flashblocks-builder-vk",
            "invalid_hex",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn test_log_format_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let json_args = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--log-format",
            "json",
        ])?;

        match json_args.log_format {
            LogFormat::Json => {}
            LogFormat::Text => panic!("Expected Json format"),
        }

        let text_args = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-token",
            SECRET,
            "--l2-jwt-token",
            SECRET,
            "--log-format",
            "text",
        ])?;

        match text_args.log_format {
            LogFormat::Text => {}
            LogFormat::Json => panic!("Expected Text format"),
        }

        Ok(())
    }

    #[test]
    fn test_parse_args_with_jwt_paths() -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut builder_jwt_file = NamedTempFile::new()?;
        writeln!(builder_jwt_file, "{}", SECRET)?;
        let builder_jwt_path = builder_jwt_file.path();

        let mut l2_jwt_file = NamedTempFile::new()?;
        writeln!(l2_jwt_file, "{}", SECRET)?;
        let l2_jwt_path = l2_jwt_file.path();

        let args = RollupBoostArgs::try_parse_from([
            "rollup-boost",
            "--builder-jwt-path",
            builder_jwt_path.to_str().unwrap(),
            "--l2-jwt-path",
            l2_jwt_path.to_str().unwrap(),
        ])?;

        assert!(args.builder.builder_jwt_path.is_some());
        assert!(args.l2_client.l2_jwt_path.is_some());

        Ok(())
    }
}
