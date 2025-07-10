#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use clap::Parser;
use ed25519_dalek::VerifyingKey;
use flashblocks_p2p::protocol::handler::{FlashblocksP2PState, FlashblocksProtoHandler};
use flashblocks_rpc::{EthApiOverrideServer, FlashblocksApiExt,  FlashblocksOverlayBuilder};
use reth_ethereum::network::{NetworkProtocols, protocol::IntoRlpxSubProtocol};
use reth_optimism_cli::{Cli, chainspec::OpChainSpecParser};
use reth_optimism_node::{OpNode, args::RollupArgs};
use rollup_boost::parse_vk;
use tokio::sync::mpsc;
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(next_help_heading = "Rollup")]
struct FlashblocksRollupArgs {
    #[command(flatten)]
    rollup_args: RollupArgs,

    #[arg(long = "flashblocks.enabled", default_value = "false")]
    flashblocks_enabled: bool,

    #[arg(long = "flashblocks.websocket-url", value_name = "WEBSOCKET_URL")]
    websocket_url: url::Url,

    #[arg(long, 
        env = "FLASHBLOCKS_BUILDER_VK", value_parser = parse_vk,
        required = false,
    )]
    pub flashblocks_builder_vk: VerifyingKey,
}

pub fn main() {
    if let Err(err) =
        Cli::<OpChainSpecParser, FlashblocksRollupArgs>::parse().run(async move |builder, args| {
            let rollup_args = args.rollup_args;
            let chain_spec = builder.config().chain.clone();
            let (tx, events) = mpsc::unbounded_channel();

            let flashblocks_overlay_builder =
                FlashblocksOverlayBuilder::new(chain_spec, args.flashblocks_builder_vk, events);
            let flashblocks_overlay = flashblocks_overlay_builder.start()?;

            info!(target: "reth::cli", "Launching Flashblocks RPC overlay node");
            let handle = builder
                .node(OpNode::new(rollup_args))
                .extend_rpc_modules(move |ctx| {
                    if args.flashblocks_enabled {

                        let eth_api = ctx.registry.eth_api().clone();
                        let api_ext = FlashblocksApiExt::new(eth_api.clone(), flashblocks_overlay);

                        ctx.modules.replace_configured(api_ext.into_rpc())?;
                    }
                    Ok(())
                })
                .launch_with_debug_capabilities()
                .await?;


            let custom_rlpx_handler = FlashblocksProtoHandler {
                network_handle: handle.node.network.clone(),
                state: FlashblocksP2PState { events: tx },
            };

            handle
                .node
                .network
                .add_rlpx_sub_protocol(custom_rlpx_handler.into_rlpx_sub_protocol());
            handle.node_exit_future.await
        })
    {
        tracing::error!("Error: {err:?}");
        std::process::exit(1);
    }
}
