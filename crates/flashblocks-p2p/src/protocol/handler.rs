use crate::connection::handler::FlashblocksConnectionHandler;
use crate::protocol::proto::FlashblocksProtoMessage;
use ed25519_dalek::VerifyingKey;
use parking_lot::Mutex;
use reth::payload::PayloadId;
use reth_ethereum::network::{api::PeerId, protocol::ProtocolHandler};
use reth_network::Peers;
use rollup_boost::FlashblocksPayloadV1;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

pub trait FlashblocksP2PNetworHandle: Clone + Unpin + Peers + std::fmt::Debug + 'static {}

impl<N: Clone + Unpin + Peers + std::fmt::Debug + 'static> FlashblocksP2PNetworHandle for N {}

/// Protocol state is an helper struct to store the protocol events.
#[derive(Debug)]
pub struct FlashblocksP2PState {
    /// The index of the next flashblock to emit over the flashblocks_stream.
    pub flashblock_index: usize,
    /// Timestamp of the most recent flashblocks payload.
    pub payload_timestamp: u64,
    /// Most recent payload id.
    pub payload_id: PayloadId,
    /// Buffer of flashblocks for the current payload.
    pub flashblocks: Vec<Option<FlashblocksPayloadV1>>,
}

/// The protocol handler takes care of incoming and outgoing connections.
#[derive(Debug)]
pub struct FlashblocksProtoHandler<N> {
    /// Network handle, used to update peer state.
    pub network_handle: N,
    /// Authorizer verifying, used to verify flashblocks payloads.
    pub authorizer_vk: VerifyingKey,
    /// Sender for newly created or received and validated flashblocks payloads
    /// which will be broadcasted to all peers. May not be strictly ordered.
    /// TODO: we still need an internal listener for this channel to
    /// handle locally created flashblocks.
    pub peer_tx: broadcast::Sender<FlashblocksProtoMessage>,
    /// Receiver of verified and strictly ordered flashbloacks payloads.
    /// For consumption by the rpc overlay.
    pub flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
    /// Mutable state of the flashblocks protocol.
    pub state: Arc<Mutex<FlashblocksP2PState>>,
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksProtoHandler<N> {
    /// Creates a new protocol handler with the given state.
    pub fn new(
        network_handle: N,
        authorizer_vk: VerifyingKey,
        flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
        peer_tx: broadcast::Sender<FlashblocksProtoMessage>,
    ) -> Self {
        println!("Protocol created: {authorizer_vk:?}");
        let state = Arc::new(Mutex::new(FlashblocksP2PState {
            flashblock_index: 0,
            payload_timestamp: 0,
            payload_id: PayloadId::default(),
            flashblocks: vec![],
        }));

        Self {
            network_handle,
            authorizer_vk,
            peer_tx,
            flashblock_tx,
            state,
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> ProtocolHandler for FlashblocksProtoHandler<N> {
    type ConnectionHandler = FlashblocksConnectionHandler<N>;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        info!("here4");
        Some(FlashblocksConnectionHandler::<N> {
            network_handle: self.network_handle.clone(),
            authorizer_vk: self.authorizer_vk,
            peer_tx: self.peer_tx.clone(),
            flashblock_tx: self.flashblock_tx.clone(),
            state: self.state.clone(),
        })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        info!("here3");
        Some(FlashblocksConnectionHandler::<N> {
            network_handle: self.network_handle.clone(),
            authorizer_vk: self.authorizer_vk,
            peer_tx: self.peer_tx.clone(),
            flashblock_tx: self.flashblock_tx.clone(),
            state: self.state.clone(),
        })
    }
}
