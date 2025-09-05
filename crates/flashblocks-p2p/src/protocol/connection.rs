use crate::protocol::handler::{
    FlashblocksP2PNetworkHandle, FlashblocksP2PProtocol, MAX_FLASHBLOCK_INDEX, P2PClient, PeerMsg,
    PublishingStatus,
};
use alloy_primitives::bytes::BytesMut;
use futures::{Stream, StreamExt};
use metrics::gauge;
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

/// Represents a single P2P connection for the flashblocks protocol.
///
/// This struct manages the bidirectional communication with a single peer in the flashblocks
/// P2P network. It handles incoming messages from the peer, validates and processes them,
/// and also streams outgoing messages that need to be broadcast.
///
/// The connection implements the `Stream` trait to provide outgoing message bytes that
/// should be sent to the connected peer over the underlying protocol connection.
pub struct FlashblocksConnection<N, C> {
    /// The flashblocks protocol handler that manages the overall protocol state.
    protocol: FlashblocksP2PProtocol<N, C>,
    /// The underlying protocol connection for sending and receiving raw bytes.
    conn: ProtocolConnection,
    /// The unique identifier of the connected peer.
    peer_id: PeerId,
    /// Receiver for peer messages to be sent to all peers.
    /// We send bytes over this stream to avoid repeatedly having to serialize the payloads.
    peer_rx: BroadcastStream<PeerMsg>,
    /// Most recent payload ID received from this peer to track payload transitions.
    payload_id: PayloadId,
    /// A list of flashblock indices that we have already received from
    /// this peer for the current payload, used to detect duplicate messages.
    received: Vec<bool>,
}

impl<N: FlashblocksP2PNetworkHandle, C> FlashblocksConnection<N, C> {
    /// Creates a new `FlashblocksConnection` instance.
    ///
    /// # Arguments
    /// * `protocol` - The flashblocks protocol handler managing the connection.
    /// * `conn` - The underlying protocol connection for sending and receiving messages.
    /// * `peer_id` - The unique identifier of the connected peer.
    /// * `peer_rx` - Receiver for peer messages to be sent to all peers.
    pub fn new(
        protocol: FlashblocksP2PProtocol<N, C>,
        conn: ProtocolConnection,
        peer_id: PeerId,
        peer_rx: BroadcastStream<PeerMsg>,
    ) -> Self {
        gauge!("p2p.flashblocks_peers", "capability" => FlashblocksP2PProtocol::<N, C>::capability().to_string()).increment(1);

        Self {
            protocol,
            conn,
            peer_id,
            peer_rx,
            payload_id: PayloadId::default(),
            received: Vec::new(),
        }
    }
}

impl<N, C> Drop for FlashblocksConnection<N, C> {
    fn drop(&mut self) {
        gauge!("p2p.flashblocks_peers", "capability" => FlashblocksP2PProtocol::<N, C>::capability().to_string()).decrement(1);
    }
}

