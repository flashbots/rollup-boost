use super::FlashblocksConnection;
use crate::protocol::{
    event::FlashblocksP2PEvent,
    handler::FlashblocksP2PState,
    proto::{FlashblocksProtoMessage, FlashblocksProtoMessageKind},
};
use reth_ethereum::network::{
    api::{Direction, PeerId},
    eth_wire::{capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol},
    protocol::{ConnectionHandler, OnNotSupported},
};
use tokio::sync::{
    broadcast,
    mpsc::{self, UnboundedSender},
};
use tokio_stream::wrappers::{BroadcastStream, UnboundedReceiverStream};

/// The connection handler for the flashblocks RLPx protocol.
pub struct FlashblocksConnectionHandler<N> {
    // pub state: FlashblocksP2PState,
    pub inbound_tx: mpsc::UnboundedSender<FlashblocksProtoMessage>,
    pub outbound_rx: broadcast::Receiver<FlashblocksProtoMessage>,
    pub network_handle: N,
}

impl<N: Unpin + Send + Sync + 'static> ConnectionHandler for FlashblocksConnectionHandler<N> {
    type Connection = FlashblocksConnection<N>;

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
        // self.state
        //     .flashblock_stream
        //     .send(FlashblocksP2PEvent::Established {
        //         direction,
        //         peer_id,
        //         to_connection: tx,
        //     })
        //     .ok();
        FlashblocksConnection {
            conn,
            peer_id,
            inbound_tx: self.inbound_tx.clone(),
            outbound_rx: BroadcastStream::new(self.outbound_rx.clone()),
            network_handle: self.network_handle,
        }
    }
}
