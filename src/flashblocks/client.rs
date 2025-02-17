use futures_util::StreamExt;
use op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV3;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info};
use url::Url;

#[derive(Debug, Deserialize, Serialize)]
struct FlashbotsMessage {
    method: String,
    params: serde_json::Value,
    #[serde(default)]
    id: Option<u64>,
}

// Simplify actor messages to just handle shutdown
#[derive(Debug)]
enum ActorMessage {
    BestPayload {
        response: OpExecutionPayloadEnvelopeV3,
    },
}

pub struct FlashbotsClient {
    sender: mpsc::Sender<ActorMessage>,
    best_payload: Arc<RwLock<Option<OpExecutionPayloadEnvelopeV3>>>,
    mailbox: mpsc::Receiver<ActorMessage>,
}

impl FlashbotsClient {
    pub fn new() -> Self {
        let (sender, mailbox) = mpsc::channel(100);

        Self {
            sender,
            mailbox,
            best_payload: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn get_best_payload(&self) -> Option<OpExecutionPayloadEnvelopeV3> {
        self.best_payload.read().await.clone()
    }

    pub fn init(&mut self, ws_url: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = Url::parse(&ws_url)?;
        let sender = self.sender.clone();

        // Spawn WebSocket handler
        tokio::spawn(async move {
            loop {
                match connect_websocket(&url, sender.clone()).await {
                    Ok(()) => break,
                    Err(e) => {
                        error!(
                            message = "Flashbots WebSocket connection error, retrying in 5 seconds",
                            error = %e
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        // Take ownership of mailbox and state for the actor loop
        let mut mailbox = std::mem::replace(&mut self.mailbox, mpsc::channel(1).1);
        let best_payload = self.best_payload.clone();

        // Spawn actor's event loop
        tokio::spawn(async move {
            while let Some(message) = mailbox.recv().await {
                match message {
                    ActorMessage::BestPayload { response } => {
                        *best_payload.write().await = Some(response);
                    }
                }
            }
        });

        Ok(())
    }
}

async fn connect_websocket(
    url: &Url,
    sender: mpsc::Sender<ActorMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (ws_stream, _) = connect_async(url.as_str()).await?;
    let (_write, mut read) = ws_stream.split();

    info!(message = "Flashbots WebSocket connected", url = %url);

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<OpExecutionPayloadEnvelopeV3>(&text) {
                    Ok(payload) => {
                        debug!(message = "Received new payload from Flashbots");
                        let _ = sender
                            .send(ActorMessage::BestPayload { response: payload })
                            .await;
                    }
                    Err(e) => {
                        println!("{}", text);
                        error!(message = "Failed to parse payload", error = %e);
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!(message = "Received close frame");
                break;
            }
            Err(e) => {
                error!(message = "WebSocket error", error = %e);
                return Err(e.into());
            }
            _ => {} // Ignore other message types
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_flashbots_client() {}
}
