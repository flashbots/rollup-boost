use std::future;

use crate::client::auth::AuthLayer;
use crate::server::PayloadSource;
use alloy_rpc_types_engine::JwtSecret;
use http::Uri;
use http_body_util::BodyExt;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use jsonrpsee::core::BoxError;
use jsonrpsee::http_client::HttpBody;
use opentelemetry::trace::SpanKind;
use tower::{
    Service as _, ServiceBuilder, ServiceExt,
    retry::{Retry, RetryLayer},
};
use tower_http::decompression::{Decompression, DecompressionLayer};
use tracing::{debug, error, instrument};

use super::auth::Auth;

pub type HttpClientService =
    Retry<Attempts, Decompression<Auth<Client<HttpsConnector<HttpConnector>, HttpBody>>>>;

#[derive(Clone, Debug)]
pub struct HttpClient {
    client: HttpClientService,
    url: Uri,
    target: PayloadSource,
}

impl HttpClient {
    pub fn new(url: Uri, secret: JwtSecret, target: PayloadSource) -> Self {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .expect("no native root CA certificates found")
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();

        let client = Client::builder(TokioExecutor::new()).build(connector);

        let client = ServiceBuilder::new()
            .layer(RetryLayer::new(Attempts(5)))
            .layer(DecompressionLayer::new())
            .layer(AuthLayer::new(secret))
            .service(client);

        Self {
            client,
            url,
            target,
        }
    }

    /// Forwards an HTTP request to the `authrpc`, attaching the provided JWT authorization.
    #[instrument(
        skip(self, req),
        fields(otel.kind = ?SpanKind::Client,
        url = %self.url,
        method,
        code,
        ),
        err(Debug)
    )]
    pub async fn forward(
        &mut self,
        mut req: http::Request<HttpBody>,
        method: String,
    ) -> Result<http::Response<HttpBody>, BoxError> {
        debug!("forwarding {} to {}", method, self.target);
        tracing::Span::current().record("method", method);
        *req.uri_mut() = self.url.clone();

        let res = self.client.ready().await?.call(req).await?;

        let (parts, body) = res.into_parts();
        let body_bytes = body.collect().await?.to_bytes().to_vec();

        if let Some(code) = parse_response_code(&body_bytes)? {
            error!(%code, "error in forwarded response");
            tracing::Span::current().record("code", code);
        }

        Ok(http::Response::from_parts(
            parts,
            HttpBody::from(body_bytes),
        ))
    }
}

use tower::retry::Policy;

type Req = String;
type Res = String;

#[derive(Clone, Debug)]
struct Attempts(usize);

impl<E> Policy<Req, Res, E> for Attempts {
    type Future = future::Ready<Self>;

    fn retry(&self, req: &Req, result: Result<&Res, &E>) -> Option<Self::Future> {
        match result {
            Ok(_) => {
                // Treat all `Response`s as success,
                // so don't retry...
                None
            }
            Err(_) => {
                // Treat all errors as failures...
                // But we limit the number of attempts...
                if self.0 > 0 {
                    // Try again!
                    Some(future::ready(Attempts(self.0 - 1)))
                } else {
                    // Used all our attempts, no retry...
                    None
                }
            }
        }
    }

    fn clone_request(&self, req: &Req) -> Option<Req> {
        Some(req.clone())
    }
}

fn parse_response_code(body_bytes: &[u8]) -> eyre::Result<Option<i32>> {
    #[derive(serde::Deserialize, Debug)]
    struct RpcResponse {
        error: Option<JsonRpcError>,
    }

    #[derive(serde::Deserialize, Debug)]
    struct JsonRpcError {
        code: i32,
    }

    let res = serde_json::from_slice::<RpcResponse>(body_bytes)?;

    Ok(res.error.map(|e| e.code))
}
