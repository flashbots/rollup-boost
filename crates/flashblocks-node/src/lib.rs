use clap::Args;
use ed25519_dalek::VerifyingKey;
use rollup_boost::parse_vk;

#[derive(Args, Clone, Debug)]
#[group(requires = "flashblocks")]
pub struct FlashblocksNodeArgs {
    /// Enable Flashblocks client
    #[arg(long, env, required = false)]
    pub flashblocks: bool,

    #[arg(
        long = "flashblocks.authorizor_vk",
        env = "FLASHBLOCKS_AUTHORIZOR_VK",
        value_parser = parse_vk,
        required = false,
    )]
    pub authorizor_vk: VerifyingKey,
}
