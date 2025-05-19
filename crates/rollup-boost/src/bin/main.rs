use clap::Parser;
use rollup_boost::Args;
use rollup_boost::init_tracing;

use dotenv::dotenv;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    dotenv().ok();

    let args = Args::parse();
    init_tracing(&args)?;
    args.run().await
}
