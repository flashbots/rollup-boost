use crate::client::auth::AuthLayer;
use crate::server::PayloadSource;
use alloy_rpc_types_engine::JwtSecret;
use futures::FutureExt;
use http::Uri;
use http_body_util::BodyExt;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use jsonrpsee::core::BoxError;
use jsonrpsee::http_client::HttpBody;
use opentelemetry::trace::SpanKind;
use std::pin::Pin;
use std::time::Duration;
use tower::retry::Policy;
use tower::{
    Service as _, ServiceBuilder, ServiceExt,
    retry::{Retry, RetryLayer},
};
use tower_http::decompression::{Decompression, DecompressionLayer};
use tracing::{debug, error, instrument};

use super::auth::Auth;

pub type HttpClientService =
    Retry<Delay, Decompression<Auth<Client<HttpsConnector<HttpConnector>, HttpBody>>>>;

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
            .layer(RetryLayer::new(Delay {
                delay: Duration::from_millis(200),
                attempts: 10,
            }))
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

#[derive(Clone, Debug)]
pub struct Delay {
    delay: Duration,
    attempts: u64,
}

impl<Req, Res, E> Policy<Req, Res, E> for Delay {
    type Future = Pin<Box<dyn Future<Output = Self> + Send + 'static>>;

    fn retry(&self, _req: &Req, res: Result<&Res, &E>) -> Option<Self::Future> {
        match res {
            Ok(_) => None,
            Err(_) => {
                if self.attempts > 0 {
                    let clone = self.clone();
                    Some(
                        async move {
                            tokio::time::sleep(clone.delay).await;
                            Delay {
                                delay: clone.delay,
                                attempts: clone.attempts - 1,
                            }
                        }
                        .boxed(),
                    )
                } else {
                    None
                }
            }
        }
    }

    fn clone_request(&self, _req: &Req) -> Option<Req> {
        None
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
