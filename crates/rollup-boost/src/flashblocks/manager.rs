use crate::flashblocks::inbound::FlashblocksReceiverService;
use crate::{FlashblocksService, RpcClient};
use core::net::SocketAddr;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
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
        let (mut sink, mut stream) = ws_stream.split();

        let ping_handle = spawn_ping(sink);

        // TODO: handle stream

        Ok(())
    }
}

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
}
