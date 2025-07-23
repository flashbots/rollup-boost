use ed25519_dalek::{SigningKey, VerifyingKey};
use reth::chainspec::Hardforks;
use reth_eth_wire::NetPrimitivesFor;
use reth_ethereum::network::api::FullNetwork;
use reth_network::{NetworkProtocols, protocol::IntoRlpxSubProtocol};
use reth_node_api::{PrimitivesTy, TxTy};
use reth_node_builder::{
    BuilderContext,
    components::NetworkBuilder,
    node::{FullNodeTypes, NodeTypes},
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use rollup_boost::FlashblocksPayloadV1;
use tokio::sync::broadcast;

use crate::protocol::handler::{FlashblocksHandler, FlashblocksP2PNetworHandle};

#[derive(Debug)]
struct FlashblocksNetworkBuilderCtx {
    authorizer_vk: VerifyingKey,
    builder_sk: SigningKey,
    flashblocks_receiver_tx: broadcast::Sender<FlashblocksPayloadV1>,
}

#[derive(Debug)]
pub struct FlashblocksNetworkBuilder<T> {
    inner: T,
    ctx: Option<FlashblocksNetworkBuilderCtx>,
}

impl<T> FlashblocksNetworkBuilder<T> {
    pub fn new(
        inner: T,
        authorizer_vk: VerifyingKey,
        builder_sk: SigningKey,
        flashblocks_receiver_tx: broadcast::Sender<FlashblocksPayloadV1>,
    ) -> Self {
        Self {
            inner,
            ctx: Some(FlashblocksNetworkBuilderCtx {
                authorizer_vk,
                builder_sk,
                flashblocks_receiver_tx,
            }),
        }
    }

    pub fn disabled(inner: T) -> Self {
        Self { inner, ctx: None }
    }
}

impl<T, Network, Node, Pool> NetworkBuilder<Node, Pool> for FlashblocksNetworkBuilder<T>
where
    T: NetworkBuilder<Node, Pool, Network = Network>,
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec: Hardforks>>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    Network: FlashblocksP2PNetworHandle
        + NetworkProtocols
        + FullNetwork<Primitives: NetPrimitivesFor<PrimitivesTy<Node::Types>>>,
{
    type Network = T::Network;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::Network> {
        let handle = self.inner.build_network(ctx, pool).await?;
        if let Some(ctx) = self.ctx {
            let handler = FlashblocksHandler::<Network>::new(
                handle.clone(),
                ctx.authorizer_vk,
                ctx.builder_sk,
                ctx.flashblocks_receiver_tx,
            );
            handle.add_rlpx_sub_protocol(handler.into_rlpx_sub_protocol());
        }

        Ok(handle)
    }
}
