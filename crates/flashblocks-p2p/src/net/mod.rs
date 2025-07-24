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

use crate::protocol::handler::{FlashblocksHandler, FlashblocksP2PNetworHandle};

#[derive(Debug)]
pub struct FlashblocksNetworkBuilder<T, N> {
    inner: T,
    flashblocks_p2p_handler: Option<FlashblocksHandler<N>>,
}

impl<T, N> FlashblocksNetworkBuilder<T, N> {
    pub fn new(inner: T, flashblocks_p2p_handler: FlashblocksHandler<N>) -> Self {
        Self {
            inner,
            flashblocks_p2p_handler: Some(flashblocks_p2p_handler),
        }
    }

    pub fn disabled(inner: T) -> Self {
        Self {
            inner,
            flashblocks_p2p_handler: None,
        }
    }
}

impl<T, Network, Node, Pool> NetworkBuilder<Node, Pool> for FlashblocksNetworkBuilder<T, Network>
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
        if let Some(flashblocks_p2p_handler) = self.flashblocks_p2p_handler {
            handle.add_rlpx_sub_protocol(flashblocks_p2p_handler.into_rlpx_sub_protocol());
        }

        Ok(handle)
    }
}
