use metrics::{Counter, Gauge};
use metrics_derive::Metrics;
#[derive(Metrics)]
#[metrics(scope = "websocket_proxy")]
pub struct Metrics {
    #[metric(describe = "Messages sent to clients")]
    pub sent_messages: Counter,

    #[metric(describe = "Count of messages that were unable to be sent")]
    pub failed_messages: Counter,

    #[metric(describe = "Count of new connections opened")]
    pub new_connections: Counter,

    #[metric(describe = "Count of number of connections closed")]
    pub closed_connections: Counter,

    #[metric(describe = "Number of client connections currently open")]
    pub active_connections: Gauge,

    #[metric(describe = "Count of rate limited request")]
    pub rate_limited_requests: Counter,

    #[metric(describe = "Count of times that a client lagged")]
    pub lag_events: Counter,

    #[metric(describe = "Count of times upstream receiver was closed/errored")]
    pub upstream_errors: Counter,

    #[metric(describe = "Count of messages received from the upstream source")]
    pub upstream_messages: Gauge,
}
