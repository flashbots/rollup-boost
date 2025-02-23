use crate::registry::{ClientConnection, Registry};
use axum::body::Body;
use axum::extract::ws::WebSocket;
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Clone)]
struct ServerState {
    registry: Registry,
}

#[derive(Clone)]
pub struct Server {
    listen_addr: SocketAddr,
    registry: Registry,
}

impl Server {
    pub fn new(listen_addr: SocketAddr, registry: Registry) -> Self {
        Self {
            listen_addr,
            registry,
        }
    }

    pub async fn listen(&self, cancellation_token: CancellationToken) {
        let router = Router::new()
            .route("/healthz", get(healthz_handler))
            .route("/ws", any(websocket_handler))
            .with_state(ServerState {
                registry: self.registry.clone(),
            });

        let listener = tokio::net::TcpListener::bind(self.listen_addr)
            .await
            .unwrap();

        info!(
            message = "starting server",
            address = listener.local_addr().unwrap().to_string()
        );

        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(cancellation_token.cancelled_owned())
        .await
        .unwrap()
    }
}

async fn healthz_handler() -> impl IntoResponse {
    StatusCode::OK
}

async fn websocket_handler(
    State(state): State<ServerState>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    match state.registry.try_register(addr) {
        Ok(client) => ws
            .on_failed_upgrade(move |_e| {
                info!(
                    message = "failed to upgrade connection",
                    client = addr.to_string()
                )
            })
            .on_upgrade(move |socket| handle_socket(socket, client, state)),
        Err(_) => Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .body(Body::empty())
            .unwrap(),
    }
}

async fn handle_socket(ws: WebSocket, mut client: ClientConnection, state: ServerState) {
    client.with_websocket(ws);
    state.registry.subscribe(client).await;
}
