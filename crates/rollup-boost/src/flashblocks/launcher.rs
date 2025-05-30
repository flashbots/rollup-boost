use crate::flashblocks::inbound::FlashblocksReceiverService;
use crate::{FlashblocksService, RpcClient};
use tokio::sync::mpsc;
use url::Url;

pub struct Flashblocks {}

impl Flashblocks {
    pub fn run(
        builder_url: RpcClient,
        flashblocks_url: Url,
        outbound_addr: String,
    ) -> eyre::Result<FlashblocksService> {
        let (tx, rx) = mpsc::channel(100);

        let receiver = FlashblocksReceiverService::new(flashblocks_url, tx)?;
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
