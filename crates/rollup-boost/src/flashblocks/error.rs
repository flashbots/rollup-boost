use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum FlashblocksError {
    #[error("invalid authorizer signature")]
    InvalidAuthorizerSig,
    #[error("invalid builder signature")]
    InvalidBuilderSig,
    #[error("input too short")]
    InputTooShort,
    #[error("unknown message type")]
    UnknownMessageType,
    #[error("invalid builder signature")]
    Rlp(#[from] alloy_rlp::Error),
    #[error("Missing base payload for initial flashblock")]
    MissingBasePayload,
    #[error("Unexpected base payload for non-initial flashblock")]
    UnexpectedBasePayload,
    #[error("Missing delta for flashblock")]
    MissingDelta,
    #[error("Invalid index for flashblock")]
    InvalidIndex,
    #[error("Missing payload")]
    MissingPayload,
}
