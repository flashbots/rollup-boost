use thiserror::Error;

#[derive(Error, Debug, Eq, PartialEq)]
pub enum FlashblocksP2PError {
    #[error("attempt to publish flashblocks without clearance")]
    NotClearedToPublish,
    #[error(
        "attempt to publish flashblocks with expired authorization. Make sure to call `start_publishing` first"
    )]
    ExpiredAuthorization,
}
