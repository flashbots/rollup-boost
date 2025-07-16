use metrics::{Counter, Gauge, Histogram};
use metrics_derive::Metrics;

#[derive(Metrics, Clone)]
#[metrics(scope = "flashblocks.subscriber")]
pub struct FlashblocksSubscriberMetrics {
    /// Total number of WebSocket reconnection attempts
    #[metric(describe = "Total number of WebSocket reconnection attempts")]
    pub reconnect_attempts: Counter,

    /// Current WebSocket connection status (1 = connected, 0 = disconnected)
    #[metric(describe = "Current WebSocket connection status")]
    pub connection_status: Gauge,

    #[metric(describe = "Number of flashblock messages received from builder")]
    pub messages_received: Counter,

    #[metric(describe = "Number of errors when extending payload")]
    pub extend_payload_errors: Counter,

    #[metric(describe = "Number of times the current payload ID has been set")]
    pub current_payload_id_mismatch: Counter,
}

#[derive(Metrics, Clone)]
#[metrics(scope = "flashblocks.provider")]
pub struct FlashblocksProviderMetrics {
    #[metric(describe = "Number of flashblocks used to build a block")]
    pub flashblocks_used: Histogram,
}
