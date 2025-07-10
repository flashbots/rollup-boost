use super::event::FlashblocksP2PEvent;
use crate::connection::handler::FlashblocksConnectionHandler;
use reth_ethereum::network::{api::PeerId, protocol::ProtocolHandler};
use reth_network::Peers;
use std::net::SocketAddr;
use tokio::sync::mpsc;

pub(crate) trait FlashblocksP2PNetworHandle:
    Clone + Unpin + Peers + std::fmt::Debug + 'static
{
}

impl<N: Clone + Unpin + Peers + std::fmt::Debug + 'static> FlashblocksP2PNetworHandle for N {}

/// Protocol state is an helper struct to store the protocol events.
#[derive(Clone, Debug)]
pub struct FlashblocksP2PState {
    pub events: mpsc::UnboundedSender<FlashblocksP2PEvent>,
}

/// The protocol handler takes care of incoming and outgoing connections.
#[derive(Debug)]
pub struct FlashblocksProtoHandler<N: Peers + std::fmt::Debug> {
    pub state: FlashblocksP2PState,
    pub network_handle: N,
}

impl<N: FlashblocksP2PNetworHandle> FlashblocksProtoHandler<N> {
    /// Creates a new protocol handler with the given state.
    pub fn new(network_handle: N, events: mpsc::UnboundedSender<FlashblocksP2PEvent>) -> Self {
        Self {
            state: FlashblocksP2PState { events },
            network_handle,
        }
    }
}

impl<N: FlashblocksP2PNetworHandle> ProtocolHandler for FlashblocksProtoHandler<N> {
    type ConnectionHandler = FlashblocksConnectionHandler<N>;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(FlashblocksConnectionHandler::<N> {
            state: self.state.clone(),
            network_handle: self.network_handle.clone(),
        })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(FlashblocksConnectionHandler::<N> {
            state: self.state.clone(),
            network_handle: self.network_handle.clone(),
        })
    }
}
