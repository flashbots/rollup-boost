use std::time::Duration;

use super::primitives::FlashblocksPayloadV1;
use futures::{SinkExt, StreamExt};
use tokio::{
    sync::mpsc,
    time::{interval, timeout},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info};
use url::Url;

pub struct FlashblocksReceiverService {
    url: Url,
    sender: mpsc::Sender<FlashblocksPayloadV1>,
    reconnect_ms: u64,
}

impl FlashblocksReceiverService {
    pub fn new(url: Url, sender: mpsc::Sender<FlashblocksPayloadV1>, reconnect_ms: u64) -> Self {
        Self {
            url,
            sender,
            reconnect_ms,
        }
    }

    pub async fn run(self) {
        loop {
            if let Err(e) = self.connect_and_handle().await {
                error!("Flashblocks receiver connection error, retrying in 5 seconds: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(self.reconnect_ms)).await;
            } else {
                break;
            }
        }
    }

    async fn connect_and_handle(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _) = connect_async(self.url.as_str()).await?;
        let (mut write, mut read) = ws_stream.split();

        info!("Connected to Flashblocks receiver at {}", self.url);

        // Channel for coordinating ping/pong and activity tracking
        let (activity_tx, mut activity_rx) = mpsc::channel::<()>(10);
        let (pong_tx, mut pong_rx) = mpsc::channel::<Vec<u8>>(10);

        let ping_handle = tokio::spawn(async move {
            let mut last_activity = std::time::Instant::now();
            let mut check_interval = interval(Duration::from_millis(1000));
            let ping_interval = 1000;
            let pong_timeout = 2000;

            loop {
                tokio::select! {
                    // Reset activity timestamp when we receive any activity signal
                    _ = activity_rx.recv() => {
                        last_activity = std::time::Instant::now();
                    }

                    // Check if we need to send a ping
                    _ = check_interval.tick() => {
                        let idle_duration = last_activity.elapsed();

                        if idle_duration >= Duration::from_millis(ping_interval) {
                            if write.send(Message::Ping(Default::default())).await.is_err() {
                                break;
                            }

                            match timeout(Duration::from_millis(pong_timeout), pong_rx.recv()).await {
                                Ok(Some(_pong_data)) => {
                                    // if pong_data != b"pong" {
                                    last_activity = std::time::Instant::now();
                                    //}
                                }
                                Ok(None) => {
                                    return Err("Pong channel closed");
                                }
                                _ => {
                                    return Err("Pong timeout");
                                }
                            };
                        }
                    }
                }
            }

            Ok(())
        });

        let sender = self.sender.clone();
        let message_handle = tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                // Signal activity - we received a real message
                let _ = activity_tx.send(()).await;

                match msg.unwrap() {
                    // TODO: handle errors
                    Message::Text(text) => {
                        if let Ok(flashblocks_msg) =
                            serde_json::from_str::<FlashblocksPayloadV1>(&text)
                        {
                            sender.send(flashblocks_msg).await.unwrap(); // TODO
                        }
                    }
                    Message::Pong(data) => {
                        println!("Received pong");
                        if pong_tx.send(data.to_vec()).await.is_err() {
                            tracing::warn!("Failed to forward pong to ping handler");
                        }
                    }
                    Message::Close(_) => {
                        return Err("Closed");
                    }
                    _ => {}
                }
            }

            Ok(())
        });

        // Wait for either ping task to fail (connection dead) or message task to complete
        tokio::select! {
            ping_result = ping_handle => {
                error!("Ping task failed: {:?}", ping_result);
                panic!("Ping task failed");
            }
            message_result = message_handle => {
                match message_result {
                    Ok(Ok(())) => {
                        info!("Message handling completed normally");
                    }
                    Ok(Err(e)) => {
                        error!("Message handling error: {}", e);
                        panic!("Message handling error");
                    }
                    Err(e) => {
                        error!("Message task panicked: {}", e);
                        panic!("Message task panicked");
                    }
                }
            }
        }

        Ok(())
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
        url::Url,
    )> {
        let (term_tx, mut term_rx) = watch::channel(false);
        let (send_tx, mut send_rx) = mpsc::channel::<FlashblocksPayloadV1>(100);

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
                                    Ok(mut ws_stream) => {
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
                                                            println!("Received ping");
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

        Ok((term_tx, send_tx, url))
    }

    #[tokio::test]
    async fn test_flashblocks_receiver_service() -> eyre::Result<()> {
        let addr = "127.0.0.1:8080".parse::<SocketAddr>().unwrap();
        let (term, send_msg, url) = start(addr).await?;

        let (tx, mut rx) = mpsc::channel(100);

        let service = FlashblocksReceiverService::new(url, tx, 100);
        let _ = tokio::spawn(async move {
            service.run().await;
        });

        // Send a message to the websocket server
        let _ = send_msg
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
        let (term, send_msg, _url) = start(addr).await?;
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
        let (_term, _send_msg, url) = start(addr).await?;

        println!("Starting service");
        let (tx, mut rx) = mpsc::channel(100);

        let service = FlashblocksReceiverService::new(url, tx, 100);
        let _ = tokio::spawn(async move {
            service.run().await;
        });

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        Ok(())
    }
}
