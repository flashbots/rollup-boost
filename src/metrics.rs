use std::time::Duration;

use metrics::{counter, histogram, Counter, Histogram};
use metrics_derive::Metrics;

#[derive(Metrics)]
#[metrics(scope = "rpc")]
pub struct ServerMetrics {
    #[metric(describe = "Total latency for server `engine_newPayloadV3` call")]
    pub new_payload_v3_total: Histogram,

    #[metric(describe = "Total latency for server `engine_getPayloadV3` call")]
    pub get_payload_v3_total: Histogram,

    #[metric(describe = "Total latency for server `engine_forkChoiceUpdatedV3` call")]
    pub fork_choice_updated_v3_total: Histogram,

    #[metric(describe = "Count of blocks created by the builder")]
    pub blocks_created_by_builder: Counter,

    #[metric(describe = "Count of blocks created by the L2 builder")]
    pub blocks_created_by_l2: Counter,

    #[metric(describe = "Count of l2 client `engine_newPayloadV3` responses", labels = ["target", "code"])]
    #[allow(dead_code)]
    pub new_payload_v3_response_count: Counter,

    #[metric(describe = "Count of l2 client `engine_getPayloadV3` responses", labels = ["target", "code"])]
    #[allow(dead_code)]
    pub get_payload_v3_response_count: Counter,

    #[metric(describe = "Count of l2 client `engine_forkChoiceUpdatedV3` responses", labels = ["target", "code"])]
    #[allow(dead_code)]
    pub fork_choice_updated_v3_response_count: Counter,

    // Builder proxy metrics
    #[metric(describe = "Latency for builder client forwarded rpc calls (excluding the engine api)", labels = ["target", "method"])]
    #[allow(dead_code)]
    pub forwarded_call: Histogram,

    #[metric(describe = "Number of builder client rpc responses", labels = ["target", "http_status_code", "rpc_status_code", "method"])]
    #[allow(dead_code)]
    pub forwarded_call_response_count: Counter,
}

impl ServerMetrics {
    /*
    pub fn record_new_payload_v3(&self, latency: Duration, code: String, source: PayloadSource) {
        match source {
            PayloadSource::L2 => {
                self.l2_new_payload_v3.record(latency.as_secs_f64());
                counter!("rpc.l2_new_payload_v3_response_count", "code" => code).increment(1);
            }
            PayloadSource::Builder => {
                self.builder_new_payload_v3.record(latency.as_secs_f64());
                counter!("rpc.builder_new_payload_v3_response_count", "code" => code).increment(1);
            }
        }
    }

    pub fn record_get_payload_v3(&self, latency: Duration, code: String, source: PayloadSource) {
        match source {
            PayloadSource::L2 => {
                self.l2_get_payload_v3.record(latency.as_secs_f64());
                counter!("rpc.l2_get_payload_v3_response_count", "code" => code).increment(1);
            }
            PayloadSource::Builder => {
                self.builder_get_payload_v3.record(latency.as_secs_f64());
                counter!("rpc.builder_get_payload_v3_response_count", "code" => code).increment(1);
            }
        }
    }

    pub fn record_fork_choice_updated_v3(
        &self,
        latency: Duration,
        code: String,
        source: PayloadSource,
    ) {
        match source {
            PayloadSource::L2 => {
                self.l2_fork_choice_updated_v3.record(latency.as_secs_f64());
                counter!("rpc.l2_fork_choice_updated_v3_response_count", "code" => code)
                    .increment(1);
            }
            PayloadSource::Builder => {
                self.builder_fork_choice_updated_v3
                    .record(latency.as_secs_f64());
                counter!("rpc.builder_fork_choice_updated_v3_response_count", "code" => code)
                    .increment(1);
            }
        }
    }
    */

    pub fn record_builder_forwarded_call(&self, latency: Duration, method: String) {
        histogram!("rpc.builder_forwarded_call", "method" => method).record(latency.as_secs_f64());
    }

    pub fn increment_builder_rpc_response_count(
        &self,
        http_status_code: String,
        rpc_status_code: Option<String>,
        method: String,
    ) {
        counter!("rpc.builder_response_count",
            "http_status_code" => http_status_code,
            "rpc_status_code" => rpc_status_code.unwrap_or("".to_string()),
            "method" => method,
        )
        .increment(1);
    }

    pub fn record_l2_forwarded_call(&self, latency: Duration, method: String) {
        histogram!("rpc.l2_forwarded_call", "method" => method).record(latency.as_secs_f64());
    }

    pub fn increment_l2_rpc_response_count(
        &self,
        http_status_code: String,
        rpc_status_code: Option<String>,
        method: String,
    ) {
        counter!("rpc.l2_response_count",
            "http_status_code" => http_status_code,
            "rpc_status_code" => rpc_status_code.unwrap_or("".to_string()),
            "method" => method,
        )
        .increment(1);
    }
}
