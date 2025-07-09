use thiserror::Error;

#[derive(Error, Debug)]
pub enum FlashblocksP2PError {
    #[error("invalid authorizer signature")]
    InvalidAuthorizerSig,
    #[error("invalid builder signature")]
    InvalidBuilderSig,
}
