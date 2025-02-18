mod inbound;
mod primitives;
mod service;
use inbound::FlashblocksReceiverService;
use tokio::sync::mpsc;

pub use service::FlashblocksService;

pub struct Flashblocks {}

impl Flashblocks {
    pub fn run(builder_url: String) -> eyre::Result<FlashblocksService> {
        let (tx, rx) = mpsc::channel(100);
        let receiver = FlashblocksReceiverService::new(builder_url, tx).unwrap();

        tokio::spawn(async move {
            let _ = receiver.run().await;
        });

        let service = FlashblocksService::new();
        let mut service_handle = service.clone();
        tokio::spawn(async move {
            service_handle.run(rx).await;
        });

        Ok(service)
    }
}
