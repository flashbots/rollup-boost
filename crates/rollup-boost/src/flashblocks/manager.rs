use crate::flashblocks::inbound::FlashblocksReceiverService;
use crate::{FlashblocksService, RpcClient};
use core::net::SocketAddr;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{self, Sender};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tokio_util::bytes::Bytes;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::FlashblocksPayloadV1;

pub struct FlashblocksManager {
    builder_ws_endpoint: Url,
}

impl FlashblocksManager {
    pub fn new() -> Self {
        todo!()
    }

    pub fn spawn(self) {
        let (tx, rx) = mpsc::channel(100);
        self.subscribe_flashblocks(tx);

        // TODO: serve stream to multiple connections

        todo!();
    }

    pub async fn subscribe_flashblocks(
        &self,
        tx: Sender<FlashblocksPayloadV1>,
    ) -> Result<(), FlashblocksManagerError> {
        // TODO: establish connection
        let (ws_stream, _) = connect_async(self.builder_ws_endpoint.as_str()).await?;
        let (sink, stream) = ws_stream.split();

        let ping_handle = spawn_ping(sink);
        let stream_handle = handle_flashblocks_stream(stream, tx);

        Ok(())
    }
}

fn handle_flashblocks_stream(
    mut stream: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    tx: Sender<FlashblocksPayloadV1>,
) -> JoinHandle<Result<(), FlashblocksManagerError>> {
    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| {
                tracing::error!("Ws connection error: {e}");
                e
            })?;

            match msg {
                Message::Text(text) => {
                    let flashblock_payload = serde_json::from_str::<FlashblocksPayloadV1>(&text)
                        .expect("TODO: handle error ");
                    tx.send(flashblock_payload).await?;
                }

                Message::Pong(_) => {
                    todo!("pong received")
                }

                Message::Close(_) => {
                    todo!("conection closed")
                }

                // TODO: handle other message types
                _ => {}
            }
        }

        Ok(())
    })
}

// TODO: implement timeout logic here on when we expect a pong
fn spawn_ping(
    mut sink: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
) -> JoinHandle<Result<(), FlashblocksManagerError>> {
    tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            ping_interval.tick().await;
            sink.send(Message::Ping(Bytes::new()))
                .await
                .map_err(|_| FlashblocksManagerError::PingFailed)?;
        }
    })
}

#[derive(thiserror::Error, Debug)]
pub enum FlashblocksManagerError {
    #[error("Ping failed")]
    PingFailed,
    #[error(transparent)]
    ConnectError(#[from] tungstenite::Error),
    #[error(transparent)]
    SendError(#[from] SendError<FlashblocksPayloadV1>),
}
