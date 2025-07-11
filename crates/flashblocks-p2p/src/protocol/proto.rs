//! Simple RLPx Flashblocks protocol for propagating FlashblocksPayloadV1 messages
//! following [RLPx specs](https://github.com/ethereum/devp2p/blob/master/rlpx.md)

use alloy_primitives::bytes::{Buf, BufMut, BytesMut};
use reth_ethereum::network::eth_wire::{Capability, protocol::Protocol};
use rollup_boost::FlashblocksPayloadV1;
use serde_json;

use crate::protocol::auth::Authorized;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashblocksProtoMessageId {
    FlashblocksPayloadV1 = 0x00,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlashblocksProtoMessageKind {
    FlashblocksPayloadV1(Authorized<FlashblocksPayloadV1>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashblocksProtoMessage {
    pub message_type: FlashblocksProtoMessageId,
    pub message: FlashblocksProtoMessageKind,
}

impl FlashblocksProtoMessage {
    /// Returns the capability for the `flashblocks` protocol.
    pub fn capability() -> Capability {
        Capability::new_static("flashblocks", 1)
    }

    /// Returns the protocol for the `flashblocks` protocol.
    pub fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 1)
    }

    /// Creates a flashblocks payload message
    pub fn flashblocks_payload(payload: Authorized<FlashblocksPayloadV1>) -> Self {
        Self {
            message_type: FlashblocksProtoMessageId::FlashblocksPayloadV1,
            message: FlashblocksProtoMessageKind::FlashblocksPayloadV1(payload),
        }
    }

    /// Creates a new `FlashblocksProtoMessage` with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            FlashblocksProtoMessageKind::FlashblocksPayloadV1(payload) => {
                // Serialize the payload as JSON for transmission
                let json = serde_json::to_string(payload).unwrap_or_default();
                buf.put(json.as_bytes());
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
            0x00 => FlashblocksProtoMessageId::FlashblocksPayloadV1,
            _ => return None,
        };

        let message = match message_type {
            FlashblocksProtoMessageId::FlashblocksPayloadV1 => {
                // Deserialize the JSON payload
                let json_str = String::from_utf8_lossy(&buf[..]);
                match serde_json::from_str::<Authorized<FlashblocksPayloadV1>>(&json_str) {
                    Ok(payload) => FlashblocksProtoMessageKind::FlashblocksPayloadV1(payload),
                    Err(_) => return None,
                }
            }
        };

        Some(Self {
            message_type,
            message,
        })
    }
}
