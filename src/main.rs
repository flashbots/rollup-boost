#[cfg(all(feature = "integration", test))]
mod integration;
mod metrics;
mod registry;
mod server;
mod subscriber;

use crate::metrics::Metrics;
use crate::registry::Registry;
use crate::server::Server;
use crate::subscriber::WebsocketSubscriber;
use axum::http::Uri;
use clap::Parser;
use dotenv::dotenv;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, Level};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(
        long,
        env,
        default_value = "0.0.0.0:8545",
        help = "The address and port to listen on for incoming connections"
    )]
    listen_addr: SocketAddr,

    #[arg(long, env, help = "WebSocket URI of the upstream server to connect to")]
    upstream_ws: Uri,

    #[arg(
        long,
        env,
        default_value = "20",
        help = "Number of messages to buffer for lagging clients"
    )]
    message_buffer_size: usize,

    #[arg(
        long,
        env,
        default_value = "100",
        help = "Maximum number of concurrently connected clients"
    )]
    maximum_concurrent_connections: usize,

    #[arg(long, env, default_value = "info")]
    log_level: Level,

    /// Format for logs, can be json or text
    #[arg(long, env, default_value = "text")]
    log_format: String,

    // Enable Prometheus metrics
    #[arg(long, env, default_value = "true")]
    metrics: bool,

    /// Address to run the metrics server on
    #[arg(long, env, default_value = "0.0.0.0:9000")]
    metrics_addr: SocketAddr,

    /// Maximum backoff allowed for upstream connections
    #[arg(long, env, default_value = "20")]
    subscriber_max_interval: u64,
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    let args = Args::parse();

    let log_format = args.log_format.to_lowercase();
    let log_level = args.log_level.to_string();

    if log_format == "json" {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(EnvFilter::new(log_level))
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(log_level))
            .with_ansi(false)
            .init();
    }

    if args.metrics {
        info!(
            message = "starting metrics server",
            address = args.metrics_addr.to_string()
        );

        PrometheusBuilder::new()
            .with_http_listener(args.metrics_addr)
            .install()
            .expect("failed to setup Prometheus endpoint")
    }

    let metrics = Arc::new(Metrics::default());
    let metrics_clone = metrics.clone();

    let (send, _rec) = broadcast::channel(args.message_buffer_size);
    let sender = send.clone();

    let listener = move |data: String| {
        trace!(message = "received data", data = data);
        // Subtract one from receiver count, as we have to keep one receiver open at all times (see _rec)
        // to avoid the channel being closed. However this is not an active client connection.
        metrics_clone
            .active_connections
            .set((send.receiver_count() - 1) as f64);

        match send.send(data) {
            Ok(_) => (),
            Err(e) => error!(message = "failed to send data", error = e.to_string()),
        }
    };

    let token = CancellationToken::new();

    let mut subscriber = WebsocketSubscriber::new(
        args.upstream_ws,
        listener,
        args.subscriber_max_interval,
        metrics.clone(),
    );
    let subscriber_task = subscriber.run(token.clone());

    let registry = Registry::new(sender, args.maximum_concurrent_connections, metrics.clone());

    let server = Server::new(args.listen_addr, registry.clone());
    let server_task = server.listen(token.clone());

    let mut interrupt = signal(SignalKind::interrupt()).unwrap();
    let mut terminate = signal(SignalKind::terminate()).unwrap();

    tokio::select! {
        _ = subscriber_task => {
            info!("subscriber task terminated");
            token.cancel();
        },
        _ = server_task => {
            info!("server task terminated");
            token.cancel();
        }
        _ = interrupt.recv() => {
            info!("process interrupted, shutting down");
            token.cancel();
        }
        _ = terminate.recv() => {
            info!("process terminated, shutting down");
            token.cancel();
        }
    }
}
