use std::time::Duration;

use crate::payload::PayloadSource;
use crate::{secret_to_bearer_header, AuthLayer};
use alloy_json_rpc::{RpcError, RpcRecv, RpcSend};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::JwtSecret;
use alloy_transport::TransportResult;
use alloy_transport_http::{Http, HyperClient};
use http::header::AUTHORIZATION;
use http::{HeaderMap, Uri};
use http_body_util::Full;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use opentelemetry::trace::SpanKind;
use tower::ServiceBuilder;
use tower::timeout::TimeoutLayer;
use tower_http::decompression::DecompressionLayer;
use tracing::{debug, error, instrument};

#[derive(Clone, Debug)]
pub struct RpcProxyClient {
    client: RpcClient,
    url: Uri,
    target: PayloadSource,
}

impl RpcProxyClient {
    pub fn new(url: Uri, secret: JwtSecret, target: PayloadSource, timeout: u64) -> Self {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .expect("no native root CA certificates found")
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();

        let client = Client::builder(TokioExecutor::new()).build::<_, Full<hyper::body::Bytes>>(connector);

        let service = ServiceBuilder::new()
            // .layer(TimeoutLayer::new(Duration::from_millis(timeout)))
            // .layer(DecompressionLayer::new())
            .layer(AuthLayer::new(secret))
            .service(client);

        let layer_transport = HyperClient::with_service(service);
        let http = Http::with_client(layer_transport, url.to_string().parse().unwrap());

        let client = RpcClient::new(http, true);

        Self {
            client,
            url,
            target,
        }
    }

    /// Forwards a JSON-RPC request to the endpoint
    #[instrument(
        skip(self),
        fields(
            otel.kind = ?SpanKind::Client,
            url = %self.url,
            method,
            code,
        ),
        err(Debug)
    )]
    pub async fn request<Params: RpcSend, Resp: RpcRecv>(
        &self,
        method: &str,
        params: Params,
    ) -> TransportResult<Resp> {
        debug!("forwarding {} to {}", method, self.target);
        tracing::Span::current().record("method", method);
        let resp = self
            .client
            .request::<Params, Resp>(method.to_string(), params)
            .await
            .inspect_err(|err| {
                error!(
                    %err,
                    "HTTP request to proxy failed"
                );
                if let RpcError::ErrorResp(err) = err {
                    tracing::Span::current().record("code", err.code);
                }
            })?;
        Ok(resp)
    }
}
