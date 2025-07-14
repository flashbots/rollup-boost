use crate::connection::handler::FlashblocksConnectionHandler;
use crate::protocol::proto::{FlashblocksProtoMessage, FlashblocksProtoMessageKind};
use ed25519_dalek::VerifyingKey;
use parking_lot::Mutex;
use reth::payload::PayloadId;
use reth_ethereum::network::{api::PeerId, protocol::ProtocolHandler};
use reth_network::Peers;
use rollup_boost::FlashblocksPayloadV1;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info};

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
    /// Sender for flashblocks payloads which will be broadcasted to all peers.
    /// May not be strictly ordered.
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
        publish_tx: broadcast::Sender<FlashblocksProtoMessage>,
    ) -> Self {
        let state = Arc::new(Mutex::new(FlashblocksP2PState {
            flashblock_index: 0,
            payload_timestamp: 0,
            payload_id: PayloadId::default(),
            flashblocks: vec![],
        }));

        // TODO: Clean up duplicated code
        let state_clone = state.clone();
        let mut publish_rx = publish_tx.subscribe();
        let peer_tx = broadcast::Sender::new(100);
        let peer_tx_clone = peer_tx.clone();
        let flashblock_tx_clone = flashblock_tx.clone();
        tokio::spawn({
            async move {
                while let Ok(msg) = publish_rx.recv().await {
                    let message = match msg.message {
                        FlashblocksProtoMessageKind::FlashblocksPayloadV1(message) => message,
                    };
                    // Check if we have a payload id and flashblocks to emit.
                    let mut state = state_clone.lock();
                    let len = state.flashblocks.len();
                    state
                        .flashblocks
                        .resize_with(len.max(message.payload.index as usize + 1), || None);
                    let flashblock = &mut state.flashblocks[message.payload.index as usize];
                    if flashblock.is_none() {
                        // We haven't seen this index yet
                        // Add the flashblock to our cache
                        *flashblock = Some(message.clone().payload);
                        tracing::debug!(
                            "Received flashblocks payload with id: {}, index: {}",
                            message.payload.payload_id,
                            message.payload.index
                        );
                        let message = FlashblocksProtoMessage {
                            message: FlashblocksProtoMessageKind::FlashblocksPayloadV1(message),
                            message_type: msg.message_type,
                        };
                        peer_tx_clone.send(message).ok();
                        // Broadcast any flashblocks in the cache that are in order
                        while let Some(Some(flashblock_event)) =
                            state.flashblocks.get(state.flashblock_index)
                        {
                            // Send the flashblock to the stream
                            debug!(payload_id = %flashblock_event.payload_id, flashblock_index = %state.flashblock_index, "Publishing new flashblock");
                            flashblock_tx_clone.send(flashblock_event.clone()).ok();
                            // Update the index
                            state.flashblock_index += 1;
                        }
                    }
                }
            }
        });

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
