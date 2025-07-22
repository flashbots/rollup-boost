use thiserror::Error;

#[derive(Error, Debug, Eq, PartialEq)]
pub enum FlashblocksP2PError {
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
}
