use super::event::FlashblocksP2PEvent;
use crate::connection::handler::FlashblocksConnectionHandler;
use reth_ethereum::network::{api::PeerId, protocol::ProtocolHandler};
use std::net::SocketAddr;
use tokio::sync::mpsc;

/// Protocol state is an helper struct to store the protocol events.
#[derive(Clone, Debug)]
pub struct FlashblocksP2PState {
    pub events: mpsc::UnboundedSender<FlashblocksP2PEvent>,
}

/// The protocol handler takes care of incoming and outgoing connections.
#[derive(Debug)]
pub struct FlashblocksProtoHandler {
    pub state: FlashblocksP2PState,
}

impl ProtocolHandler for FlashblocksProtoHandler {
    type ConnectionHandler = FlashblocksConnectionHandler;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(FlashblocksConnectionHandler {
            state: self.state.clone(),
        })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(FlashblocksConnectionHandler {
            state: self.state.clone(),
        })
    }
}
