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

pub trait FlashblocksP2PNetworHandle: Clone + Unpin + Peers + std::fmt::Debug + 'static {}

impl<N: Clone + Unpin + Peers + std::fmt::Debug + 'static> FlashblocksP2PNetworHandle for N {}

#[derive(Clone, Debug)]
pub enum PeerMsg {
    /// Send an already serialized flashblock to all peers.
    FlashblocksPayload((PayloadId, usize, BytesMut)),
    /// Send a previously serialized p2p message to all peers.
    Other(BytesMut),
}

#[derive(Clone, Debug)]
pub enum PublishingStatus {
    /// We are currently publishing flashblocks.
    Publishing { authorization: Authorization },
    /// We are waiting for the previous publisher to stop.
    WaitingToPublish {
        authorization: Authorization,
        /// The block number at which we requested to start publishing.
        wait_block_number: u64,
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

/// Protocol state is an helper struct to store the protocol events.
#[derive(Debug, Default)]
pub struct FlashblocksP2PState {
    pub publishing_status: PublishingStatus,
    /// Most recent payload id.
    pub payload_id: PayloadId,
    /// Block number of the most recent flashblocks payload.
    /// We only recieve the block bumber from `ExecutionPayloadBaseV1`
    /// so this may be set to `None` in the even that we receive the flashblocks payloads
    /// out of order.
    pub block_number: Option<u64>,
    /// Timestamp of the most recent flashblocks payload.
    pub payload_timestamp: u64,
    /// The index of the next flashblock to emit over the flashblocks_stream.
    pub flashblock_index: usize,
    /// Buffer of flashblocks for the current payload.
    pub flashblocks: Vec<Option<FlashblocksPayloadV1>>,
}

impl FlashblocksP2PState {
    /// Returns wether or now we are currently allowed to publish flashblocks.
    pub fn publishing_status(&self) -> PublishingStatus {
        self.publishing_status.clone()
    }
}

/// The protocol handler takes care of incoming and outgoing connections.
#[derive(Clone, Debug)]
pub struct FlashblocksP2PCtx<N> {
    /// Network handle, used to update peer state.
    pub network_handle: N,
    /// Authorizer verifying, used to verify flashblocks payloads.
    pub authorizer_vk: VerifyingKey,
    /// Builder signing key, used to sign authorized p2p messages.
    pub builder_sk: SigningKey,
    /// Sender for flashblocks payloads which will be broadcasted to all peers.
    /// May not be strictly ordered.
    pub peer_tx: broadcast::Sender<PeerMsg>,
    /// Receiver of verified and strictly ordered flashbloacks payloads.
    /// For consumption by the rpc overlay.
    pub flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
}

/// A cloneable protocol handler that takes care of incoming and outgoing connections.
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

    /// Returns the capability for the `flashblocks v1` p2p rotocol.
    pub fn capability() -> Capability {
        Capability::new_static("flblk", 1)
    }

    /// Publishes a newly created flashblock from the payload builder to the state and to the p2p network.
    /// Returns an error if we don't have clearance to publish flashblocks.
    /// You must call `start_publishing` on the current block before publishing any payloads for
    /// this block.
    ///
    /// TODO: We should eventually assert that flashblocks are consecutive have the correct parrent
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

    /// Returns the current publishing status.
    /// You must call `start_publishing` on the current block before this will return the current
    /// status.
    pub fn publishing_status(&self) -> PublishingStatus {
        self.state.lock().publishing_status.clone()
    }

    /// This should be called immediately after we receive a ForkChoiceUpdated
    /// with attributes, and the included Auhorization for each block. It's important to
    /// note that calling this does not guarantee that we will immediately have clearance
    /// to publish.
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
                wait_block_number,
                ..
            } => {
                // We are waiting to publish, so we update the authorization and
                // the block number at which we requested to start publishing.
                if new_block_number > *wait_block_number {
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
                // Send an authorized message to the network
                let authorized_msg = AuthorizedMsg::StartPublish(StartPublish {
                    block_number: new_block_number,
                });
                let authorized_payload =
                    Authorized::new(&self.ctx.builder_sk, new_authorization, authorized_msg);
                let p2p_msg = FlashblocksP2PMsg::Authorized(authorized_payload);
                let peer_msg = PeerMsg::Other(p2p_msg.encode());
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
                        wait_block_number: new_block_number,
                        active_publishers: active_publishers.clone(),
                    };
                }
            }
        }
    }

    /// Sends a message over the p2p network letting other peers know that we are stopping
    /// This should be called whenever we receive a ForkChoiceUpdated without attributes
    /// or without and `Authorization`.
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
                let peer_msg = PeerMsg::Other(p2p_msg.encode());
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
                let peer_msg = PeerMsg::Other(p2p_msg.encode());
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
    /// Commit new and already verified flashblocks payloads to the state
    /// broadcast them to peers, and publish them to the stream.
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
            state.flashblock_index = 0;
            state.payload_timestamp = authorization.timestamp;
            state.payload_id = authorization.payload_id;
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
                PeerMsg::FlashblocksPayload((payload.payload_id, payload.index as usize, bytes));

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
