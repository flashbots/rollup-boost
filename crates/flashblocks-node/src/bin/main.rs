#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
use clap::Parser;
use ed25519_dalek::{SigningKey, VerifyingKey};
use flashblocks_node::FlashblocksNodeArgs;
use flashblocks_p2p::protocol::handler::{FlashblocksHandle, FlashblocksP2PProtocol};
use flashblocks_rpc::{EthApiOverrideServer, FlashblocksApiExt, FlashblocksOverlay};
use reth_ethereum::network::{NetworkProtocols, protocol::IntoRlpxSubProtocol};
use reth_optimism_cli::{Cli, chainspec::OpChainSpecParser};
use reth_optimism_node::{OpNode, args::RollupArgs};
use tracing::info;

#[derive(Debug, Clone, clap::Args)]
#[command(next_help_heading = "Rollup")]
struct FlashblocksRollupArgs {
    #[command(flatten)]
    pub rollup_args: RollupArgs,

    #[command(flatten)]
    pub flashblock_args: Option<FlashblocksNodeArgs>,
}

pub fn main() {
    if let Err(err) =
        Cli::<OpChainSpecParser, FlashblocksRollupArgs>::parse().run(async move |builder, args| {
            let rollup_args = args.rollup_args;
            let chain_spec = builder.config().chain.clone();
            let flashblocks_handle = FlashblocksHandle::new(
                VerifyingKey::default(),
                Some(SigningKey::from_bytes(&[0u8; 32])),
            );

            let flashblocks_overlay =
                FlashblocksOverlay::new(flashblocks_handle.clone(), chain_spec);
            flashblocks_overlay.clone().start()?;

            info!(target: "reth::cli", "Launching Flashblocks RPC overlay node");
            let handle = builder
                .node(OpNode::new(rollup_args))
                .extend_rpc_modules(move |ctx| {
                    if args.flashblock_args.is_some() {
                        let eth_api = ctx.registry.eth_api().clone();
                        let api_ext = FlashblocksApiExt::new(eth_api.clone(), flashblocks_overlay);

                        ctx.modules.replace_configured(api_ext.into_rpc())?;
                    }
                    Ok(())
                })
                .launch_with_debug_capabilities()
                .await?;

            let flashblocks_p2p_protocol =
                FlashblocksP2PProtocol::new(handle.node.network.clone(), flashblocks_handle);

            handle
                .node
                .network
                .add_rlpx_sub_protocol(flashblocks_p2p_protocol.into_rlpx_sub_protocol());
            handle.node_exit_future.await
        })
    {
        tracing::error!("Error: {err:?}");
        std::process::exit(1);
    }
}
