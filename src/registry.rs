use crate::metrics::Metrics;
use axum::extract::ws::WebSocket;
use axum::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Sender;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::Error::ConnectionClosed;
use tracing::{error, info, trace, warn};

pub struct ClientConnection {
    pub ip_addr: SocketAddr,
    _permit: OwnedSemaphorePermit,
    websocket: Option<WebSocket>,
}

impl ClientConnection {
    pub fn new(ip_addr: SocketAddr, permit: OwnedSemaphorePermit) -> Self {
        Self {
            ip_addr,
            _permit: permit,
            websocket: None,
        }
    }

    pub fn with_websocket(&mut self, ws: WebSocket) {
        self.websocket = Some(ws);
    }

    pub async fn send(&mut self, data: String) -> Result<(), Error> {
        match self.websocket {
            Some(ref mut wss) => wss.send(data.into_bytes().into()).await,
            None => {
                error!(
                    message = "websocket connection was not opened",
                    client = self.id()
                );
                Err(Error::new(ConnectionClosed))
            }
        }
    }

    pub fn id(&self) -> String {
        self.ip_addr.to_string()
    }
}

#[derive(Clone)]
pub struct Registry {
    sender: Sender<String>,
    semaphore: Arc<Semaphore>,
    metrics: Arc<Metrics>,
}

impl Registry {
    pub fn new(
        sender: Sender<String>,
        max_concurrent_connections: usize,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            sender,
            semaphore: Arc::new(Semaphore::new(max_concurrent_connections)),
            metrics,
        }
    }

    pub fn try_register(&self, ip_addr: SocketAddr) -> Result<ClientConnection, ()> {
        match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => Ok(ClientConnection::new(ip_addr, permit)),
            Err(_) => {
                self.metrics.rate_limited_requests.increment(1);
                Err(())
            }
        }
    }

    pub async fn subscribe(&self, mut client: ClientConnection) {
        info!(message = "subscribing client", client = client.id());

        let mut receiver = self.sender.subscribe();
        let metrics = self.metrics.clone();
        metrics.new_connections.increment(1);

        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(msg) => match client.send(msg.clone()).await {
                        Ok(_) => {
                            trace!(message = "message sent to client", client = client.id());
                            metrics.sent_messages.increment(1);
                        }
                        Err(e) => {
                            warn!(
                                message = "failed to send data to client",
                                client = client.id(),
                                error = e.to_string()
                            );
                            metrics.failed_messages.increment(1);
                            break;
                        }
                    },
                    Err(RecvError::Closed) => {
                        info!(message = "upstream connection closed", client = client.id());
                        break;
                    }
                    Err(RecvError::Lagged(_)) => {
                        info!(message = "client is lagging", client = client.id());
                        metrics.lag_events.increment(1);
                        receiver = receiver.resubscribe();
                    }
                }
            }

            metrics.closed_connections.increment(1);
            info!(message = "client disconnected", client = client.id());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_max_concurrent_connections() {
        let (tx, _rx) = tokio::sync::broadcast::channel(1);
        let registry = Registry::new(tx, 3, Arc::new(Metrics::default()));

        assert_eq!(registry.semaphore.available_permits(), 3);

        let c1 = registry
            .try_register(SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap();
        let _c2 = registry
            .try_register(SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap();
        let _c3 = registry
            .try_register(SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap();

        assert_eq!(registry.semaphore.available_permits(), 0);

        let c4 = registry.try_register(SocketAddr::from(([127, 0, 0, 1], 0)));
        assert!(c4.is_err());

        drop(c1);
        assert_eq!(registry.semaphore.available_permits(), 1);

        let c4 = registry.try_register(SocketAddr::from(([127, 0, 0, 1], 0)));
        assert!(c4.is_ok());
        let c4 = c4.unwrap();

        assert_eq!(registry.semaphore.available_permits(), 0);
        let _c5 = c4;
        assert_eq!(registry.semaphore.available_permits(), 0);
    }
}
