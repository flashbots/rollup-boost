use crate::{connection::FlashblocksCommand, protocol::auth::Authorized};
use reth_ethereum::network::{Direction, api::PeerId};
use rollup_boost::FlashblocksPayloadV1;
use tokio::sync::mpsc;

/// The events that can be emitted by our custom protocol.
#[derive(Debug)]
pub(crate) enum FlashblocksP2PEvent {
    Established {
        #[expect(dead_code)]
        direction: Direction,
        peer_id: PeerId,
        to_connection: mpsc::UnboundedSender<FlashblocksCommand>,
    },
    FlashblocksPayloadV1(Authorized<FlashblocksPayloadV1>),
}
