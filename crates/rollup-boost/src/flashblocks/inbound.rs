use super::{metrics::FlashblocksWsInboundMetrics, primitives::FlashblocksPayloadV1};
use crate::FlashblocksWebsocketConfig;
use backoff::backoff::Backoff;
use bytes::Bytes;
use dashmap::DashSet;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use backoff::ExponentialBackoff;
use tokio::{sync::mpsc, time::interval};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use url::Url;

#[derive(Debug, thiserror::Error)]
enum FlashblocksReceiverError {
    #[error("WebSocket connection failed: {0}")]
    Connection(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("Ping failed")]
    PingFailed,

    #[error("Pong timeout")]
    PongTimeout,

    #[error("Websocket haven't return the message")]
    MessageMissing,

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Task panicked: {0}")]
    TaskPanic(String),

    #[error("Failed to send message to sender: {0}")]
    SendError(#[from] Box<tokio::sync::mpsc::error::SendError<FlashblocksPayloadV1>>),
}

pub struct FlashblocksReceiverService {
    url: Url,
    sender: mpsc::Sender<FlashblocksPayloadV1>,
    websocket_config: FlashblocksWebsocketConfig,
    metrics: FlashblocksWsInboundMetrics,
}

impl FlashblocksReceiverService {
    pub fn new(
        url: Url,
        sender: mpsc::Sender<FlashblocksPayloadV1>,
        websocket_config: FlashblocksWebsocketConfig,
    ) -> Self {
        Self {
            url,
            sender,
            websocket_config,
            metrics: Default::default(),
        }
    }

    pub async fn run(self) {
        let mut backoff = self.websocket_config.backoff();
        loop {
            if let Err(e) = self.connect_and_handle(&mut backoff).await {
                let interval = backoff
                    .next_backoff()
                    .expect("max_elapsed_time not set, never None");
                error!(
                    "Flashblocks receiver connection error, retrying in {}ms: {}",
                    interval.as_millis(),
                    e
                );
                self.metrics.reconnect_attempts.increment(1);
                self.metrics.connection_status.set(0);
                tokio::time::sleep(interval).await;
            } else {
                break;
            }
        }
    }

    async fn connect_and_handle(&self, backoff: &mut ExponentialBackoff) -> Result<(), FlashblocksReceiverError> {
        let (ws_stream, _) = connect_async(self.url.as_str()).await?;
        let (mut write, mut read) = ws_stream.split();

        // if we have successfully connected - reset backoff
        backoff.reset();

        info!("Connected to Flashblocks receiver at {}", self.url);
        self.metrics.connection_status.set(1);

        let cancel_token = CancellationToken::new();
        let cancel_for_ping = cancel_token.clone();

        let ping_map = Arc::new(DashSet::with_capacity(10));
        let pong_map = ping_map.clone();

        let mut ping_interval = interval(self.websocket_config.ping_interval());
        let ping_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = ping_interval.tick() => {
                        let uuid = uuid::Uuid::now_v7();
                        if write.send(Message::Ping(Bytes::copy_from_slice(uuid.as_bytes().as_slice()))).await.is_err() {
                            return Err(FlashblocksReceiverError::PingFailed);
                        }
                        ping_map.insert(uuid);
                    }
                    _ = cancel_for_ping.cancelled() => {
                        tracing::debug!("Ping task cancelled");
                        return Ok(());
                    }
                }
            }
        });

        let sender = self.sender.clone();
        let metrics = self.metrics.clone();

        let pong_timeout = self.websocket_config.pong_interval();
        let message_handle = tokio::spawn(async move {
            let mut pong_interval = interval(pong_timeout);
            // We await here because first tick executes immediately
            pong_interval.tick().await;
            loop {
                tokio::select! {
                    result = read.next() => {
                        match result {
                            Some(Ok(msg)) => match msg {
                                Message::Text(text) => {
                                    metrics.messages_received.increment(1);
                                    if let Ok(flashblocks_msg) =
                                        serde_json::from_str::<FlashblocksPayloadV1>(&text)
                                    {
                                        sender.send(flashblocks_msg).await.map_err(|e| {
                                            FlashblocksReceiverError::SendError(Box::new(e))
                                        })?;
                                    }
                                }
                                Message::Close(_) => {
                                    return Err(FlashblocksReceiverError::ConnectionClosed);
                                }
                                Message::Pong(data) => {
                                    match uuid::Uuid::from_slice(data.as_ref()) {
                                        Ok(uuid) => {
                                            if pong_map.remove(&uuid).is_some() {
                                                pong_interval.reset();
                                            } else {
                                                tracing::warn!("Received pong with unknown data {}", uuid);
                                            }

                                        }
                                        Err(e) => {
                                            tracing::warn!("Failed to parse pong: {e}");
                                        }
                                    }

                                }
                                msg => {
                                    tracing::warn!("Received unexpected message: {:?}", msg);
                                }
                            },
                            Some(Err(e)) => {
                                return Err(FlashblocksReceiverError::ConnectionError(e.to_string()));
                            }
                            None => {
                                return Err(FlashblocksReceiverError::MessageMissing);
                            }
                        }
                    },
                    _ = pong_interval.tick() => {
                        return Err(FlashblocksReceiverError::PongTimeout);
                    }
                };
            }
        });

        let result = tokio::select! {
            result = message_handle => {
                result.map_err(|e| FlashblocksReceiverError::TaskPanic(e.to_string()))?
            },
            result = ping_task => {
                result.map_err(|e| FlashblocksReceiverError::TaskPanic(e.to_string()))?
            },
        };

        cancel_token.cancel();
        result
    }
}

