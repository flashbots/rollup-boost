use crate::protocol::handler::{
    FlashblocksHandler, FlashblocksP2PNetworHandle, PeerMsg, PublishingStatus,
};
use alloy_primitives::bytes::BytesMut;
use futures::{Stream, StreamExt};
use reth::payload::PayloadId;
use reth_ethereum::network::{api::PeerId, eth_wire::multiplex::ProtocolConnection};
use reth_network::types::ReputationChangeKind;
use rollup_boost::{
    Authorized, AuthorizedPayload, FlashblocksP2PMsg, FlashblocksPayloadV1, StartPublish,
    StopPublish,
};
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio_stream::wrappers::BroadcastStream;
use tracing::trace;

pub const INNITIATE_BUILD_TIMOUT: u64 = 8; // seconds

pub struct FlashblocksConnection<N> {
    pub handler: FlashblocksHandler<N>,
    pub conn: ProtocolConnection,
    pub peer_id: PeerId,
    /// Receiver for peer messages to be sent to all peers.
    /// We send bytes over this stream to avoid repeatedly having to serialize the payloads.
    pub peer_rx: BroadcastStream<PeerMsg>,
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
                    Ok(peer_msg) => {
                        match peer_msg {
                            PeerMsg::FlashblocksPayloadV1((
                                payload_id,
                                flashblock_index,
                                bytes,
                            )) => {
                                // Check if this flashblock actually originated from this peer.
                                if this.payload_id != payload_id
                                    || this.received.get(flashblock_index) != Some(&true)
                                {
                                    trace!(
                                        target: "flashblocks::p2p",
                                        peer_id = %this.peer_id,
                                        %payload_id,
                                        %flashblock_index,
                                        "Broadcasting `FlashblocksPayloadV1` message to peer"
                                    );
                                    return Poll::Ready(Some(bytes));
                                }
                            }
                            PeerMsg::StartPublishing(bytes_mut) => {
                                trace!(
                                    target: "flashblocks::p2p",
                                    peer_id = %this.peer_id,
                                    "Broadcasting `StartPublishing` to peer"
                                );
                                return Poll::Ready(Some(bytes_mut));
                            }
                            PeerMsg::StopPublishing(bytes_mut) => {
                                trace!(
                                    target: "flashblocks::p2p",
                                    peer_id = %this.peer_id,
                                    "Broadcasting `StopPublishing` to peer"
                                );
                                return Poll::Ready(Some(bytes_mut));
                            }
                        }
                    }
                    Err(error) => {
                        tracing::error!(
                            target: "flashblocks::p2p",
                            %error,
                            "failed to receive flashblocks message from peer_rx"
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
                        target: "flashblocks::p2p",
                        peer_id = %this.peer_id,
                        %error,
                        "failed to decode flashblocks message from peer",
                    );
                    this.handler
                        .ctx
                        .network_handle
                        .reputation_change(this.peer_id, ReputationChangeKind::BadMessage);
                    return Poll::Ready(None);
                }
            };

            match msg {
                FlashblocksP2PMsg::Authorized(authorized) => {
                    if authorized.authorization.builder_vk
                        == this.handler.ctx.builder_sk.verifying_key()
                    {
                        tracing::warn!(
                            target: "flashblocks::p2p",
                            peer_id = %this.peer_id,
                            "received our own message from peer",
                        );
                        this.handler
                            .ctx
                            .network_handle
                            .reputation_change(this.peer_id, ReputationChangeKind::BadMessage);
                        continue;
                    }

                    if let Err(error) = authorized.verify(this.handler.ctx.authorizer_vk) {
                        tracing::warn!(
                            target: "flashblocks::p2p",
                            peer_id = %this.peer_id,
                            %error,
                            "failed to verify flashblock",
                        );
                        this.handler
                            .ctx
                            .network_handle
                            .reputation_change(this.peer_id, ReputationChangeKind::BadMessage);
                        continue;
                    }

                    match &authorized.msg {
                        rollup_boost::AuthorizedMsg::FlashblocksPayloadV1(_) => {
                            this.handle_flashblocks_payload_v1(authorized.into_unchecked());
                        }
                        rollup_boost::AuthorizedMsg::StartPublish(_) => {
                            this.handle_start_publish(authorized.into_unchecked());
                        }
                        rollup_boost::AuthorizedMsg::StopPublish(_) => {
                            this.handle_stop_publish(authorized.into_unchecked());
                        }
                    }
                }
            }
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksConnection<N> {
    fn handle_flashblocks_payload_v1(
        &mut self,
        authorized_payload: AuthorizedPayload<FlashblocksPayloadV1>,
    ) {
        let mut state = self.handler.state.lock();
        let authorization = &authorized_payload.authorized.authorization;
        let msg = authorized_payload.msg();

        // check if this is an old payload
        if authorization.timestamp < state.payload_timestamp {
            tracing::warn!(
                target: "flashblocks::p2p",
                peer_id = %self.peer_id,
                timestamp = authorization.timestamp,
                "received flashblock with outdated timestamp",
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        // Check if this is a new payload from this peer
        if self.payload_id != msg.payload_id {
            self.payload_id = msg.payload_id;
            self.received.fill(false);
        }

        // Check if this peer is spamming us with the same payload index
        let len = self.received.len();
        self.received
            .resize_with(len.max(msg.index as usize + 1), || false);
        if self.received[msg.index as usize] {
            // We've already seen this index from this peer.
            // They could be trying to DOS us.
            tracing::warn!(
                target: "flashblocks::p2p",
                peer_id = %self.peer_id,
                payload_id = %msg.payload_id,
                index = msg.index,
                "received duplicate flashblock from peer",
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::AlreadySeenTransaction);
            return;
        }
        self.received[msg.index as usize] = true;

        let active_publishers = match &mut state.publishing_status {
            PublishingStatus::Publishing { .. } => {
                // We are currently building, so we should not be seeing any new flashblocks
                // over the p2p network.
                tracing::error!(
                    target: "flashblocks::p2p",
                    peer_id = %self.peer_id,
                    "received flashblock while already building",
                );
                return;
            }
            PublishingStatus::WaitingToPublish {
                active_publishers, ..
            } => active_publishers,
            PublishingStatus::NotPublishing { active_publishers } => active_publishers,
        };

        // Update the list of active publishers
        if let Some(base) = &msg.base {
            if let Some((_, block_number)) = active_publishers
                .iter_mut()
                .find(|(publisher, _)| *publisher == authorization.builder_vk)
            {
                // This is an existing publisher, we should update their block number
                *block_number = base.block_number;
            } else {
                // This is a new publisher, we should add them to the list of active publishers
                active_publishers.push((authorization.builder_vk, base.block_number));
            }
        }

        self.handler.ctx.publish(&mut state, authorized_payload);
    }

    // TODO: Handle replay attacks with StartPublish messages.
    // TODO: handle propogating this if we care. For now we assume direct peering.
    fn handle_start_publish(&mut self, authorized_payload: AuthorizedPayload<StartPublish>) {
        let mut state = self.handler.state.lock();
        let authorization = &authorized_payload.authorized.authorization;
        let msg = authorized_payload.msg();

        // Check if the request is expired for dos protection
        if state.payload_timestamp
            > authorized_payload.authorized.authorization.timestamp + INNITIATE_BUILD_TIMOUT
        {
            tracing::warn!(
                target: "flashblocks::p2p",
                peer_id = %self.peer_id,
                current_timestamp = state.payload_timestamp,
                timestamp = authorized_payload.authorized.authorization.timestamp,
                "received initiate build request with outdated timestamp",
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        let active_publishers = match &mut state.publishing_status {
            PublishingStatus::Publishing { authorization } => {
                tracing::info!(
                    target: "flashblocks::p2p",
                    peer_id = %self.peer_id,
                    "Received StartPublish over p2p, stopping publishing flashblocks"
                );

                let authorized = Authorized::new(
                    &self.handler.ctx.builder_sk,
                    *authorization,
                    StopPublish.into(),
                );
                let p2p_msg = FlashblocksP2PMsg::Authorized(authorized);
                let peer_msg = PeerMsg::StopPublishing(p2p_msg.encode());
                self.handler.ctx.peer_tx.send(peer_msg).ok();

                state.publishing_status = PublishingStatus::NotPublishing {
                    active_publishers: vec![(authorization.builder_vk, msg.block_number)],
                };

                return;
            }
            PublishingStatus::WaitingToPublish {
                active_publishers, ..
            } => {
                // We are currently waiting to build, but someone else is requesting to build
                // This could happen during a double failover.
                // We have a potential race condition here so we'll just wait for the
                // build request override to kick in next block.
                tracing::warn!(
                    target: "flashblocks::p2p",
                    peer_id = %self.peer_id,
                    "Received StartPublish over p2p while already waiting to publish, ignoring",
                );
                active_publishers
            }
            PublishingStatus::NotPublishing { active_publishers } => active_publishers,
        };

        if let Some((_, block_number)) = active_publishers
            .iter_mut()
            .find(|(publisher, _)| *publisher == authorization.builder_vk)
        {
            // This is an existing publisher, we should update their block number
            *block_number = msg.block_number;
        } else {
            // This is a new publisher, we should add them to the list of active publishers
            active_publishers.push((authorization.builder_vk, msg.block_number));
        }
    }

    // TODO: Handle replay attacks with StopPublish messages.
    // TODO: handle propogating this if we care. For now we assume direct peering.
    fn handle_stop_publish(&mut self, authorized_payload: AuthorizedPayload<StopPublish>) {
        let mut state = self.handler.state.lock();
        if state.payload_timestamp
            > authorized_payload.authorized.authorization.timestamp + INNITIATE_BUILD_TIMOUT
        {
            tracing::warn!(
                target: "flashblocks::p2p",
                peer_id = %self.peer_id,
                current_timestamp = state.payload_timestamp,
                timestamp = authorized_payload.authorized.authorization.timestamp,
                "Received initiate build response with outdated timestamp",
            );
            self.handler
                .ctx
                .network_handle
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        match &mut state.publishing_status {
            PublishingStatus::Publishing { .. } => {
                tracing::warn!(
                    target: "flashblocks::p2p",
                    peer_id = %self.peer_id,
                    "Received StopPublish over p2p while we are the publisher"
                );
            }
            PublishingStatus::WaitingToPublish {
                active_publishers,
                authorization,
                ..
            } => {
                // We are currently waiting to build, but someone else is requesting to build
                // This could happen during a double failover.
                // We have a potential race condition here so we'll just wait for the
                // build request override to kick in next block.
                tracing::info!(
                    target: "flashblocks::p2p",
                    peer_id = %self.peer_id,
                    "Received StopPublish over p2p while waiting to publish",
                );

                // Remove the publisher from the list of active publishers
                if let Some(index) = active_publishers.iter().position(|(publisher, _)| {
                    *publisher == authorized_payload.authorized.authorization.builder_vk
                }) {
                    active_publishers.remove(index);
                } else {
                    tracing::warn!(
                        target: "flashblocks::p2p",
                        peer_id = %self.peer_id,
                        "Received StopPublish for unknown publisher",
                    );
                }

                if active_publishers.is_empty() {
                    // If there are no active publishers left, we should stop waiting to publish
                    tracing::info!(
                        target: "flashblocks::p2p",
                        peer_id = %self.peer_id,
                        "starting to publish"
                    );
                    state.publishing_status = PublishingStatus::Publishing {
                        authorization: *authorization,
                    };
                } else {
                    tracing::info!(
                        target: "flashblocks::p2p",
                        peer_id = %self.peer_id,
                        "still waiting on active publishers",
                    );
                }
            }
            PublishingStatus::NotPublishing { active_publishers } => {
                // Remove the publisher from the list of active publishers
                if let Some(index) = active_publishers.iter().position(|(publisher, _)| {
                    *publisher == authorized_payload.authorized.authorization.builder_vk
                }) {
                    active_publishers.remove(index);
                } else {
                    tracing::warn!(
                        target: "flashblocks::p2p",
                        peer_id = %self.peer_id,
                        "Received StopPublish for unknown publisher",
                    );
                }
            }
        }
    }
}
