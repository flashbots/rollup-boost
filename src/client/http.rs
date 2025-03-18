use crate::client::auth::{AuthClientLayer, AuthClientService};
use crate::server::PayloadSource;
use alloy_rpc_types_engine::JwtSecret;
use eyre::Context;
use flate2::read::GzDecoder;
use http::response::Parts;
use http::{Request, Uri};
use http_body_util::BodyExt;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use jsonrpsee::core::BoxError;
use jsonrpsee::http_client::HttpBody;
use opentelemetry::trace::SpanKind;
use std::io::Read;
use tower::{Layer, Service};
use tracing::{debug, error, instrument};

#[derive(Clone, Debug)]
pub(crate) struct HttpClient {
    client: AuthClientService<Client<HttpsConnector<HttpConnector>, HttpBody>>,
    url: Uri,
    target: PayloadSource,
}

impl HttpClient {
    pub(crate) fn new(url: Uri, secret: JwtSecret, target: PayloadSource) -> Self {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .expect("no native root CA certificates found")
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        let auth = AuthClientLayer::new(secret);
        let client: Client<HttpsConnector<HttpConnector>, HttpBody> =
            Client::builder(TokioExecutor::new()).build(connector);
        let client = auth.layer(client);
        Self {
            client,
            url,
            target,
        }
    }

    #[instrument(
        skip(self, req),
        fields(otel.kind = ?SpanKind::Client),
        err
    )]
    pub(crate) async fn forward(
        &mut self,
        mut req: Request<HttpBody>,
        method: String,
    ) -> Result<http::Response<HttpBody>, BoxError> {
        *req.uri_mut() = self.url.clone();
        debug!("forwarding {} to {}", method, self.target);

        let res = self.client.call(req).await?;

        let (parts, body) = res.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| {
                error!(
                    target: "proxy::forward_request",
                    message = "error collecting body",
                    error = %e,
                );
                e
            })?
            .to_bytes()
            .to_vec();
        let parts_clone = parts.clone();
        let body_bytes_clone = body_bytes.clone();

        self.handle_response(parts_clone, body_bytes_clone)
            .context("error fowarding request")?;

        Ok(http::Response::from_parts(
            parts,
            HttpBody::from(body_bytes),
        ))
    }

    fn handle_response(&self, parts: Parts, body_bytes: Vec<u8>) -> eyre::Result<()> {
        // Check for GZIP compression
        let is_gzipped = parts
            .headers
            .get(http::header::CONTENT_ENCODING)
            .is_some_and(|val| val.as_bytes() == b"gzip");

        let decoded_body = if is_gzipped {
            // Decompress GZIP content
            let mut decoder = GzDecoder::new(&body_bytes[..]);
            let mut decoded = Vec::new();
            decoder
                .read_to_end(&mut decoded)
                .context("error decompressing body")?;
            decoded
        } else {
            body_bytes
        };

        debug!(
            target: "proxy::forward_request",
            message = "raw response body",
            body = %String::from_utf8_lossy(&decoded_body),
        );

        handle_response_code(&decoded_body)?;

        Ok(())
    }
}

fn handle_response_code(body_bytes: &[u8]) -> eyre::Result<()> {
    #[derive(serde::Deserialize, Debug)]
    struct RpcResponse {
        error: Option<JsonRpcError>,
    }

    #[derive(serde::Deserialize, Debug)]
    struct JsonRpcError {
        code: i32,
    }

    let res =
        serde_json::from_slice::<RpcResponse>(body_bytes).context("error deserializing body")?;

    if let Some(e) = res.error {
        return Err(eyre::eyre!("code: {}", e.code));
    }

    Ok(())
}
