use std::time::Duration;

use crate::payload::PayloadSource;
use crate::secret_to_bearer_header;
use alloy_json_rpc::{RpcError, RpcRecv, RpcSend};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::JwtSecret;
use alloy_transport::TransportResult;
use http::header::AUTHORIZATION;
use http::{HeaderMap, Uri};
use opentelemetry::trace::SpanKind;
use tracing::{debug, error, instrument};

#[derive(Clone, Debug)]
pub struct RpcProxyClient {
    client: RpcClient,
    url: Uri,
    target: PayloadSource,
}

impl RpcProxyClient {
    pub fn new(url: Uri, secret: JwtSecret, target: PayloadSource, timeout: u64) -> Self {
        let mut headers = HeaderMap::new();
        let mut auth_header = secret_to_bearer_header(&secret);
        auth_header.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_header);

        // Create http client that alloys is using inside
        let http_client = reqwest::ClientBuilder::default()
            .timeout(Duration::from_millis(timeout))
            .default_headers(headers)
            .deflate(true)
            .brotli(true)
            .zstd(true)
            .gzip(true)
            .build()
            .expect("rpc client creation");
        let client =
            RpcClient::builder().http_with_client(http_client, url.to_string().parse().unwrap());

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
