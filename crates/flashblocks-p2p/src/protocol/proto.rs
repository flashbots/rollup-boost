//! Simple RLPx Ping Pong protocol that also support sending messages,
//! following [RLPx specs](https://github.com/ethereum/devp2p/blob/master/rlpx.md)

use alloy_primitives::bytes::{Buf, BufMut, BytesMut};
use reth_ethereum::network::eth_wire::{Capability, protocol::Protocol};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlashblocksProtoMessageId {
    Ping = 0x00,
    Pong = 0x01,
    PingMessage = 0x02,
    PongMessage = 0x03,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FlashblocksProtoMessageKind {
    Ping,
    Pong,
    PingMessage(String),
    PongMessage(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlashblocksProtoMessage {
    pub message_type: FlashblocksProtoMessageId,
    pub message: FlashblocksProtoMessageKind,
}

impl FlashblocksProtoMessage {
    /// Returns the capability for the `custom_rlpx` protocol.
    pub fn capability() -> Capability {
        Capability::new_static("flashblocks", 1)
    }

    /// Returns the protocol for the `custom_rlpx` protocol.
    pub fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 4)
    }

    /// Creates a ping message
    pub fn ping_message(msg: impl Into<String>) -> Self {
        Self {
            message_type: FlashblocksProtoMessageId::PingMessage,
            message: FlashblocksProtoMessageKind::PingMessage(msg.into()),
        }
    }
    /// Creates a pong message
    pub fn pong_message(msg: impl Into<String>) -> Self {
        Self {
            message_type: FlashblocksProtoMessageId::PongMessage,
            message: FlashblocksProtoMessageKind::PongMessage(msg.into()),
        }
    }

    /// Creates a ping message
    pub fn ping() -> Self {
        Self {
            message_type: FlashblocksProtoMessageId::Ping,
            message: FlashblocksProtoMessageKind::Ping,
        }
    }

    /// Creates a pong message
    pub fn pong() -> Self {
        Self {
            message_type: FlashblocksProtoMessageId::Pong,
            message: FlashblocksProtoMessageKind::Pong,
        }
    }

    /// Creates a new `FlashblocksProtoMessage` with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            FlashblocksProtoMessageKind::Ping | FlashblocksProtoMessageKind::Pong => {}
            FlashblocksProtoMessageKind::PingMessage(msg)
            | FlashblocksProtoMessageKind::PongMessage(msg) => {
                buf.put(msg.as_bytes());
            }
        }
        buf
    }

    /// Decodes a `FlashblocksProtoMessage` from the given message buffer.
    pub fn decode_message(buf: &mut &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }
        let id = buf[0];
        buf.advance(1);
        let message_type = match id {
            0x00 => FlashblocksProtoMessageId::Ping,
            0x01 => FlashblocksProtoMessageId::Pong,
            0x02 => FlashblocksProtoMessageId::PingMessage,
            0x03 => FlashblocksProtoMessageId::PongMessage,
            _ => return None,
        };
        let message = match message_type {
            FlashblocksProtoMessageId::Ping => FlashblocksProtoMessageKind::Ping,
            FlashblocksProtoMessageId::Pong => FlashblocksProtoMessageKind::Pong,
            FlashblocksProtoMessageId::PingMessage => FlashblocksProtoMessageKind::PingMessage(
                String::from_utf8_lossy(&buf[..]).into_owned(),
            ),
            FlashblocksProtoMessageId::PongMessage => FlashblocksProtoMessageKind::PongMessage(
                String::from_utf8_lossy(&buf[..]).into_owned(),
            ),
        };

        Some(Self {
            message_type,
            message,
        })
    }
}
