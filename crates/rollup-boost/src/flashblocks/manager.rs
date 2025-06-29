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
use tokio_tungstenite::{WebSocketStream, connect_async};
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

        // TODO: spawn_ping
        let ping_handle: JoinHandle<Result<(), FlashblocksManagerError>> =
            tokio::spawn(async move {
                let mut ping_interval = tokio::time::interval(Duration::from_millis(500));
                loop {
                    ping_interval.tick().await;
                    sink.send(Message::Ping(Bytes::new()))
                        .await
                        .map_err(|_| FlashblocksManagerError::PingFailed)?;
                }
            });

        Ok(())
    }

    pub fn run(
        builder_url: RpcClient,
        flashblocks_url: Url,
        outbound_addr: SocketAddr,
        reconnect_ms: u64,
        // TODO: update error handling
    ) -> eyre::Result<FlashblocksService> {
        let (tx, rx) = mpsc::channel(100);

        let receiver = FlashblocksReceiverService::new(flashblocks_url, tx, reconnect_ms);
        tokio::spawn(async move {
            let _ = receiver.run().await;
        });

        let service = FlashblocksService::new(builder_url, outbound_addr)?;
        let mut service_handle = service.clone();
        tokio::spawn(async move {
            service_handle.run(rx).await;
        });

        Ok(service)
    }
}

// fn spawn_ping<S, Item>(mut sink: SplitSink<WebSocketStream<S>, Item>) -> JoinHandle<()> {
//     tokio::spawn(async move {
//         let mut ping_interval = tokio::time::interval(Duration::from_millis(500));
//         loop {
//             ping_interval.tick().await;
//             sink.send(Message::Ping(Bytes::new()));
//         }
//     })
// }
//

#[derive(thiserror::Error, Debug)]
pub enum FlashblocksManagerError {
    #[error("Ping failed")]
    PingFailed,
    #[error(transparent)]
    ConnectError(#[from] tungstenite::Error),
}
