use crate::protocol::handler::{FlashblocksHandler, FlashblocksP2PNetworHandle};
use alloy_primitives::bytes::BytesMut;
use futures::{Stream, StreamExt};
use reth::payload::PayloadId;
use reth_ethereum::network::{api::PeerId, eth_wire::multiplex::ProtocolConnection};
use reth_network::types::ReputationChangeKind;
use rollup_boost::{Authorized, FlashblocksP2PMsg, FlashblocksPayloadV1};
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio_stream::wrappers::BroadcastStream;
use tracing::trace;

pub struct FlashblocksConnection<N> {
    pub handler: FlashblocksHandler<N>,
    pub conn: ProtocolConnection,
    pub peer_id: PeerId,
    /// Receiver for newly created or received and validated flashblocks payloads
    /// which will be broadcasted to all peers. May not be strictly ordered.
    pub peer_rx: BroadcastStream<BytesMut>,
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
                    Ok(bytes) => {
                        // TODO: handle the case where this peer is the one that sent the original
                        trace!(peer_id = %this.peer_id, target = "flashblocks", "Broadcasting flashblocks message");
                        return Poll::Ready(Some(bytes));
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
            let Some(buf) = ready!(this.conn.poll_next_unpin(cx)) else {
                return Poll::Ready(None);
            };

            // TODO: handle max buffer size
            let msg = match FlashblocksP2PMsg::decode(&mut &buf[..]) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::warn!(
                        "Failed to decode flashblocks message from peer {}: {}",
                        this.peer_id,
                        e
                    );
                    this.handler
                        .ctx
                        .network_handle
                        .reputation_change(this.peer_id, ReputationChangeKind::BadMessage);
                    return Poll::Ready(None);
                }
            };

            match msg {
                FlashblocksP2PMsg::FlashblocksPayloadV1(authorized) => {
                    this.handle_flashblocks_payload_v1(authorized);
                }
            }
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksConnection<N> {
    fn handle_flashblocks_payload_v1(&mut self, message: Authorized<FlashblocksPayloadV1>) {
        let mut state = self.handler.state.lock();

        if let Err(e) = message.verify(self.handler.ctx.authorizer_vk) {
            tracing::warn!(
                "Failed to verify flashblocks payload: {:?}, error: {}",
                message,
                e
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        if message.authorization.timestamp < state.payload_timestamp {
            tracing::warn!(
                "Received flashblocks payload with outdated timestamp: {}",
                message.authorization.timestamp
            );
            self.handler
                .ctx
                .network_handle
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
            // We've already seen this index from this peer.
            // They could be trying to DOS us.
            tracing::warn!(
                "Received duplicate flashblocks payload with id: {}, index: {}, from peer: {}",
                message.payload.payload_id,
                message.payload.index,
                self.peer_id
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::AlreadySeenTransaction);
            return;
        }
        self.received[message.payload.index as usize] = true;

        self.handler
            .ctx
            .publish(&mut state, FlashblocksP2PMsg::FlashblocksPayloadV1(message));
    }
}
