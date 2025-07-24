use crate::protocol::connection::FlashblocksConnection;
use alloy_rlp::BytesMut;
use ed25519_dalek::{SigningKey, VerifyingKey};
use eyre::bail;
use parking_lot::Mutex;
use reth::payload::PayloadId;
use reth_eth_wire::Capability;
use reth_ethereum::network::{api::PeerId, protocol::ProtocolHandler};
use reth_network::Peers;
use rollup_boost::{
    Authorization, Authorized, AuthorizedMsg, AuthorizedPayload, FlashblocksP2PMsg,
    FlashblocksPayloadV1, StartPublish, StopPublish,
};
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

/// The maximum number of blocks we will wait for a previous publisher to stop
const MAX_PUBLISH_WAIT_BLOCKS: u64 = 1;

/// Trait bound for network handles that can be used with the flashblocks P2P protocol.
///
/// This trait combines all the necessary bounds for a network handle to be used
/// in the flashblocks P2P system, including peer management capabilities.
pub trait FlashblocksP2PNetworHandle: Clone + Unpin + Peers + std::fmt::Debug + 'static {}

impl<N: Clone + Unpin + Peers + std::fmt::Debug + 'static> FlashblocksP2PNetworHandle for N {}

/// Messages that can be broadcast over a channel to each internal peer connection.
///
/// These messages are used internally to coordinate the broadcasting of flashblocks
/// and publishing status changes to all connected peers.
#[derive(Clone, Debug)]
pub enum PeerMsg {
    /// Send an already serialized flashblock to all peers.
    FlashblocksPayloadV1((PayloadId, usize, BytesMut)),
    /// Send a previously serialized StartPublish message to all peers.
    StartPublishing(BytesMut),
    /// Send a previously serialized StopPublish message to all peers.
    StopPublishing(BytesMut),
}

/// The current publishing status of this node in the flashblocks P2P network.
///
/// This enum tracks whether we are actively publishing flashblocks, waiting to publish,
/// or not publishing at all. It also maintains information about other active publishers
/// to coordinate multi-builder scenarios and handle failover situations.
#[derive(Clone, Debug)]
pub enum PublishingStatus {
    /// We are currently publishing flashblocks.
    Publishing {
        /// The authorization token that grants us permission to publish.
        authorization: Authorization,
    },
    /// We are waiting for the previous publisher to stop.
    WaitingToPublish {
        /// The authorization token we will use once we start publishing.
        authorization: Authorization,
        /// A map of active publishers (excluding ourselves) to their most recently published
        /// or requested to publish block number.
        active_publishers: Vec<(VerifyingKey, u64)>,
    },
    /// We are not currently publishing flashblocks.
    NotPublishing {
        /// A map of previous publishers to their most recently published
        /// or requested to publish block number.
        active_publishers: Vec<(VerifyingKey, u64)>,
    },
}

impl Default for PublishingStatus {
    fn default() -> Self {
        Self::NotPublishing {
            active_publishers: Vec::new(),
        }
    }
}

/// Protocol state that stores the flashblocks P2P protocol events and coordination data.
///
/// This struct maintains the current state of flashblock publishing, including coordination
/// with other publishers, payload buffering, and ordering information. It serves as the
/// central state management for the flashblocks P2P protocol handler.
#[derive(Debug, Default)]
pub struct FlashblocksP2PState {
    /// Current publishing status indicating whether we're publishing, waiting, or not publishing.
    pub publishing_status: PublishingStatus,
    /// Block number of the most recent flashblocks payload.
    /// We only receive the block number from `ExecutionPayloadBaseV1`
    /// so this may be set to `None` in the event that we receive the flashblocks payloads
    /// out of order.
    pub block_number: Option<u64>,
    /// Most recent payload ID for the current block being processed.
    pub payload_id: PayloadId,
    /// Timestamp of the most recent flashblocks payload.
    pub payload_timestamp: u64,
    /// The index of the next flashblock to emit over the flashblocks stream.
    /// Used to maintain strict ordering of flashblock delivery.
    pub flashblock_index: usize,
    /// Buffer of flashblocks for the current payload, indexed by flashblock sequence number.
    /// Contains `None` for flashblocks not yet received, enabling out-of-order receipt
    /// while maintaining in-order delivery.
    pub flashblocks: Vec<Option<FlashblocksPayloadV1>>,
}

