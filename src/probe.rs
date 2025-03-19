use std::{
    pin::Pin,
    sync::{Arc, atomic::AtomicBool},
    task::{Context, Poll},
};

use futures::FutureExt as _;
use jsonrpsee::{
    core::BoxError,
    http_client::{HttpBody, HttpRequest, HttpResponse},
};
use tower::{Layer, Service};

#[derive(Debug, Default)]
pub struct Probes {
    pub health: AtomicBool,
    pub ready: AtomicBool,
    pub live: AtomicBool,
}

impl Probes {
    pub fn set_health(&self, value: bool) {
        self.health
            .store(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_ready(&self, value: bool) {
        self.ready
            .store(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_live(&self, value: bool) {
        self.live.store(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn health(&self) -> bool {
        self.health.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn live(&self) -> bool {
        self.live.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// A [`Layer`] that filters out /healthz requests and responds with a 200 OK.
#[derive(Clone, Debug)]
pub struct ProbeLayer {
    probes: Arc<Probes>,
}

impl ProbeLayer {
    pub(crate) fn new() -> (Self, Arc<Probes>) {
        let probes = Arc::new(Probes::default());
        (
            Self {
                probes: probes.clone(),
            },
            probes,
        )
    }
}

impl<S> Layer<S> for ProbeLayer {
    type Service = ProbeService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ProbeService {
            inner,
            probes: self.probes.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProbeService<S> {
    inner: S,
    probes: Arc<Probes>,
}

impl<S> Service<HttpRequest<HttpBody>> for ProbeService<S>
where
    S: Service<HttpRequest<HttpBody>, Response = HttpResponse> + Send + Sync + Clone + 'static,
    S::Response: 'static,
    S::Error: Into<BoxError> + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, request: HttpRequest<HttpBody>) -> Self::Future {
        // See https://github.com/tower-rs/tower/blob/abb375d08cf0ba34c1fe76f66f1aba3dc4341013/tower-service/src/lib.rs#L276
        // for an explanation of this pattern
        let mut service = self.clone();
        service.inner = std::mem::replace(&mut self.inner, service.inner);

        async move {
            match request.uri().path() {
                "/healthz" => {
                    if service.probes.health() {
                        ok()
                    } else {
                        internal_server_error()
                    }
                }
                "/readyz" => {
                    if service.probes.ready() {
                        ok()
                    } else {
                        service_unavailable()
                    }
                }
                "/livez" => {
                    if service.probes.live() {
                        ok()
                    } else {
                        service_unavailable()
                    }
                }
                _ => service.inner.call(request).await.map_err(|e| e.into()),
            }
        }
        .boxed()
    }
}

fn ok() -> Result<HttpResponse<HttpBody>, BoxError> {
    Ok(HttpResponse::builder()
        .status(200)
        .body(HttpBody::from("OK"))
        .unwrap())
}

fn internal_server_error() -> Result<HttpResponse<HttpBody>, BoxError> {
    Ok(HttpResponse::builder()
        .status(500)
        .body(HttpBody::from("Internal Server Error"))
        .unwrap())
}

fn service_unavailable() -> Result<HttpResponse<HttpBody>, BoxError> {
    Ok(HttpResponse::builder()
        .status(503)
        .body(HttpBody::from("Service Unavailable"))
        .unwrap())
}
