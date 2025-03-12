#![allow(clippy::complexity)]
use ::tracing::{error, info, Level};
use clap::{arg, Parser, Subcommand};
use client::{BuilderArgs, ExecutionClient, L2ClientArgs};
use debug_api::DebugClient;
use metrics::{init_metrics, ClientMetrics};
use std::{net::SocketAddr, sync::Arc};
use tracing::init_tracing;

use alloy_rpc_types_engine::JwtSecret;
use dotenv::dotenv;
use eyre::bail;
use http::StatusCode;
use hyper::service::service_fn;
use hyper::{server::conn::http1, Request, Response};
use hyper_util::rt::TokioIo;
use jsonrpsee::http_client::HttpBody;
use jsonrpsee::server::Server;
use jsonrpsee::RpcModule;
use metrics_exporter_prometheus::PrometheusHandle;
use proxy::ProxyLayer;
use server::{ExecutionMode, PayloadSource, RollupBoostServer};

use tokio::net::TcpListener;
use tokio::signal::unix::{signal as unix_signal, SignalKind};

mod auth_layer;
mod client;
mod debug_api;
#[cfg(all(feature = "integration", test))]
mod integration;
mod metrics;
mod proxy;
mod server;
mod tracing;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    #[clap(flatten)]
    builder: BuilderArgs,

    #[clap(flatten)]
    l2_client: L2ClientArgs,

    /// Disable using the proposer to sync the builder node
    #[arg(long, env, default_value = "false")]
    no_boost_sync: bool,

    /// Host to run the server on
    #[arg(long, env, default_value = "0.0.0.0")]
    rpc_host: String,

    /// Port to run the server on
    #[arg(long, env, default_value = "8081")]
    rpc_port: u16,

    // Enable tracing
    #[arg(long, env, default_value = "false")]
    tracing: bool,

    // Enable Prometheus metrics
    #[arg(long, env, default_value = "false")]
    metrics: bool,

    /// Host to run the metrics server on
    #[arg(long, env, default_value = "0.0.0.0")]
    metrics_host: String,

    /// Port to run the metrics server on
    #[arg(long, env, default_value = "9090")]
    metrics_port: u16,

    /// OTLP endpoint
    #[arg(long, env, default_value = "http://localhost:4317")]
    otlp_endpoint: String,

    /// Log level
    #[arg(long, env, default_value = "info")]
    log_level: Level,

    /// Log format
    #[arg(long, env, default_value = "text")]
    log_format: LogFormat,

    /// Host to run the debug server on
    #[arg(long, env, default_value = "127.0.0.1")]
    debug_host: String,

    /// Debug server port
    #[arg(long, env, default_value = "5555")]
    debug_server_port: u16,

    /// Execution mode to start rollup boost with
    #[arg(long, env, default_value = "enabled")]
    execution_mode: ExecutionMode,
}

#[derive(Clone, Debug)]
enum LogFormat {
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

#[derive(Subcommand, Debug)]
enum Commands {
    /// Debug commands
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
}

#[derive(Subcommand, Debug)]
enum DebugCommands {
    /// Set the execution mode
    SetExecutionMode { execution_mode: ExecutionMode },

    /// Get the execution mode
    ExecutionMode {},
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load .env file
    dotenv().ok();
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install TLS ring CryptoProvider");
    let args: Args = Args::parse();

    let debug_addr = format!("{}:{}", args.debug_host, args.debug_server_port);

    // Handle commands if present
    if let Some(cmd) = args.command {
        let debug_addr = format!("http://{}", debug_addr);
        return match cmd {
            Commands::Debug { command } => match command {
                DebugCommands::SetExecutionMode { execution_mode } => {
                    let client = DebugClient::new(debug_addr.as_str())?;
                    let result = client.set_execution_mode(execution_mode).await.unwrap();
                    println!("Response: {:?}", result.execution_mode);

                    Ok(())
                }
                DebugCommands::ExecutionMode {} => {
                    let client = DebugClient::new(debug_addr.as_str())?;
                    let result = client.get_execution_mode().await?;
                    println!("Execution mode: {:?}", result.execution_mode);

                    Ok(())
                }
            },
        };
    }

    init_tracing(&args)?;
    let metrics = init_metrics(&args)?;

    let l2_client_args = args.l2_client;

    let l2_auth_jwt = if let Some(secret) = l2_client_args.l2_jwt_token {
        secret
    } else if let Some(path) = l2_client_args.l2_jwt_path.as_ref() {
        JwtSecret::from_file(path)?
    } else {
        bail!("Missing L2 Client JWT secret");
    };

    let l2_metrics = if args.metrics {
        Some(Arc::new(ClientMetrics::new(&PayloadSource::L2)))
    } else {
        None
    };
    let l2_client = ExecutionClient::new(
        l2_client_args.l2_url.clone(),
        l2_auth_jwt,
        l2_client_args.l2_timeout,
        l2_metrics,
        PayloadSource::L2,
    )?;

    let builder_args = args.builder;
    let builder_auth_jwt = if let Some(secret) = builder_args.builder_jwt_token {
        secret
    } else if let Some(path) = builder_args.builder_jwt_path.as_ref() {
        JwtSecret::from_file(path)?
    } else {
        bail!("Missing Builder JWT secret");
    };

    let builder_metrics = if args.metrics {
        Some(Arc::new(ClientMetrics::new(&PayloadSource::Builder)))
    } else {
        None
    };

    let builder_client = ExecutionClient::new(
        builder_args.builder_url.clone(),
        builder_auth_jwt,
        builder_args.builder_timeout,
        builder_metrics,
        PayloadSource::Builder,
    )?;