impl FlashblocksP2PState {
    /// Returns the current publishing status of this node.
    ///
    /// This indicates whether the node is actively publishing flashblocks,
    /// waiting to publish, or not publishing at all.
    pub fn publishing_status(&self) -> PublishingStatus {
        self.publishing_status.clone()
    }
}

/// Context struct containing shared resources for the flashblocks P2P protocol.
///
/// This struct holds the network handle, cryptographic keys, and communication channels
/// used across all connections in the flashblocks P2P protocol. It provides the shared
/// infrastructure needed for message verification, signing, and broadcasting.
#[derive(Clone, Debug)]
pub struct FlashblocksP2PCtx<N> {
    /// Network handle used to update peer reputation and manage connections.
    pub network_handle: N,
    /// Authorizer's verifying key used to verify authorization signatures from rollup-boost.
    pub authorizer_vk: VerifyingKey,
    /// Builder's signing key used to sign outgoing authorized P2P messages.
    pub builder_sk: SigningKey,
    /// Broadcast sender for peer messages that will be sent to all connected peers.
    /// Messages may not be strictly ordered due to network conditions.
    pub peer_tx: broadcast::Sender<PeerMsg>,
    /// Broadcast sender for verified and strictly ordered flashblock payloads.
    /// Used by RPC overlays and other consumers of flashblock data.
    pub flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
}

