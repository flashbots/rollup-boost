use clap::{Args, Parser};
use ed25519_dalek::{SigningKey, VerifyingKey};
use eyre::Context;
use url::Url;

use hex::FromHex;

#[derive(Args, Clone, Debug)]
#[group(requires = "flashblocks")]
pub struct FlashblocksArgs {
    /// Enable Flashblocks client
    #[arg(long, env, required = false)]
    pub flashblocks: bool,

    /// Flashblocks Builder WebSocket URL
    #[arg(
        long,
        env,
        default_value = "ws://127.0.0.1:1111"
    )]
    pub flashblocks_builder_url: Url,

    /// Flashblocks WebSocket host for outbound connections
    #[arg(
        long,
        env,
        default_value = "127.0.0.1"
    )]
    pub flashblocks_host: String,

    /// Flashblocks WebSocket port for outbound connections
    #[arg(
        long,
        env,
        default_value = "1112"
    )]
    pub flashblocks_port: u16,

    /// Time used for timeout if builder disconnected
    #[arg(
        long,
        env,
        default_value = "5000"
    )]
    pub flashblock_builder_ws_reconnect_ms: u64,

    #[arg(
        long,
        env = "FLASHBLOCKS_AUTHORIZATION_SK", value_parser = parse_sk,
        required = false,
    )]
    pub flashblocks_authorization_sk: SigningKey,

    #[arg(long, 
        env = "FLASHBLOCKS_BUILDER_VK", value_parser = parse_vk,
        required = false,
    )]
    pub flashblocks_builder_vk: VerifyingKey,
}

fn parse_sk(s: &str) -> eyre::Result<SigningKey> {
    let bytes =
        <[u8; 32]>::from_hex(s.trim()).context("failed parsing flashblocks_authorization_sk")?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn parse_vk(s: &str) -> eyre::Result<VerifyingKey> {
    let bytes = <[u8; 32]>::from_hex(s.trim()).context("failed parsing flashblocks_builder_vk")?;
    Ok(VerifyingKey::from_bytes(&bytes)?)
}
