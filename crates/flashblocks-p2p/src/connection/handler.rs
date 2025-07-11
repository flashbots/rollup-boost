use super::FlashblocksConnection;
use crate::protocol::proto::FlashblocksProtoMessage;
use reth_ethereum::network::{
    api::{Direction, PeerId},
    eth_wire::{capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol},
    protocol::{ConnectionHandler, OnNotSupported},
};
use tokio::sync::{
    broadcast,
    mpsc::{self},
};
use tokio_stream::wrappers::BroadcastStream;

/// The connection handler for the flashblocks RLPx protocol.
pub struct FlashblocksConnectionHandler<N> {
    pub network_handle: N,
    pub inbound_tx: mpsc::UnboundedSender<FlashblocksProtoMessage>,
    pub flashblocks_sender_rx: broadcast::Receiver<FlashblocksProtoMessage>,
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
        FlashblocksConnection {
            conn,
            peer_id,
            inbound_tx: self.inbound_tx.clone(),
            outbound_rx: BroadcastStream::new(self.flashblocks_sender_rx.resubscribe()),
            network_handle: self.network_handle,
        }
    }
}
