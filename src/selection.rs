use crate::{OpExecutionPayloadEnvelope, PayloadSource};

pub enum BlockSelectionPolicy {
    GasUsed { threshold: f64 },
}

impl BlockSelectionPolicy {
    pub fn select_block(
        &self,
        builder_payload: OpExecutionPayloadEnvelope,
        l2_payload: OpExecutionPayloadEnvelope,
    ) -> (OpExecutionPayloadEnvelope, PayloadSource) {
        match self {
            BlockSelectionPolicy::GasUsed { threshold } => {
                let builder_gas = builder_payload.gas_used() as f64;
                let l2_gas = l2_payload.gas_used() as f64;

                if l2_gas > *threshold * builder_gas {
                    (l2_payload, PayloadSource::L2)
                } else {
                    (builder_payload, PayloadSource::Builder)
                }
            }
        }
    }
}
