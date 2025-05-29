use crate::flashblocks::inbound::FlashblocksReceiverService;
use crate::flashblocks::outbound::FlashblocksOutboundService;
use crate::{FlashblocksService, RpcClient};
use std::net::SocketAddr;
use tokio::sync::mpsc;

pub struct Flashblocks {}

impl Flashblocks {
    pub fn run(
        builder_url: RpcClient,
        flashblocks_url: String,
        outbound_addr: String,
    ) -> eyre::Result<FlashblocksService> {
        let (tx, rx) = mpsc::channel(100);
        let (outbound_tx, outbound_rx) = mpsc::channel(100);

        let receiver = FlashblocksReceiverService::new(flashblocks_url, tx)?;
        tokio::spawn(async move {
            let _ = receiver.run().await;
        });

        // Create and spawn the outbound WebSocket service
        let outbound_service = FlashblocksOutboundService::new(outbound_rx);
        let addr: SocketAddr = outbound_addr
            .parse()
            .map_err(|e| eyre::eyre!("Invalid outbound address {}: {}", outbound_addr, e))?;

        tokio::spawn(async move {
            if let Err(e) = outbound_service.run(addr).await {
                tracing::error!("Outbound service error: {}", e);
            }
        });

        let service = FlashblocksService::new(builder_url, outbound_tx);
        let mut service_handle = service.clone();
        tokio::spawn(async move {
            service_handle.run(rx).await;
        });

        Ok(service)
    }
}
