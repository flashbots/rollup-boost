use std::sync::Arc;

use super::FlashblocksConnection;
use crate::protocol::{
    handler::{FlashblocksP2PNetworHandle, FlashblocksP2PState},
    proto::FlashblocksProtoMessage,
};
use ed25519_dalek::VerifyingKey;
use parking_lot::{Mutex, RwLock};
use reth_ethereum::network::{
    api::{Direction, PeerId},
    eth_wire::{capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol},
    protocol::{ConnectionHandler, OnNotSupported},
};
use rollup_boost::FlashblocksPayloadV1;
use tokio::sync::{
    broadcast,
    mpsc::{self},
};
use tokio_stream::wrappers::BroadcastStream;

/// The connection handler for the flashblocks RLPx protocol.
pub struct FlashblocksConnectionHandler<N> {
    pub network_handle: N,
    pub authorizer_vk: VerifyingKey,
    pub peer_tx: broadcast::Sender<FlashblocksProtoMessage>,
    pub flashblock_tx: broadcast::Sender<FlashblocksPayloadV1>,
    pub state: Arc<Mutex<FlashblocksP2PState>>,
}

impl<N: FlashblocksP2PNetworHandle> ConnectionHandler for FlashblocksConnectionHandler<N> {
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
            network_handle: self.network_handle,
            authorizer_vk: self.authorizer_vk,
            peer_tx: self.peer_tx.clone(),
            peer_rx: BroadcastStream::new(self.peer_tx.subscribe()),
            flashblock_tx: self.flashblock_tx.clone(),
            state: self.state.clone(),
            payload_id: Default::default(),
            received: Vec::new(),
        }
    }
}
