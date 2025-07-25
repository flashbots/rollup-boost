use thiserror::Error;

#[derive(Error, Debug, Eq, PartialEq)]
pub enum FlashblocksP2PError {
    #[error("attempt to publish flashblocks without clearance")]
    NotClearedToPublish,
    #[error(
        "attempt to publish flashblocks with expired authorization. Make sure to call `start_publishing` first"
    )]
    ExpiredAuthorization,
    #[error("input too short")]
    InputTooShort,
    #[error("unknown message type")]
    UnknownMessageType,
    #[error("invalid builder signature")]
    Rlp(#[from] alloy_rlp::Error),
}
