use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::FutureExt as _;
use jsonrpsee::{
    core::BoxError,
    http_client::{HttpBody, HttpRequest, HttpResponse},
};
use tower::{Layer, Service, util::Either};

/// A [`Layer`] that filters out /healthz requests and responds with a 200 OK.
pub(crate) struct HealthLayer;

impl<S> Layer<S> for HealthLayer {
    type Service = HealthService<S>;

    fn layer(&self, service: S) -> Self::Service {
        HealthService(service)
    }
}

#[derive(Clone)]
pub struct HealthService<S>(S);

impl<S> Service<HttpRequest<HttpBody>> for HealthService<S>
where
    S: Service<HttpRequest<HttpBody>, Response = HttpResponse> + Send + Sync + Clone + 'static,
    S::Response: 'static,
    S::Error: Into<BoxError> + 'static,
    S::Future: Send + 'static,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = Either<
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>,
        S::Future,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, request: HttpRequest<HttpBody>) -> Self::Future {
        if request.uri().path() == "/healthz" {
            Either::A(Self::healthz().boxed())
        } else {
            Either::B(self.0.call(request))
        }
    }
}

impl<S> HealthService<S>
where
    S: Service<HttpRequest<HttpBody>, Response = HttpResponse> + Send + Sync + Clone + 'static,
    S::Response: 'static,
    S::Error: Into<BoxError> + 'static,
    S::Future: Send + 'static,
{
    async fn healthz() -> Result<HttpResponse, BoxError> {
        Ok(HttpResponse::new(HttpBody::from("OK")))
    }
}