impl<N: FlashblocksP2PNetworkHandle, C> Stream for FlashblocksConnection<N, C>
where
    C: P2PClient,
{
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
                    this.protocol
                        .network
                        .reputation_change(this.peer_id, ReputationChangeKind::BadMessage);
                    return Poll::Ready(None);
                }
            };

            match msg {
                FlashblocksP2PMsg::Authorized(authorized) => {
                    if Some(authorized.authorization.builder_vk)
                        == this
                            .protocol
                            .handle
                            .ctx
                            .client
                            .builder_sk()
                            .map(|s| s.verifying_key())
                    {
                        tracing::warn!(
                            target: "flashblocks::p2p",
                            peer_id = %this.peer_id,
                            "received our own message from peer",
                        );
                        this.protocol
                            .network
                            .reputation_change(this.peer_id, ReputationChangeKind::BadMessage);
                        continue;
                    }

                    if let Err(error) = authorized.verify(this.protocol.handle.ctx.authorizer_vk) {
                        tracing::warn!(
                            target: "flashblocks::p2p",
                            peer_id = %this.peer_id,
                            %error,
                            "failed to verify flashblock",
                        );
                        this.protocol
                            .network
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

impl<N: FlashblocksP2PNetworkHandle, C: P2PClient> FlashblocksConnection<N, C> {
    /// Handles incoming flashblock payload messages from a peer.
    ///
    /// This method validates the flashblock payload, checks for duplicates and ordering,
    /// updates the active publisher tracking, and forwards valid payloads for processing.
    /// It also manages peer reputation based on message validity and prevents spam attacks.
    ///
    /// # Arguments
    /// * `authorized_payload` - The authorized flashblock payload received from the peer
    ///
    /// # Behavior
    /// - Validates timestamp to prevent replay attacks
    /// - Tracks payload transitions and resets duplicate detection
    /// - Prevents duplicate flashblock spam from the same peer
    /// - Updates active publisher information from base payload data
    /// - Forwards valid payloads to the protocol handler for processing
    fn handle_flashblocks_payload_v1(
        &mut self,
        authorized_payload: AuthorizedPayload<FlashblocksPayloadV1>,
    ) {
        let mut state = self.protocol.handle.state.lock();
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
            self.protocol
                .network
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        // Check if this is a new payload from this peer
        if self.payload_id != msg.payload_id {
            self.payload_id = msg.payload_id;
            self.received.fill(false);
        }

        // Check if the payload index is within the allowed range
        if msg.index as usize > MAX_FLASHBLOCK_INDEX {
            tracing::error!(
                target: "flashblocks::p2p",
                peer_id = %self.peer_id,
                index = msg.index,
                payload_id = %msg.payload_id,
                max_index = MAX_FLASHBLOCK_INDEX,
                "Received flashblocks payload with index exceeding maximum"
            );
            return;
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
            self.protocol
                .network
                .reputation_change(self.peer_id, ReputationChangeKind::AlreadySeenTransaction);
            return;
        }
        self.received[msg.index as usize] = true;

        state.publishing_status.send_modify(|status| {
            let active_publishers = match status {
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
            if let Some((_, timestamp)) = active_publishers
                .iter_mut()
                .find(|(publisher, _)| *publisher == authorization.builder_vk)
            {
                // This is an existing publisher, we should update their block number
                *timestamp = authorization.timestamp;
            } else {
                // This is a new publisher, we should add them to the list of active publishers
                active_publishers.push((authorization.builder_vk, authorization.timestamp));
            }
        });

        self.protocol
            .handle
            .ctx
            .publish(&mut state, authorized_payload);
    }

    /// Handles incoming `StartPublish` messages from a peer.
    ///
    /// TODO: handle propogating this if we care. For now we assume direct peering.
    ///
    /// # Arguments
    /// * `authorized_payload` - The authorized `StartPublish` message received from the peer
    ///
    /// # Behavior
    /// - Validates the timestamp to prevent replay attacks
    /// - Updates the publishing status to reflect the new publisher
    /// - If we are currently publishing, sends a `StopPublish` message to ourselves
    /// - If we are waiting to publish, updates the list of active publishers
    /// - If we are not publishing, adds the new publisher to the list of active publishers
    fn handle_start_publish(&mut self, authorized_payload: AuthorizedPayload<StartPublish>) {
        let state = self.protocol.handle.state.lock();
        let authorization = &authorized_payload.authorized.authorization;
        let Some(builder_sk) = self.protocol.handle.ctx.client.builder_sk() else {
            // We're not publishers so we can ignore
            return;
        };

        // Check if the request is expired for dos protection.
        // It's important to ensure that this `StartPublish` request
        // is very recent, or it could be used in a replay attack.
        if state.payload_timestamp > authorization.timestamp {
            tracing::warn!(
                target: "flashblocks::p2p",
                peer_id = %self.peer_id,
                current_timestamp = state.payload_timestamp,
                timestamp = authorized_payload.authorized.authorization.timestamp,
                "received initiate build request with outdated timestamp",
            );
            self.protocol
                .network
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        state.publishing_status.send_modify(|status| {
            let active_publishers = match status {
                PublishingStatus::Publishing {
                    authorization: our_authorization,
                } => {
                    tracing::info!(
                        target: "flashblocks::p2p",
                        peer_id = %self.peer_id,
                        "Received StartPublish over p2p, stopping publishing flashblocks"
                    );

                    let authorized =
                        Authorized::new(builder_sk, *our_authorization, StopPublish.into());
                    let p2p_msg = FlashblocksP2PMsg::Authorized(authorized);
                    let peer_msg = PeerMsg::StopPublishing(p2p_msg.encode());
                    self.protocol.handle.ctx.peer_tx.send(peer_msg).ok();

                    *status = PublishingStatus::NotPublishing {
                        active_publishers: vec![(
                            our_authorization.builder_vk,
                            authorization.timestamp,
                        )],
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

            if let Some((_, timestamp)) = active_publishers
                .iter_mut()
                .find(|(publisher, _)| *publisher == authorization.builder_vk)
            {
                // This is an existing publisher, we should update their block number
                *timestamp = authorization.timestamp;
            } else {
                // This is a new publisher, we should add them to the list of active publishers
                active_publishers.push((authorization.builder_vk, authorization.timestamp));
            }
        });
    }

    /// Handles incoming `StopPublish` messages from a peer.
    ///
    /// TODO: handle propogating this if we care. For now we assume direct peering.
    ///
    /// # Arguments
    /// * `authorized_payload` - The authorized `StopPublish` message received from the peer
    ///
    /// # Behavior
    /// - Validates the timestamp to prevent replay attacks
    /// - Updates the publishing status based on the current state
    /// - If we are currently publishing, logs a warning
    /// - If we are waiting to publish, removes the publisher from the list of active publishers and checks if we can start publishing
    /// - If we are not publishing, removes the publisher from the list of active publishers
    fn handle_stop_publish(&mut self, authorized_payload: AuthorizedPayload<StopPublish>) {
        let state = self.protocol.handle.state.lock();
        let authorization = &authorized_payload.authorized.authorization;

        // Check if the request is expired for dos protection.
        // It's important to ensure that this `StartPublish` request
        // is very recent, or it could be used in a replay attack.
        if state.payload_timestamp > authorization.timestamp {
            tracing::warn!(
                target: "flashblocks::p2p",
                peer_id = %self.peer_id,
                current_timestamp = state.payload_timestamp,
                timestamp = authorized_payload.authorized.authorization.timestamp,
                "Received initiate build response with outdated timestamp",
            );
            self.protocol
                .network
                .reputation_change(self.peer_id, ReputationChangeKind::BadMessage);
            return;
        }

        state.publishing_status.send_modify(|status| {
            match status {
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
                        *status = PublishingStatus::Publishing {
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
        });
    }
}
