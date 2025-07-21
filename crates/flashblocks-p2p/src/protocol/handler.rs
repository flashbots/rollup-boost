use crate::protocol::connection::FlashblocksConnection;
use alloy_rlp::BytesMut;
use ed25519_dalek::VerifyingKey;
use parking_lot::Mutex;
use reth::payload::PayloadId;
use reth_eth_wire::Capability;
use reth_ethereum::network::{api::PeerId, protocol::ProtocolHandler};
use reth_network::Peers;
use rollup_boost::{FlashblocksP2PMsg, FlashblocksPayloadV1};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::debug;

use reth_ethereum::network::{
    api::Direction,
    eth_wire::{capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol},
    protocol::{ConnectionHandler, OnNotSupported},
};
use tokio_stream::wrappers::BroadcastStream;

/// Maximum frame size for rlpx messages.
const MAX_FRAME: usize = 1 << 24; // 16 MiB

/// Maximum index for flashblocks payloads.
/// Not intended to ever be hit. Since we resize the flashblocks vector dynamically,
/// this is just a sanity check to prevent excessive memory usage.
const MAX_FLASHBLOCK_INDEX: usize = 100;

pub trait FlashblocksP2PNetworHandle: Clone + Unpin + Peers + std::fmt::Debug + 'static {}

impl<N: Clone + Unpin + Peers + std::fmt::Debug + 'static> FlashblocksP2PNetworHandle for N {}

/// Protocol state is an helper struct to store the protocol events.
#[derive(Debug, Default)]
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
#[derive(Clone, Debug)]
pub struct FlashblocksP2PCtx<N> {
    /// Network handle, used to update peer state.
    pub network_handle: N,
    /// Authorizer verifying, used to verify flashblocks payloads.
    pub authorizer_vk: VerifyingKey,
    /// Sender for flashblocks payloads which will be broadcasted to all peers.
    /// May not be strictly ordered.
    pub peer_tx: broadcast::Sender<(PayloadId, usize, BytesMut)>,
    /// Receiver of verified and strictly ordered flashbloacks payloads.
    /// For consumption by the rpc overlay.
    pub flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
}

/// The protocol handler takes care of incoming and outgoing connections.
#[derive(Clone, Debug)]
pub struct FlashblocksHandler<N> {
    /// Network handle, used to update peer state.
    pub ctx: FlashblocksP2PCtx<N>,
    /// Mutable state of the flashblocks protocol.
    pub state: Arc<Mutex<FlashblocksP2PState>>,
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksHandler<N> {
    /// Creates a new protocol handler with the given state.
    pub fn new(
        network_handle: N,
        authorizer_vk: VerifyingKey,
        flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
        mut publish_rx: broadcast::Receiver<FlashblocksP2PMsg>,
    ) -> Self {
        let peer_tx = broadcast::Sender::new(100);
        let state = Arc::new(Mutex::new(FlashblocksP2PState::default()));
        let ctx = FlashblocksP2PCtx {
            network_handle: network_handle.clone(),
            authorizer_vk,
            peer_tx,
            flashblock_tx,
        };

        let state_clone = state.clone();
        let ctx_clone = ctx.clone();
        tokio::spawn({
            async move {
                while let Ok(msg) = publish_rx.recv().await {
                    let mut state = state_clone.lock();
                    ctx_clone.publish(&mut state, msg);
                }
            }
        });

        Self { ctx, state }
    }

    /// Returns the capability for the `flashblocks v1` p2p rotocol.
    pub fn capability() -> Capability {
        Capability::new_static("flblk", 1)
    }
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksP2PCtx<N> {
    /// Commit new and already verified flashblocks payloads to the state
    /// broadcast them to peers, and publish them to the stream.
    pub fn publish(&self, state: &mut FlashblocksP2PState, msg: FlashblocksP2PMsg) {
        let FlashblocksP2PMsg::FlashblocksPayloadV1(ref message) = msg;

        // Resize our array if needed
        if message.payload.index as usize > MAX_FLASHBLOCK_INDEX {
            tracing::error!(
                target: "flashblocks::p2p",
                index = message.payload.index,
                max_index = MAX_FLASHBLOCK_INDEX,
                "Received flashblocks payload with index exceeding maximum"
            );
            return;
        }
        let len = state.flashblocks.len();
        state
            .flashblocks
            .resize_with(len.max(message.payload.index as usize + 1), || None);
        let flashblock = &mut state.flashblocks[message.payload.index as usize];

        // If we've already seen this index, skip it
        // Otherwise, add it to the list
        if flashblock.is_none() {
            // We haven't seen this index yet
            // Add the flashblock to our cache
            *flashblock = Some(message.clone().payload);
            tracing::trace!(
                target: "flashblocks::p2p",
                payload_id = %message.payload.payload_id,
                flashblock_index = message.payload.index,
                "queueing flashblock",
            );
            let bytes = msg.encode();
            let len = bytes.len();
            metrics::histogram!("flashblock_size").record(len as f64);

            if len > MAX_FRAME {
                tracing::error!(
                    target: "flashblocks::p2p",
                    size = bytes.len(),
                    max_size = MAX_FRAME,
                    "FlashblocksP2PMsg too large",
                );
                return;
            }
            if len > MAX_FRAME / 2 {
                tracing::warn!(
                    target: "flashblocks::p2p",
                    size = bytes.len(),
                    max_size = MAX_FRAME,
                    "FlashblocksP2PMsg almost too large",
                );
            }
            self.peer_tx
                .send((
                    message.payload.payload_id,
                    message.payload.index as usize,
                    bytes,
                ))
                .ok();
            // Broadcast any flashblocks in the cache that are in order
            while let Some(Some(flashblock_event)) = state.flashblocks.get(state.flashblock_index) {
                // Publish the flashblock
                debug!(
                    target: "flashblocks::p2p",
                    payload_id = %flashblock_event.payload_id,
                    flashblock_index = %state.flashblock_index,
                    "publishing flashblock"
                );
                self.flashblock_tx.send(flashblock_event.clone()).ok();
                // Update the index
                state.flashblock_index += 1;
            }
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> ProtocolHandler for FlashblocksHandler<N> {
    type ConnectionHandler = Self;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(self.clone())
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(self.clone())
    }
}

impl<N: FlashblocksP2PNetworHandle> ConnectionHandler for FlashblocksHandler<N> {
    type Connection = FlashblocksConnection<N>;

    fn protocol(&self) -> Protocol {
        Protocol::new(Self::capability(), 1)
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let capability = Self::capability();

        debug!(
            %peer_id,
            %direction,
            capability = %capability.name,
            version = %capability.version,
            "new flashblocks connection"
        );

        FlashblocksConnection {
            peer_rx: BroadcastStream::new(self.ctx.peer_tx.subscribe()),
            handler: self,
            conn,
            peer_id,
            payload_id: Default::default(),
            received: Vec::new(),
        }
    }
}
