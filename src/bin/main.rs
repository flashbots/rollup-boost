use clap::Parser;
use rollup_boost::Args;

use dotenv::dotenv;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    dotenv().ok();
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install TLS ring CryptoProvider");

    Args::parse().run().await
}
