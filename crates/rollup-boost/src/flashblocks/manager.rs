use crate::flashblocks::inbound::FlashblocksReceiverService;
use crate::{FlashblocksService, RpcClient};
use core::net::SocketAddr;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::watch::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tokio_util::bytes::Bytes;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::FlashblocksPayloadV1;

#[derive(thiserror::Error, Debug)]
pub enum FlashblocksManagerError {
    #[error("Ping failed")]
    PingFailed,
    #[error("Missing pong response")]
    MissingPong,
    #[error(transparent)]
    ConnectError(#[from] tungstenite::Error),
    #[error(transparent)]
    FlashblocksPayloadSendError(#[from] broadcast::error::SendError<FlashblocksPayloadV1>),
    #[error(transparent)]
    MessageSendError(#[from] watch::error::SendError<Message>),
    #[error(transparent)]
    RecvError(#[from] RecvError),
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
}

// NOTE: update to use FlashblocksPublisher and FlashblocksSubscriber
pub struct FlashblocksPubSubManager {
    // TODO: ping timeout
    // TODO: last ping
}

impl FlashblocksPubSubManager {
    pub fn new() -> Self {
        todo!()
    }

    pub fn spawn(
        builder_ws_endpoint: Url,
    ) -> Result<broadcast::Receiver<FlashblocksPayloadV1>, FlashblocksManagerError> {
        let (payload_tx, payload_rx) = broadcast::channel(100);
        FlashblocksSubscriber::spawn(builder_ws_endpoint, payload_tx)?;

        Ok(payload_rx)
    }

    // NOTE: multiplex flashblocks stream to multiple connections
    fn publish_flashblocks(&self) {
        todo!()
    }
}

#[derive(Clone, Debug)]
pub struct FlashblocksSubscriber;

impl FlashblocksSubscriber {
    // TODO: decide if return joinhandle
    fn spawn(
        builder_ws_endpoint: Url,
        payload_tx: broadcast::Sender<FlashblocksPayloadV1>,
    ) -> Result<(), FlashblocksManagerError> {
        let payload_tx = Arc::new(payload_tx);
        tokio::spawn(async move {
            loop {
                let Ok((ws_stream, _)) = connect_async(builder_ws_endpoint.as_str()).await else {
                    // TODO: log error
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                };

                let (sink, stream) = ws_stream.split();
                let (pong_tx, mut pong_rx) = watch::channel(Message::Pong(Bytes::default()));
                pong_rx.mark_changed();

                let ping_handle = spawn_ping(sink, pong_rx);
                let stream_handle = FlashblocksSubscriber::handle_flashblocks_stream(
                    stream,
                    payload_tx.clone(),
                    pong_tx,
                );

                tokio::select! {
                    _ = ping_handle => {
                        tracing::warn!("Ping handle resolved early, re-establing connection");
                    }
                    _ = stream_handle => {
                        tracing::warn!("Flashblocks stream handle resolved early, re-establing connection");
                    }
                }

                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });

        Ok(())
    }

    fn handle_flashblocks_stream(
        mut stream: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        payload_tx: Arc<broadcast::Sender<FlashblocksPayloadV1>>,
        pong_tx: watch::Sender<Message>,
    ) -> JoinHandle<Result<(), FlashblocksManagerError>> {
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let msg = msg.map_err(|e| {
                    tracing::error!("Ws connection error: {e}");
                    e
                })?;

                match msg {
                    Message::Text(text) => {
                        let flashblock_payload =
                            serde_json::from_str::<FlashblocksPayloadV1>(&text)?;
                        payload_tx.send(flashblock_payload)?;
                    }

                    Message::Pong(_) => {
                        pong_tx.send(Message::Pong(Bytes::default()))?;
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
}

// TODO: implement timeout logic here on when we expect a pong
fn spawn_ping(
    mut sink: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pong_rx: tokio::sync::watch::Receiver<Message>,
) -> JoinHandle<Result<(), FlashblocksManagerError>> {
    tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            ping_interval.tick().await;
            if pong_rx.has_changed()? {
                sink.send(Message::Ping(Bytes::new()))
                    .await
                    .map_err(|_| FlashblocksManagerError::PingFailed)?;
            } else {
                tracing::error!("Missing pong response from builder stream");
                return Err(FlashblocksManagerError::MissingPong);
            }
        }
    })
}

pub struct FlashblocksPublisher;