/// Main protocol handler for the flashblocks P2P protocol.
///
/// This handler manages incoming and outgoing connections, coordinates flashblock publishing,
/// and maintains the protocol state across all peer connections. It implements the core
/// logic for multi-builder coordination and failover scenarios in HA sequencer setups.
#[derive(Clone, Debug)]
pub struct FlashblocksHandler<N> {
    /// Shared context containing network handle, keys, and communication channels.
    pub ctx: FlashblocksP2PCtx<N>,
    /// Thread-safe mutable state of the flashblocks protocol.
    /// Protected by a mutex to allow concurrent access from multiple connections.
    pub state: Arc<Mutex<FlashblocksP2PState>>,
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksHandler<N> {
    /// Creates a new flashblocks P2P protocol handler.
    ///
    /// Initializes the handler with the necessary cryptographic keys, network handle,
    /// and communication channels. The handler starts in a non-publishing state.
    ///
    /// # Arguments
    /// * `network_handle` - Network handle for peer management and reputation updates
    /// * `authorizer_vk` - Verifying key for validating authorization signatures from rollup-boost
    /// * `builder_sk` - Signing key for this builder to sign outgoing messages
    /// * `flashblock_tx` - Broadcast channel for publishing verified flashblocks to consumers
    pub fn new(
        network_handle: N,
        authorizer_vk: VerifyingKey,
        builder_sk: SigningKey,
        flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
    ) -> Self {
        let peer_tx = broadcast::Sender::new(100);
        let state = Arc::new(Mutex::new(FlashblocksP2PState::default()));
        let ctx = FlashblocksP2PCtx {
            network_handle: network_handle.clone(),
            authorizer_vk,
            builder_sk,
            peer_tx,
            flashblock_tx,
        };

        Self { ctx, state }
    }

    /// Returns the P2P capability for the flashblocks v1 protocol.
    ///
    /// This capability is used during devp2p handshake to advertise support
    /// for the flashblocks protocol with protocol name "flblk" and version 1.
    pub fn capability() -> Capability {
        Capability::new_static("flblk", 1)
    }

    /// Publishes a newly created flashblock from the payload builder to the P2P network.
    ///
    /// This method validates that the builder has authorization to publish and that
    /// the authorization matches the current publishing session. The flashblock is
    /// then processed, cached, and broadcast to all connected peers.
    ///
    /// # Arguments
    /// * `authorized_payload` - The signed flashblock payload with authorization
    ///
    /// # Returns
    /// * `Ok(())` if the flashblock was successfully published
    /// * `Err` if the builder lacks authorization or the authorization is outdated
    ///
    /// # Note
    /// You must call `start_publishing` before calling this method to establish
    /// authorization for the current block.
    pub fn publish_new(
        &self,
        authorized_payload: AuthorizedPayload<FlashblocksPayloadV1>,
    ) -> eyre::Result<()> {
        let mut state = self.state.lock();
        let PublishingStatus::Publishing { authorization } = &state.publishing_status else {
            bail!("attempt to publish flashblocks without clearance");
        };

        if authorization != &authorized_payload.authorized.authorization {
            bail!(
                "attempt to publish flashblocks with a previous authorization. Make sure to call `start_publishing` first."
            );
        }
        self.ctx.publish(&mut state, authorized_payload);
        Ok(())
    }

    /// Returns the current publishing status of this node.
    ///
    /// The status indicates whether the node is actively publishing flashblocks,
    /// waiting for another publisher to stop, or not publishing at all.
    ///
    /// # Returns
    /// The current `PublishingStatus` enum value
    pub fn publishing_status(&self) -> PublishingStatus {
        self.state.lock().publishing_status.clone()
    }

    /// Initiates flashblock publishing for a new block.
    ///
    /// This method should be called immediately after receiving a ForkChoiceUpdated
    /// with payload attributes and the corresponding Authorization token. It coordinates
    /// with other potential publishers to ensure only one builder publishes at a time.
    ///
    /// The method may transition the node to either Publishing or WaitingToPublish state
    /// depending on whether other builders are currently active.
    ///
    /// # Arguments
    /// * `new_authorization` - Authorization token signed by rollup-boost for this block
    /// * `new_block_number` - L2 block number being built
    /// * `new_payload_id` - Unique identifier for this block's payload
    ///
    /// # Note
    /// Calling this method does not guarantee immediate publishing clearance.
    /// The node may need to wait for other publishers to stop first.
    pub fn start_publishing(
        &self,
        new_authorization: Authorization,
        new_block_number: u64,
        new_payload_id: PayloadId,
    ) {
        let mut state = self.state.lock();
        match &mut state.publishing_status {
            PublishingStatus::Publishing { authorization } => {
                // We are already publishing, so we just update the authorization.
                *authorization = new_authorization;
            }
            PublishingStatus::WaitingToPublish {
                authorization,
                active_publishers,
            } => {
                let most_recent_publisher = active_publishers
                    .iter()
                    .map(|(_, block_number)| *block_number)
                    .max()
                    .unwrap_or_default();
                // We are waiting to publish, so we update the authorization and
                // the block number at which we requested to start publishing.
                if new_block_number >= most_recent_publisher + MAX_PUBLISH_WAIT_BLOCKS {
                    // If the block number is greater than the one we requested to start publishing,
                    // we will update it.
                    tracing::warn!(
                        target: "flashblocks::p2p",
                        %new_payload_id,
                        %new_block_number,
                        "waiting to publish timed out, starting to publish",
                    );
                    state.publishing_status = PublishingStatus::Publishing {
                        authorization: new_authorization,
                    };
                } else {
                    // Continue to wait for the previous builder to stop.
                    *authorization = new_authorization;
                }
            }
            PublishingStatus::NotPublishing { active_publishers } => {
                // Send an authorized `StartPublish` message to the network
                let authorized_msg = AuthorizedMsg::StartPublish(StartPublish {
                    block_number: new_block_number,
                });
                let authorized_payload =
                    Authorized::new(&self.ctx.builder_sk, new_authorization, authorized_msg);
                let p2p_msg = FlashblocksP2PMsg::Authorized(authorized_payload);
                let peer_msg = PeerMsg::StartPublishing(p2p_msg.encode());
                self.ctx.peer_tx.send(peer_msg).ok();

                if active_publishers.is_empty() {
                    // If we have no previous publishers, we can start publishing immediately.
                    tracing::info!(
                        target: "flashblocks::p2p",
                        payload_id = %new_payload_id,
                        "starting to publish flashblocks",
                    );
                    state.publishing_status = PublishingStatus::Publishing {
                        authorization: new_authorization,
                    };
                } else {
                    // If we have previous publishers, we will wait for them to stop.
                    tracing::info!(
                        target: "flashblocks::p2p",
                        payload_id = %new_payload_id,
                        "waiting to publish flashblocks",
                    );
                    state.publishing_status = PublishingStatus::WaitingToPublish {
                        authorization: new_authorization,
                        active_publishers: active_publishers.clone(),
                    };
                }
            }
        }
    }

    /// Stops flashblock publishing and notifies the P2P network.
    ///
    /// This method broadcasts a StopPublish message to all connected peers and transitions
    /// the node to a non-publishing state. It should be called when receiving a
    /// ForkChoiceUpdated without payload attributes or without an Authorization token.
    ///
    /// # Arguments
    /// * `block_height` - The L2 block number at which publishing is being stopped
    pub fn stop_publishing(&self, block_height: u64) {
        let mut state = self.state.lock();
        match &mut state.publishing_status {
            PublishingStatus::Publishing { authorization } => {
                // We are currently publishing, so we send a stop message.
                tracing::info!(
                    target: "flashblocks::p2p",
                    %block_height,
                    "stopping to publish flashblocks",
                );
                let authorized_payload =
                    Authorized::new(&self.ctx.builder_sk, *authorization, StopPublish.into());
                let p2p_msg = FlashblocksP2PMsg::Authorized(authorized_payload);
                let peer_msg = PeerMsg::StopPublishing(p2p_msg.encode());
                self.ctx.peer_tx.send(peer_msg).ok();
                state.publishing_status = PublishingStatus::NotPublishing {
                    active_publishers: Vec::new(),
                };
            }
            PublishingStatus::WaitingToPublish {
                authorization,
                active_publishers,
                ..
            } => {
                // We are waiting to publish, so we just update the status.
                tracing::info!(
                    target: "flashblocks::p2p",
                    %block_height,
                    "aborting wait to publish flashblocks",
                );
                let authorized_payload =
                    Authorized::new(&self.ctx.builder_sk, *authorization, StopPublish.into());
                let p2p_msg = FlashblocksP2PMsg::Authorized(authorized_payload);
                let peer_msg = PeerMsg::StopPublishing(p2p_msg.encode());
                self.ctx.peer_tx.send(peer_msg).ok();
                state.publishing_status = PublishingStatus::NotPublishing {
                    active_publishers: active_publishers.clone(),
                };
            }
            PublishingStatus::NotPublishing { .. } => {}
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksP2PCtx<N> {
    /// Processes and publishes a verified flashblock payload to the P2P network and local stream.
    ///
    /// This method handles the core logic of flashblock processing, including validation,
    /// caching, and broadcasting. It ensures flashblocks are delivered in order while
    /// allowing out-of-order receipt from the network.
    ///
    /// # Arguments
    /// * `state` - Mutable reference to the protocol state for updating flashblock cache
    /// * `authorized_payload` - The authorized flashblock payload to process and publish
    ///
    /// # Behavior
    /// - Validates payload consistency with authorization
    /// - Updates global state for new payloads with newer timestamps
    /// - Caches flashblocks and maintains ordering for sequential delivery
    /// - Broadcasts to peers and publishes ordered flashblocks to the stream
    pub fn publish(
        &self,
        state: &mut FlashblocksP2PState,
        authorized_payload: AuthorizedPayload<FlashblocksPayloadV1>,
    ) {
        let payload = authorized_payload.msg();
        let authorization = authorized_payload.authorized.authorization;

        // Do some basic validation
        if authorization.payload_id != payload.payload_id {
            // Since the builders are trusted, the only reason this should happen is a bug.
            tracing::error!(
                target: "flashblocks::p2p",
                authorization_payload_id = %authorization.payload_id,
                flashblock_payload_id = %payload.payload_id,
                "Authorization payload id does not match flashblocks payload id"
            );
            return;
        }

        // Check if this is a globally new payload
        if authorization.timestamp > state.payload_timestamp {
            state.block_number = payload.base.as_ref().map(|b| b.block_number);
            state.payload_id = authorization.payload_id;
            state.payload_timestamp = authorization.timestamp;
            state.flashblock_index = 0;
            state.flashblocks.fill(None);
        }

        // Resize our array if needed
        if payload.index as usize > MAX_FLASHBLOCK_INDEX {
            tracing::error!(
                target: "flashblocks::p2p",
                index = payload.index,
                max_index = MAX_FLASHBLOCK_INDEX,
                "Received flashblocks payload with index exceeding maximum"
            );
            return;
        }
        let len = state.flashblocks.len();
        state
            .flashblocks
            .resize_with(len.max(payload.index as usize + 1), || None);
        let flashblock = &mut state.flashblocks[payload.index as usize];

        // If we've already seen this index, skip it
        // Otherwise, add it to the list
        if flashblock.is_none() {
            // We haven't seen this index yet
            // Add the flashblock to our cache
            *flashblock = Some(payload.clone());
            tracing::trace!(
                target: "flashblocks::p2p",
                payload_id = %payload.payload_id,
                flashblock_index = payload.index,
                "queueing flashblock",
            );

            let p2p_msg = FlashblocksP2PMsg::Authorized(authorized_payload.authorized.clone());
            let bytes = p2p_msg.encode();
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

            let peer_msg =
                PeerMsg::FlashblocksPayloadV1((payload.payload_id, payload.index as usize, bytes));

            self.peer_tx.send(peer_msg).ok();
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
