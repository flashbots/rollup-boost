use std::time::Duration;

use super::{metrics::FlashblocksWsInboundMetrics, primitives::FlashblocksPayloadV1};
use futures::{SinkExt, StreamExt};
use tokio::{
    sync::mpsc,
    time::{Instant, interval},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, WebSocketStream};
use tokio_tungstenite::tungstenite::stream::MaybeTlsStream;
use tracing::{error, info};
use url::Url;

#[derive(Debug, thiserror::Error)]
enum FlashblocksReceiverError {
    #[error("WebSocket connection failed: {0}")]
    Connection(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("Connection timeout")]
    ConnectionTimeout,

    #[error("Ping failed")]
    PingFailed,

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
    reconnect_ms: u64,
    metrics: FlashblocksWsInboundMetrics,
}

impl FlashblocksReceiverService {
    pub fn new(url: Url, sender: mpsc::Sender<FlashblocksPayloadV1>, reconnect_ms: u64) -> Self {
        Self {
            url,
            sender,
            reconnect_ms,
            metrics: Default::default(),
        }
    }

    pub async fn run(self) {
        loop {
            if let Err(e) = self.connect_and_handle().await {
                error!("Flashblocks receiver connection error, retrying in 5 seconds: {e}");
                self.metrics.reconnect_attempts.increment(1);
                self.metrics.connection_status.set(0);
                tokio::time::sleep(std::time::Duration::from_millis(self.reconnect_ms)).await;
            } else {
                break;
            }
        }
    }

    async fn connect_and_handle(&self) -> Result<(), FlashblocksReceiverError> {
        let (ws_stream, _) = connect_async(self.url.as_str()).await?;
        let (mut write, mut read) = ws_stream.split();

        info!("Connected to Flashblocks receiver at {}", self.url);
        self.metrics.connection_status.set(1);
        
        let ping_task = tokio::spawn(async move {
            let mut ping_interval = interval(Duration::from_millis(500));
            loop {
                ping_interval.tick().await;
                if write.send(Message::Ping(Default::default())).await.is_err() {
                    return Err(FlashblocksReceiverError::PingFailed);
                }
            }
        });

        let sender = self.sender.clone();
        let metrics = self.metrics.clone();

        let message_handle = tokio::spawn(async move {
            let mut timeout = interval(Duration::from_millis(500));
            tokio::select! {
                _ = timeout.tick() => return Err(FlashblocksReceiverError::ConnectionClosed),
                Some(msg) = read.next() => {
                    // TODO: do we need this?
                    timeout.reset();
                    match msg? {
                        Message::Text(text) => {
                            self.metrics.messages_received.increment(1);
                            if let Ok(flashblocks_msg) = serde_json::from_str::<FlashblocksPayloadV1>(&text) {
                                // TODO: fix error type
                                self.sender.send(flashblocks_msg).await?;
                            }
                        },
                        Message::Close(_) => {
                            return Err(FlashblocksReceiverError::ConnectionClosed);
                        },
                        Message::Pong(_) => (),
                        _ => ()
                    }
                },
            }
            Ok(())
        });

        let result = tokio::select! {
            result = message_handle => {
                result.map_err(|e| FlashblocksReceiverError::TaskPanic(e.to_string()))?
            },
            result = ping_task => {
                result.map_err(|e| FlashblocksReceiverError::TaskPanic(e.to_string()))?
            },
        };

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
                                                        _ => {
                                                            println!("Received message: {:?}", msg);
                                                        }
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

        let service = FlashblocksReceiverService::new(url, tx, 100);
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
        let _ = send_msg
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

        let (tx, _rx) = mpsc::channel(100);
        let service = FlashblocksReceiverService::new(url, tx, 100);
        let _ = tokio::spawn(async move {
            service.run().await;
        });

        // even if we do not send any messages, we should receive pings to keep the connection alive
        for _ in 0..10 {
            let _ = ping_rx.recv().await.expect("Failed to receive ping");
        }

        Ok(())
    }
}
