#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use std::net::SocketAddr;

#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use eyre::Result;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use metrics::gauge;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use metrics_exporter_prometheus::PrometheusBuilder;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use metrics_util::layers::{PrefixLayer, Stack};
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use tokio::net::TcpListener;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use tracing::{error, info};

#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use http::StatusCode;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use hyper::service::service_fn;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use hyper::{Request, Response, server::conn::http1};
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use hyper_util::rt::TokioIo;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use jsonrpsee::http_client::HttpBody;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use metrics_exporter_prometheus::PrometheusHandle;

#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use crate::ExecutionMode;
#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
use crate::cli::RollupBoostArgs;

#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
pub fn init_metrics(args: &RollupBoostArgs) -> Result<()> {
    if args.metrics {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        Stack::new(recorder)
            .push(PrefixLayer::new("rollup-boost"))
            .install()?;

        // Start the metrics server
        let metrics_addr = format!("{}:{}", args.metrics_host, args.metrics_port);
        let addr: SocketAddr = metrics_addr.parse()?;
        tokio::spawn(init_metrics_server(addr, handle)); // Run the metrics server in a separate task
    }
    Ok(())
}

#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
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
                                .expect("Failed to create metrics response"),
                            _ => Response::builder()
                                .status(StatusCode::NOT_FOUND)
                                .body(HttpBody::empty())
                                .expect("Failed to create not found response"),
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

#[cfg(all(feature = "server", feature = "metrics-exporter-prometheus"))]
/// Update the execution_mode prometheus metric
pub fn update_execution_mode_gauge(execution_mode: ExecutionMode) {
    let gauge = gauge!("rollup_boost_execution_mode");
    gauge.set(execution_mode.to_metric_value());
}
