use jsonrpsee::core::ClientError;
use op_alloy_rpc_types_engine::OptimismExecutionPayloadEnvelopeV3;

// Define a trait for choosing a payload
pub trait PayloadSelector {
    fn select_payload(
        &self,
        local_payload: Result<OptimismExecutionPayloadEnvelopeV3, ClientError>,
        builder_payloads: Vec<Result<OptimismExecutionPayloadEnvelopeV3, ClientError>>,
    ) -> Result<OptimismExecutionPayloadEnvelopeV3, ClientError>;
}

pub struct DefaultPayloadSelector;

impl PayloadSelector for DefaultPayloadSelector {
    fn select_payload(
        &self,
        local_payload: Result<OptimismExecutionPayloadEnvelopeV3, ClientError>,
        builder_payloads: Vec<Result<OptimismExecutionPayloadEnvelopeV3, ClientError>>,
    ) -> Result<OptimismExecutionPayloadEnvelopeV3, ClientError> {
        builder_payloads
            .iter()
            .filter_map(|payload| payload.as_ref().ok())
            .max_by_key(|p| p.block_value)
            .map(|p| Ok(p.clone()))
            .unwrap_or(local_payload)
    }
}
