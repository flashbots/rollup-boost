use ed25519_dalek::VerifyingKey;
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

use crate::protocol::{
    handler::{FlashblocksP2PNetworHandle, FlashblocksProtoHandler},
    proto::FlashblocksProtoMessage,
};

#[derive(Clone, Debug)]
pub struct FlashblocksNetworkBuilder<T> {
    inner: T,
    authorizer_vk: VerifyingKey,
    flashblocks_receiver_tx: broadcast::Sender<FlashblocksPayloadV1>,
    flashblock_sender_tx: broadcast::Sender<FlashblocksProtoMessage>,
}

impl<T> FlashblocksNetworkBuilder<T> {
    /// Creates a new `FlashblocksNetworkBuilder` with the given inner builder and events channel.
    pub fn new(
        inner: T,
        authorizer_vk: VerifyingKey,
        flashblocks_receiver_tx: broadcast::Sender<FlashblocksPayloadV1>,
        flashblock_sender_tx: broadcast::Sender<FlashblocksProtoMessage>,
    ) -> Self {
        Self {
            inner,
            authorizer_vk,
            flashblocks_receiver_tx,
            flashblock_sender_tx,
        }
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
        let handler = FlashblocksProtoHandler::<Network>::new(
            handle.clone(),
            self.authorizer_vk,
            self.flashblocks_receiver_tx,
            self.flashblock_sender_tx,
        );
        handle.add_rlpx_sub_protocol(handler.into_rlpx_sub_protocol());

        Ok(handle)
    }
}
