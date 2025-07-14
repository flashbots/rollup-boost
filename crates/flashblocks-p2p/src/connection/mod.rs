use crate::protocol::{
    auth::Authorized,
    handler::{FlashblocksP2PNetworHandle, FlashblocksP2PState},
    proto::{FlashblocksProtoMessageId, FlashblocksProtoMessageKind},
};

use super::protocol::proto::FlashblocksProtoMessage;
use alloy_primitives::bytes::BytesMut;
use ed25519_dalek::VerifyingKey;
use futures::{Stream, StreamExt};
use parking_lot::{Mutex, RwLock};
use reth::payload::PayloadId;
use reth_ethereum::network::{api::PeerId, eth_wire::multiplex::ProtocolConnection};
use reth_network::types::ReputationChangeKind;
use rollup_boost::FlashblocksPayloadV1;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

pub(crate) mod handler;

pub struct FlashblocksConnection<N> {
    pub conn: ProtocolConnection,
    pub peer_id: PeerId,
    // inbound_tx: mpsc::UnboundedSender<FlashblocksProtoMessage>,
    pub network_handle: N,
    /// Authorizer verifying, used to verify flashblocks payloads.
    pub authorizer_vk: VerifyingKey,
    /// Sender for newly created or received and validated flashblocks payloads
    /// which will be broadcasted to all peers. May not be strictly ordered.
    pub peer_tx: broadcast::Sender<FlashblocksProtoMessage>,
    /// Receiver for newly created or received and validated flashblocks payloads
    /// which will be broadcasted to all peers. May not be strictly ordered.
    pub peer_rx: BroadcastStream<FlashblocksProtoMessage>,
    /// Channel of verified and strictly ordered flashbloacks payloads.
    /// For consumption by the rpc overlay.
    pub flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
    /// Mutable state of the flashblocks protocol.
    pub state: Arc<Mutex<FlashblocksP2PState>>,
    /// Most recent payload received from this peer.
    pub payload_id: PayloadId,
    /// A list of flashblocks indices that we have already received from
    /// this peer for the current payload.
    pub received: Vec<bool>,
}

impl<N: FlashblocksP2PNetworHandle> Stream for FlashblocksConnection<N> {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // Check if there are any flashblocks ready to broadcast to our peers.
            if let Poll::Ready(Some(res)) = this.peer_rx.poll_next_unpin(cx) {
                match res {
                    Ok(outbound) => {
                        return Poll::Ready(Some(outbound.encoded()));
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to receive flashblocks message from broadcast stream: {}",
                            e
                        );
                    }
                }
            }

            // Check if there are any messages from the peer.
            let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else {
                return Poll::Ready(None);
            };
            let Some(msg) = FlashblocksProtoMessage::decode_message(&mut &msg[..]) else {
                return Poll::Ready(None);
            };

            match msg.message {
                FlashblocksProtoMessageKind::FlashblocksPayloadV1(authorized) => {
                    this.handle_flashblocks_payload_v1(authorized, msg.message_type);
                }
            }
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksConnection<N> {
    fn handle_flashblocks_payload_v1(
        &mut self,
        message: Authorized<FlashblocksPayloadV1>,
        message_type: FlashblocksProtoMessageId,
    ) -> () {
        let mut state = self.state.lock();

        if let Err(e) = message.verify(self.authorizer_vk) {
            tracing::warn!(
                "Failed to verify flashblocks payload: {:?}, error: {}",
                message,
                e
            );
            self.network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        if message.authorization.timestamp < state.payload_timestamp {
            tracing::warn!(
                "Received flashblocks payload with outdated timestamp: {}",
                message.authorization.timestamp
            );
            self.network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        // Check if this is a new payload
        if message.authorization.timestamp > state.payload_timestamp {
            state.flashblock_index = 0;
            state.payload_timestamp = message.authorization.timestamp;
            state.payload_id = message.payload.payload_id;
            state.flashblocks.fill(None);
        }

        // Check if this peer is spamming us with the same payload
        if self.payload_id != message.payload.payload_id {
            self.payload_id = message.payload.payload_id;
            self.received.fill(false);
        }
        let len = self.received.len();
        self.received
            .resize_with(len.max(message.payload.index as usize + 1), || false);
        if self.received[message.payload.index as usize] {
            // We've already seen this index, skip it
            tracing::warn!(
                "Received duplicate flashblocks payload with id: {}, index: {}, from peer: {}",
                message.payload.payload_id,
                message.payload.index,
                self.peer_id
            );
            self.network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::AlreadySeenTransaction);
            return;
        }
        self.received[message.payload.index as usize] = true;

        // If we've already seen this index, skip it
        // Otherwise, add it to the list
        // TODO: perhaps check max index
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
            // Broadcast the flashblock to all peers, possibly out of order
            let message = FlashblocksProtoMessage {
                message: FlashblocksProtoMessageKind::FlashblocksPayloadV1(message),
                message_type,
            };
            self.peer_tx.send(message).unwrap();
            // Broadcast any flashblocks in the cache that are in order
            while let Some(Some(flashblock_event)) = state.flashblocks.get(state.flashblock_index) {
                // Send the flashblock to the stream
                self.flashblock_tx.send(flashblock_event.clone()).ok();
                // Update the index
                state.flashblock_index += 1;
            }
        }
    }
}
