use clap::Parser;

#[derive(Parser, Clone, Debug)]
pub struct FlashblocksArgs {
    /// Enable Flashblocks client
    #[arg(long, env, default_value = "false")]
    pub flashblocks: bool,

    /// Flashblocks Builder WebSocket URL
    #[arg(long, env, default_value = "ws://localhost:1111")]
    pub flashblocks_builder_url: String,

    /// Flashblocks WebSocket host for outbound connections
    #[arg(long, env, default_value = "127.0.0.1")]
    pub flashblocks_host: String,

    /// Flashblocks WebSocket port for outbound connections
    #[arg(long, env, default_value = "1112")]
    pub flashblocks_port: u16,
}
