#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use clap::Parser;
use flashblocks_rpc::{EthApiOverrideServer, FlashblocksApiExt, FlashblocksOverlay};
use reth_optimism_cli::{Cli, chainspec::OpChainSpecParser};
use reth_optimism_node::{OpNode, args::RollupArgs};
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(next_help_heading = "Rollup")]
struct FlashblocksRollupArgs {
    #[command(flatten)]
    rollup_args: RollupArgs,

    #[arg(long = "websocket-url", value_name = "WEBSOCKET_URL")]
    websocket_url: String,
}

fn main() {
    if let Err(err) =
        Cli::<OpChainSpecParser, FlashblocksRollupArgs>::parse().run(async move |builder, args| {
            let rollup_args = args.rollup_args;

            info!(target: "reth::cli", "Launching node");
            let handle = builder
                .node(OpNode::new(rollup_args))
                .extend_rpc_modules(move |ctx| {
                    let flashblocks_overlay = FlashblocksOverlay::new();
                    let eth_api = ctx.registry.eth_api().clone();
                    let api_ext = FlashblocksApiExt::new(eth_api.clone(), flashblocks_overlay);

                    ctx.modules.replace_configured(api_ext.into_rpc())?;
                    Ok(())
                })
                .launch_with_debug_capabilities()
                .await?;
            handle.node_exit_future.await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
