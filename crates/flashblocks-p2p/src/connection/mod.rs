use super::protocol::proto::{FlashblocksProtoMessage, FlashblocksProtoMessageKind};
use alloy_primitives::bytes::BytesMut;
use futures::{Stream, StreamExt};
use reth_ethereum::network::{api::PeerId, eth_wire::multiplex::ProtocolConnection};
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::BroadcastStream;

pub(crate) mod handler;

/// We define some custom commands that the subprotocol supports.
pub struct IncomingPeerMessage {
    peer_id: PeerId,
    msg: FlashblocksProtoMessageKind,
}

pub struct FlashblocksConnection<N> {
    conn: ProtocolConnection,
    peer_id: PeerId,
    inbound_tx: mpsc::UnboundedSender<FlashblocksProtoMessage>,
    outbound_rx: BroadcastStream<FlashblocksProtoMessage>,
    network_handle: N,
}

impl<N: Unpin> Stream for FlashblocksConnection<N> {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // Check if there are any flashblocks ready to broadcast to our peers.
            if let Poll::Ready(Some(res)) = this.outbound_rx.poll_next_unpin(cx) {
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
            this.inbound_tx
                .send(msg.clone())
                .map_err(|e| {
                    tracing::error!(
                        "Failed to send flashblocks message to inbound channel: {}",
                        e
                    )
                })
                .ok();
        }
    }
}
