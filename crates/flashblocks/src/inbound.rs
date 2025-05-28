use super::primitives::FlashblocksPayloadV1;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info};
use url::Url;

pub struct FlashblocksReceiverService {
    url: Url,
    sender: mpsc::Sender<FlashblocksPayloadV1>,
}

impl FlashblocksReceiverService {
    pub fn new(
        url: String,
        sender: mpsc::Sender<FlashblocksPayloadV1>,
    ) -> Result<Self, url::ParseError> {
        Ok(Self {
            url: Url::parse(&url)?,
            sender,
        })
    }

    pub async fn run(self) {
        loop {
            match self.connect_and_handle().await {
                Ok(()) => break,
                Err(e) => {
                    error!(
                        message = "Flashblocks receiver connection error, retrying in 5 seconds",
                        error = %e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn connect_and_handle(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _) = connect_async(self.url.as_str()).await?;
        let (_, mut read) = ws_stream.split();

        info!("Connected to Flashblocks receiver at {}", self.url);

        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(text) => {
                    if let Ok(flashblocks_msg) = serde_json::from_str::<FlashblocksPayloadV1>(&text)
                    // TODO: Version this
                    {
                        self.sender.send(flashblocks_msg).await?;
                    }
                }
                _ => continue,
            }
        }

        Ok(())
    }
}
