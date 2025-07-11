use crate::connection::handler::FlashblocksConnectionHandler;
use crate::protocol::proto::FlashblocksProtoMessage;
use crate::protocol::proto::FlashblocksProtoMessageKind;
use ed25519_dalek::VerifyingKey;
use reth::payload::PayloadId;
use reth_ethereum::network::{api::PeerId, protocol::ProtocolHandler};
use reth_network::Peers;
use rollup_boost::FlashblocksPayloadV1;
use std::net::SocketAddr;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

pub(crate) trait FlashblocksP2PNetworHandle:
    Clone + Unpin + Peers + std::fmt::Debug + 'static
{
}

impl<N: Clone + Unpin + Peers + std::fmt::Debug + 'static> FlashblocksP2PNetworHandle for N {}

/// Protocol state is an helper struct to store the protocol events.
#[derive(Debug)]
pub struct FlashblocksP2PState<N: Peers + std::fmt::Debug> {
    /// Networkd handle, used to update peer state.
    pub network_handle: N,
    /// Authorizer verifying, used to verify flashblocks payloads.
    pub authorizer_vk: VerifyingKey,
    /// Sender of verified and strictly ordered flashbloacks payloads.
    /// For consumption by the rpc overlay.
    pub flashblock_stream: broadcast::Sender<FlashblocksPayloadV1>,
    /// Verified flashblock payloads received by peers.
    /// May not be strictly ordered.
    pub inbound_rx: mpsc::UnboundedReceiver<FlashblocksProtoMessage>,
    /// Sender for newly received and validated flashblocks payloads
    /// which will be broadcasted to all peers. May not be strictly ordered.
    pub outbound_tx: broadcast::Sender<FlashblocksProtoMessage>,
    /// The index of the next flashblock to emit.
    pub flashblock_index: usize,
    /// Timestamp of the most recent flashblocks payload.
    pub payload_timestamp: u64,
    /// Most recent payload id.
    pub payload_id: PayloadId,
    /// Buffer of flashblocks for the current payload.
    pub flashblocks: Vec<Option<FlashblocksPayloadV1>>,
}

impl<N: Peers + std::fmt::Debug> FlashblocksP2PState<N> {
    pub fn run(mut self) {
        tokio::spawn(async move {
            while let Some(msg) = self.inbound_rx.recv().await {
                match &msg.message {
                    FlashblocksProtoMessageKind::FlashblocksPayloadV1(payload) => {
                        // TODO: might make sense to perform verification in a separate task
                        if let Err(e) = payload.verify(self.authorizer_vk) {
                            tracing::warn!(
                                "Failed to verify flashblocks payload: {:?}, error: {}",
                                payload,
                                e
                            );
                            // TODO: ban peer
                            continue;
                        }
                        if payload.authorization.timestamp < self.payload_timestamp {
                            tracing::warn!(
                                "Received flashblocks payload with outdated timestamp: {}",
                                payload.authorization.timestamp
                            );
                            // TODO: handle peer
                            continue;
                        }
                        // Check if this is a new payload
                        if payload.authorization.timestamp > self.payload_timestamp {
                            self.flashblock_index = 0;
                            self.payload_timestamp = payload.authorization.timestamp;
                            self.payload_id = payload.payload.payload_id;
                            self.flashblocks.clear();
                        }
                        // If we've already seen this index, skip it
                        // Otherwise, add it to the list
                        // TODO: perhaps check max index
                        self.flashblocks
                            .resize_with(payload.payload.index as usize + 1, || None);
                        let flashblock = &mut self.flashblocks[payload.payload.index as usize];
                        if flashblock.is_none() {
                            // We haven't seen this index yet
                            // Add the flashblock to our cache
                            *flashblock = Some(payload.clone().payload);
                            tracing::debug!(
                                "Received flashblocks payload with id: {}, index: {}",
                                payload.payload.payload_id,
                                payload.payload.index
                            );
                            // Broadcast the flashblock to all peers, possible our of order
                            self.outbound_tx.send(msg).unwrap();
                            // Broadcast any flashblocks in the cache that are in order
                            while let Some(Some(flashblock_event)) =
                                self.flashblocks.get(self.flashblock_index)
                            {
                                // Send the flashblock to the stream
                                self.flashblock_stream.send(flashblock_event.clone()).ok();
                                // Update the index
                                self.flashblock_index += 1;
                            }
                        }
                    }
                }
            }
        });
    }
}

/// The protocol handler takes care of incoming and outgoing connections.
#[derive(Debug)]
pub struct FlashblocksProtoHandler {
    // /// Sender of verified and strictly ordered flashbloacks payloads.
    // /// For consumption by the rpc overlay.
    // pub flashblock_stream: broadcast::Sender<FlashblocksPayloadV1>,
    /// Sender for newly received and validated flashblocks payloads
    /// which will be broadcasted to all peers. May not be strictly ordered.
    pub outbound_rx: broadcast::Receiver<FlashblocksProtoMessage>,
    /// Verified flashblock payloads received by peers.
    /// May not be strictly ordered.
    pub inbound_tx: mpsc::UnboundedSender<FlashblocksProtoMessage>,
}

impl FlashblocksProtoHandler {
    /// Creates a new protocol handler with the given state.
    pub fn new<N: Peers + std::fmt::Debug>(
        network_handle: N,
        authorizer_vk: VerifyingKey,
        flashblock_stream: broadcast::Sender<FlashblocksPayloadV1>,
    ) -> Self {
        let (outbound_tx, outbound_rx) = broadcast::channel(100);
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let state = FlashblocksP2PState {
            network_handle,
            authorizer_vk,
            flashblock_stream,
            inbound_rx,
            outbound_tx,
            flashblock_index: 0,
            payload_timestamp: 0,
            payload_id: PayloadId::default(),
            flashblocks: vec![],
        };
        state.run();

        Self {
            // flashblock_stream,
            outbound_rx,
            inbound_tx,
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> ProtocolHandler for FlashblocksProtoHandler<N> {
    type ConnectionHandler = FlashblocksConnectionHandler<N>;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(FlashblocksConnectionHandler::<N> {
            network_handle: self.network_handle.clone(),
        })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(FlashblocksConnectionHandler::<N> {
            network_handle: self.network_handle.clone(),
        })
    }
}
