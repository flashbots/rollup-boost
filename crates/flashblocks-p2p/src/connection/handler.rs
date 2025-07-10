use super::FlashblocksConnection;
use crate::protocol::{
    event::FlashblocksP2PEvent, handler::FlashblocksP2PState, proto::FlashblocksProtoMessage,
};
use reth_ethereum::network::{
    api::{Direction, PeerId},
    eth_wire::{capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol},
    protocol::{ConnectionHandler, OnNotSupported},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The connection handler for the flashblocks RLPx protocol.
pub struct FlashblocksConnectionHandler {
    pub state: FlashblocksP2PState,
}

impl ConnectionHandler for FlashblocksConnectionHandler {
    type Connection = FlashblocksConnection;

    fn protocol(&self) -> Protocol {
        FlashblocksProtoMessage::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state
            .events
            .send(FlashblocksP2PEvent::Established {
                direction,
                peer_id,
                to_connection: tx,
            })
            .ok();
        FlashblocksConnection {
            conn,
            commands: UnboundedReceiverStream::new(rx),
            state: self.state,
        }
    }
}
