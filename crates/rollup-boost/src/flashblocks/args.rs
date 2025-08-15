use clap::Args;
use ed25519_dalek::{SigningKey, VerifyingKey};

use hex::FromHex;

#[derive(Args, Clone, Debug)]
#[group(requires = "flashblocks")]
pub struct FlashblocksArgs {
    /// Enable Flashblocks client
    #[arg(long, env, required = false)]
    pub flashblocks: bool,

    #[arg(
        long,
        env = "FLASHBLOCKS_AUTHORIZER_SK",
        value_parser = parse_sk,
        required = false,
    )]
    pub flashblocks_authorizer_sk: SigningKey,

    #[arg(
        long,
        env = "FLASHBLOCKS_BUILDER_VK",
        value_parser = parse_vk,
        required = false,
    )]
    pub flashblocks_builder_vk: VerifyingKey,
}

pub fn parse_sk(s: &str) -> eyre::Result<SigningKey> {
    let bytes = <[u8; 32]>::from_hex(s.trim())?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub fn parse_vk(s: &str) -> eyre::Result<VerifyingKey> {
    let bytes = <[u8; 32]>::from_hex(s.trim())?;
    Ok(VerifyingKey::from_bytes(&bytes)?)
}
