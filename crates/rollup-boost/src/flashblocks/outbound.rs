use super::primitives::FlashblocksPayloadV1;
use futures::{SinkExt, StreamExt};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{Mutex, mpsc},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{debug, error, info};

type WebSocketSink =
    futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Message>;

pub struct FlashblocksOutboundService {
    clients: Arc<Mutex<Vec<WebSocketSink>>>,
    receiver: mpsc::Receiver<FlashblocksPayloadV1>,
}

impl FlashblocksOutboundService {
    pub fn new(receiver: mpsc::Receiver<FlashblocksPayloadV1>) -> Self {
        Self {
            clients: Arc::new(Mutex::new(Vec::new())),
            receiver,
        }
    }

    pub async fn run(mut self, addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(&addr).await?;
        info!("Outbound WebSocket server listening on: {}", addr);

        let clients = self.clients.clone();

        // Spawn a task to handle new WebSocket connections
        tokio::spawn(async move {
            while let Ok((stream, addr)) = listener.accept().await {
                debug!("New WebSocket connection from: {}", addr);
                let clients = clients.clone();

                tokio::spawn(async move {
                    match accept_async(stream).await {
                        Ok(ws_stream) => {
                            let (sink, _) = ws_stream.split();
                            clients.lock().await.push(sink);
                            debug!("Client added: {}", addr);
                        }
                        Err(e) => error!("Error accepting WebSocket connection: {}", e),
                    }
                });
            }
        });

        // Handle incoming messages from the channel and broadcast them
        while let Some(payload) = self.receiver.recv().await {
            let message = match serde_json::to_string(&payload) {
                Ok(msg) => Message::Text(msg.into()),
                Err(e) => {
                    error!("Failed to serialize payload: {}", e);
                    continue;
                }
            };

            let mut clients = self.clients.lock().await;
            clients.retain_mut(|sink| {
                let send_future = sink.send(message.clone());
                match futures::executor::block_on(send_future) {
                    Ok(_) => true,
                    Err(e) => {
                        error!("Failed to send message to client: {}", e);
                        false // Remove failed client
                    }
                }
            });
        }

        Ok(())
    }
}
