use crate::flashblocks::inbound::FlashblocksReceiverService;
use crate::{FlashblocksService, RpcClient};
use core::net::SocketAddr;
use tokio::sync::mpsc::{self, Sender};
use url::Url;

use super::FlashblocksPayloadV1;

pub struct FlashblocksManager;

impl FlashblocksManager {
    pub fn new() -> Self {
        todo!()
    }

    pub fn spawn(self) {
        todo!();

        let (tx, rx) = mpsc::channel(100);
        self.subscribe_flashblocks(tx);
        // TODO: establish stream with builder

        // TODO: serve stream to multiple connections
    }

    pub fn subscribe_flashblocks(&self, tx: Sender<FlashblocksPayloadV1>) {
        // TODO: establish connection
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
