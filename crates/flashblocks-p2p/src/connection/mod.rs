use crate::protocol::auth::Authorized;

use super::protocol::proto::{FlashblocksProtoMessage, FlashblocksProtoMessageKind};
use alloy_primitives::bytes::BytesMut;
use futures::{Stream, StreamExt};
use reth_ethereum::network::eth_wire::multiplex::ProtocolConnection;
use rollup_boost::FlashblocksPayloadV1;
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

pub(crate) mod handler;

/// We define some custom commands that the subprotocol supports.
pub(crate) enum FlashblocksCommand {
    /// Sends a flashblocks payload to the peer
    FlashblocksPayloadV1 {
        payload: Authorized<FlashblocksPayloadV1>,
    },
}

pub(crate) struct FlashblocksConnection {
    conn: ProtocolConnection,
    commands: UnboundedReceiverStream<FlashblocksCommand>,
}

impl Stream for FlashblocksConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                return match cmd {
                    FlashblocksCommand::FlashblocksPayloadV1 { payload } => Poll::Ready(Some(
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
                    // Process the received payload (could emit an event here)
                    // For now, we just continue to the next message
                    continue;
                }
            }

            return Poll::Pending;
        }
    }
}
