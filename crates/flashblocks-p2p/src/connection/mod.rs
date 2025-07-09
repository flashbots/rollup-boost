use super::protocol::proto::{FlashblocksProtoMessage, FlashblocksProtoMessageKind};
use alloy_primitives::bytes::BytesMut;
use futures::{Stream, StreamExt};
use reth_ethereum::network::eth_wire::multiplex::ProtocolConnection;
use rollup_boost::FlashblocksPayloadV1;
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;

pub(crate) mod handler;

/// We define some custom commands that the subprotocol supports.
pub(crate) enum FlashblocksCommand {
    /// Sends a message to the peer
    NewFlashBlock {
        msg: String,
        /// The response will be sent to this channel.
        response: oneshot::Sender<String>,
    },
}

pub(crate) struct FlashblocksConnection {
    conn: ProtocolConnection,
    initial_ping: Option<FlashblocksProtoMessage>,
    commands: UnboundedReceiverStream<FlashblocksCommand>,
    pending_pong: Option<oneshot::Sender<String>>,
}

impl Stream for FlashblocksConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(initial_ping) = this.initial_ping.take() {
            return Poll::Ready(Some(initial_ping.encoded()));
        }

        loop {
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                return match cmd {
                    FlashblocksCommand::NewFlashBlock { msg, response } => {
                        this.pending_pong = Some(response);
                        Poll::Ready(Some(FlashblocksProtoMessage::ping_message(msg).encoded()))
                    }
                };
            }

            let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else {
                return Poll::Ready(None);
            };

            let Some(msg) = FlashblocksProtoMessage::decode_message(&mut &msg[..]) else {
                return Poll::Ready(None);
            };

            match msg.message {
                FlashblocksProtoMessageKind::Ping => {
                    return Poll::Ready(Some(FlashblocksProtoMessage::pong().encoded()));
                }
                FlashblocksProtoMessageKind::Pong => {}
                FlashblocksProtoMessageKind::PingMessage(msg) => {
                    return Poll::Ready(Some(FlashblocksProtoMessage::pong_message(msg).encoded()));
                }
                FlashblocksProtoMessageKind::PongMessage(msg) => {
                    if let Some(sender) = this.pending_pong.take() {
                        sender.send(msg).ok();
                    }
                    continue;
                }
            }

            return Poll::Pending;
        }
    }
}
