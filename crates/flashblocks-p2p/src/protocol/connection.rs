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
    /// We send bytes over this stream to avoid repeatedly having to serialize the payloads.
    pub peer_rx: BroadcastStream<(PayloadId, usize, BytesMut)>,
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
                    Ok((payload_id, flashblock_index, bytes)) => {
                        // Check if this flashblock actually originated from this peer.
                        if this.payload_id != payload_id
                            || this.received.get(flashblock_index) != Some(&true)
                        {
                            trace!(
                                target: "flashblocks",
                                peer_id = %this.peer_id,
                                %payload_id,
                                %flashblock_index,
                                "Broadcasting flashblock message to peer"
                            );
                            return Poll::Ready(Some(bytes));
                        }
                    }
                    Err(error) => {
                        tracing::error!(
                            target: "flashblocks",
                            %error,
                            "Failed to receive flashblocks message from peer_rx"
                        );
                    }
                }
            }

            // Check if there are any messages from the peer.
            let Some(buf) = ready!(this.conn.poll_next_unpin(cx)) else {
                return Poll::Ready(None);
            };

            let msg = match FlashblocksP2PMsg::decode(&mut &buf[..]) {
                Ok(msg) => msg,
                Err(error) => {
                    tracing::warn!(
                        target: "flashblocks",
                        peer_id = %this.peer_id,
                        %error,
                        "Failed to decode flashblocks message from peer",
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

        if let Err(error) = message.verify(self.handler.ctx.authorizer_vk) {
            tracing::warn!(
                target: "flashblocks",
                peer_id = %self.peer_id,
                %error,
                "Failed to verify flashblock",
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        if message.authorization.timestamp < state.payload_timestamp {
            tracing::warn!(
                target: "flashblocks",
                peer_id = %self.peer_id,
                timestamp = message.authorization.timestamp,
                "Received flashblock with outdated timestamp",
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        // Check if this is a globally new payload
        if message.authorization.timestamp > state.payload_timestamp {
            state.flashblock_index = 0;
            state.payload_timestamp = message.authorization.timestamp;
            state.payload_id = message.payload.payload_id;
            state.flashblocks.fill(None);
        }

        // Check if this is a new payload from this peer
        if self.payload_id != message.payload.payload_id {
            self.payload_id = message.payload.payload_id;
            self.received.fill(false);
        }

        // Check if this peer is spamming us with the same payload index
        let len = self.received.len();
        self.received
            .resize_with(len.max(message.payload.index as usize + 1), || false);
        if self.received[message.payload.index as usize] {
            // We've already seen this index from this peer.
            // They could be trying to DOS us.
            tracing::warn!(
                target: "flashblocks",
                peer_id = %self.peer_id,
                payload_id = %message.payload.payload_id,
                index = message.payload.index,
                "Received duplicate flashblock from peer",
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