    let boost_sync_enabled = !args.no_boost_sync;
    if boost_sync_enabled {
        info!("Boost sync enabled");
    }

    let rollup_boost = RollupBoostServer::new(
        l2_client,
        builder_client,
        boost_sync_enabled,
        metrics.clone(),
        args.execution_mode,
    );

    // Spawn the debug server
    rollup_boost.start_debug_server(debug_addr.as_str()).await?;

    let module: RpcModule<()> = rollup_boost.try_into()?;

    // Build and start the server
    info!("Starting server on :{}", args.rpc_port);

    let service_builder = tower::ServiceBuilder::new().layer(ProxyLayer::new(
        l2_client_args.l2_url,
        l2_auth_jwt,
        builder_args.builder_url,
        builder_auth_jwt,
        metrics,
    ));

    let server = Server::builder()
        .set_http_middleware(service_builder)
        .build(format!("{}:{}", args.rpc_host, args.rpc_port).parse::<SocketAddr>()?)
        .await?;
    let handle = server.start(module);

    let stop_handle = handle.clone();

    // Capture SIGINT and SIGTERM
    let mut sigint = unix_signal(SignalKind::interrupt())?;
    let mut sigterm = unix_signal(SignalKind::terminate())?;

    tokio::select! {
        _ = handle.stopped() => {
            // The server has already shut down by itself
            info!("Server stopped");
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

async fn init_metrics_server(addr: SocketAddr, handle: PrometheusHandle) -> eyre::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("Metrics server running on {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let handle = handle.clone(); // Clone the handle for each connection
                tokio::task::spawn(async move {
                    let service = service_fn(move |_req: Request<hyper::body::Incoming>| {
                        let response = match _req.uri().path() {
                            "/metrics" => Response::builder()
                                .header("content-type", "text/plain")
                                .body(HttpBody::from(handle.render()))
                                .unwrap(),
                            _ => Response::builder()
                                .status(StatusCode::NOT_FOUND)
                                .body(HttpBody::empty())
                                .unwrap(),
                        };
                        async { Ok::<_, hyper::Error>(response) }
                    });

                    let io = TokioIo::new(stream);

                    if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                        error!(message = "Error serving metrics connection", error = %err);
                    }
                });
            }
            Err(e) => {
                error!(message = "Error accepting connection", error = %e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_cmd::Command;
    use http::Uri;
    use jsonrpsee::core::client::ClientT;

    use crate::auth_layer::AuthClientService;
    use crate::server::PayloadSource;
    use alloy_rpc_types_engine::JwtSecret;
    use jsonrpsee::http_client::transport::Error as TransportError;
    use jsonrpsee::http_client::transport::HttpBackend;
    use jsonrpsee::http_client::HttpClient;
    use jsonrpsee::RpcModule;
    use jsonrpsee::{
        core::ClientError,
        rpc_params,
        server::{ServerBuilder, ServerHandle},
    };
    use predicates::prelude::*;
    use reth_rpc_layer::{AuthLayer, JwtAuthValidator};
    use std::result::Result;
    use std::str::FromStr;

    use super::*;

    const AUTH_PORT: u32 = 8550;
    const AUTH_ADDR: &str = "0.0.0.0";
    const SECRET: &str = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";

    #[test]
    fn test_invalid_args() {
        let mut cmd = Command::cargo_bin("rollup-boost").unwrap();
        cmd.arg("--invalid-arg");

        cmd.assert().failure().stderr(predicate::str::contains(
            "error: unexpected argument '--invalid-arg' found",
        ));
    }

    #[tokio::test]
    async fn test_create_client() {
        valid_jwt().await;
        invalid_jwt().await;
    }

    async fn valid_jwt() {
        let secret = JwtSecret::from_hex(SECRET).unwrap();

        let auth_rpc = Uri::from_str(&format!("http://{}:{}", AUTH_ADDR, AUTH_PORT)).unwrap();
        let client = ExecutionClient::new(auth_rpc, secret, 1000, None, PayloadSource::L2).unwrap();
        let response = send_request(client.auth_client).await;
        assert!(response.is_ok());
        assert_eq!(response.unwrap(), "You are the dark lord");
    }

    async fn invalid_jwt() {
        let secret = JwtSecret::random();
        let auth_rpc = Uri::from_str(&format!("http://{}:{}", AUTH_ADDR, AUTH_PORT)).unwrap();
        let client = ExecutionClient::new(auth_rpc, secret, 1000, None, PayloadSource::L2).unwrap();
        let response = send_request(client.auth_client).await;
        assert!(response.is_err());
        assert!(matches!(
            response.unwrap_err(),
            ClientError::Transport(e)
                if matches!(e.downcast_ref::<TransportError>(), Some(TransportError::Rejected { status_code: 401 }))
        ));
    }

    async fn send_request(
        client: Arc<HttpClient<AuthClientService<HttpBackend>>>,
    ) -> Result<String, ClientError> {
        let server = spawn_server().await;

        let response = client
            .request::<String, _>("greet_melkor", rpc_params![])
            .await;

        server.stop().unwrap();
        server.stopped().await;

        response
    }

    /// Spawn a new RPC server equipped with a `JwtLayer` auth middleware.
    async fn spawn_server() -> ServerHandle {
        let secret = JwtSecret::from_hex(SECRET).unwrap();
        let addr = format!("{AUTH_ADDR}:{AUTH_PORT}");
        let validator = JwtAuthValidator::new(secret);
        let layer = AuthLayer::new(validator);
        let middleware = tower::ServiceBuilder::default().layer(layer);

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
