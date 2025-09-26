use std::time::Duration;

use crate::AuthLayer;
use crate::client::error::restore_error;
use crate::payload::PayloadSource;
use alloy_json_rpc::{RpcError, RpcRecv, RpcSend};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::JwtSecret;
use alloy_transport::TransportResult;
use alloy_transport_http::{Http, HyperClient};
use http::Uri;
use http_body_util::Full;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use opentelemetry::trace::SpanKind;
use tower::util::MapErrLayer;
use tower::{BoxError, ServiceBuilder};
use tracing::{debug, instrument};

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

        let client =
            Client::builder(TokioExecutor::new()).build::<_, Full<hyper::body::Bytes>>(connector);

        let service = ServiceBuilder::new()
            // This layer formats error, because timeout layer erases error types and we need it for alloy
            .layer(MapErrLayer::new(|e: BoxError| restore_error(e)))
            // TODO: Add DecompressionLayer
            .layer(AuthLayer::new(secret))
            .timeout(Duration::from_secs(timeout))
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
        err(Display)
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
                if let RpcError::ErrorResp(err) = err {
                    tracing::Span::current().record("code", err.code);
                }
            })?;
        Ok(resp)
    }
}