#[cfg(test)]
mod tests {
    use futures::SinkExt;
    use tokio::sync::watch;
    use tokio_tungstenite::{accept_async, tungstenite::Utf8Bytes};

    use super::*;
    use std::net::{SocketAddr, TcpListener};

    async fn start(
        addr: SocketAddr,
    ) -> eyre::Result<(
        watch::Sender<bool>,
        mpsc::Sender<FlashblocksPayloadV1>,
        mpsc::Receiver<()>,
        url::Url,
    )> {
        let (term_tx, mut term_rx) = watch::channel(false);
        let (send_tx, mut send_rx) = mpsc::channel::<FlashblocksPayloadV1>(100);
        let (send_ping_tx, send_ping_rx) = mpsc::channel::<()>(100);

        let listener = TcpListener::bind(addr)?;
        let url = Url::parse(&format!("ws://{addr}"))?;

        listener
            .set_nonblocking(true)
            .expect("Failed to set TcpListener socket to non-blocking");

        let listener = tokio::net::TcpListener::from_std(listener)
            .expect("Failed to convert TcpListener to tokio TcpListener");

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = term_rx.changed() => {
                        if *term_rx.borrow() {
                            return;
                        }
                    }

                    result = listener.accept() => {
                        match result {
                            Ok((connection, _addr)) => {
                                match accept_async(connection).await {
                                    Ok(ws_stream) => {
                                        let (mut write, mut read) = ws_stream.split();

                                        loop {
                                            tokio::select! {
                                                Some(msg) = send_rx.recv() => {
                                                    let serialized = serde_json::to_string(&msg).unwrap();
                                                    let utf8_bytes = Utf8Bytes::from(serialized);

                                                    write.send(Message::Text(utf8_bytes)).await.unwrap();
                                                },
                                                msg = read.next() => {
                                                    match msg {
                                                        // we need to read for the library to handle pong messages
                                                        Some(Ok(Message::Ping(_))) => {
                                                            send_ping_tx.send(()).await.unwrap();
                                                        },
                                                        _ => {}
                                                    }
                                                }
                                                _ = term_rx.changed() => {
                                                    if *term_rx.borrow() {
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to accept WebSocket connection: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                // Optionally break or continue based on error type
                                if e.kind() == std::io::ErrorKind::Interrupted {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok((term_tx, send_tx, send_ping_rx, url))
    }

    #[tokio::test]
    async fn test_flashblocks_receiver_service() -> eyre::Result<()> {
        let addr = "127.0.0.1:8080".parse::<SocketAddr>().unwrap();
        let (term, send_msg, _, url) = start(addr).await?;

        let (tx, mut rx) = mpsc::channel(100);

        let config = FlashblocksWebsocketConfig {
            flashblock_builder_ws_initial_reconnect_ms: 100,
            flashblock_builder_ws_max_reconnect_ms: 100,
            flashblock_builder_ws_ping_interval_ms: 500,
            flashblock_builder_ws_pong_timeout_ms: 2000,
        };
        let service = FlashblocksReceiverService::new(url, tx, config);
        let _ = tokio::spawn(async move {
            service.run().await;
        });

        // Send a message to the websocket server
        send_msg
            .send(FlashblocksPayloadV1::default())
            .await
            .expect("Failed to send message");

        let msg = rx.recv().await.expect("Failed to receive message");
        assert_eq!(msg, FlashblocksPayloadV1::default());

        // Drop the websocket server and start another one with the same address
        // The FlashblocksReceiverService should reconnect to the new server
        term.send(true).unwrap();

        // sleep for 1 second to ensure the server is dropped
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // start a new server with the same address
        let (term, send_msg, _, _url) = start(addr).await?;
        send_msg
            .send(FlashblocksPayloadV1::default())
            .await
            .expect("Failed to send message");

        let msg = rx.recv().await.expect("Failed to receive message");
        assert_eq!(msg, FlashblocksPayloadV1::default());
        term.send(true).unwrap();

        Ok(())
    }

    #[tokio::test]
    async fn test_flashblocks_receiver_service_ping_pong() -> eyre::Result<()> {
        // test that if the builder is not sending any messages back, the service will send
        // ping messages to test the connection periodically

        let addr = "127.0.0.1:8081".parse::<SocketAddr>().unwrap();
        let (_term, _send_msg, mut ping_rx, url) = start(addr).await?;
        let config = FlashblocksWebsocketConfig {
            flashblock_builder_ws_initial_reconnect_ms: 100,
            flashblock_builder_ws_max_reconnect_ms: 100,
            flashblock_builder_ws_ping_interval_ms: 500,
            flashblock_builder_ws_pong_timeout_ms: 2000,
        };

        let (tx, _rx) = mpsc::channel(100);
        let service = FlashblocksReceiverService::new(url, tx, config);
        let _ = tokio::spawn(async move {
            service.run().await;
        });

        // even if we do not send any messages, we should receive pings to keep the connection alive
        for _ in 0..10 {
            ping_rx.recv().await.expect("Failed to receive ping");
        }

        Ok(())
    }
}
