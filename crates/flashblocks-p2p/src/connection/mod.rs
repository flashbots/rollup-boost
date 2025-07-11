use crate::protocol::{auth::Authorized, handler::FlashblocksP2PState};

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
            if let Poll::Ready(Some(cmd)) = this.outbound_rx.poll_next_unpin(cx) {
                return match cmd {
                    FlashblocksIncoming::FlashblocksPayloadV1 { payload } => Poll::Ready(Some(
                        FlashblocksProtoMessage::flashblocks_payload(payload).encoded(),
                    )),
                };
            }

            let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else {
                return Poll::Ready(None);
            };

            let Some(msg) = FlashblocksProtoMessage::decode_message(&mut &msg[..]) else {
                return Poll::Ready(None);
            };

            match msg.message {
                FlashblocksProtoMessageKind::FlashblocksPayloadV1(payload) => {
                    this.state
                        .flashblock_stream
                        .send(FlashblocksP2PEvent::FlashblocksPayloadV1(payload))
                        .ok();
                }
            }
        }
    }
}
