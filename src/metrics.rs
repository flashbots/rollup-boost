use metrics::Histogram;
use metrics_derive::Metrics;

#[derive(Metrics)]
#[metrics(scope = "rpc")]
pub struct ServerMetrics {
    #[metric(describe = "Latency for `engine_newPayloadV3`")]
    pub new_payload_v3: Histogram,

    #[metric(describe = "Latency for `engine_getPayloadV3`")]
    pub get_payload_v3: Histogram,

    #[metric(describe = "Latency for `engine_forkChoiceUpdatedV3`")]
    pub fork_choice_updated_v3: Histogram,
}
