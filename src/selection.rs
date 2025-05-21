use crate::{OpExecutionPayloadEnvelope, PayloadSource};

#[derive(Debug, Clone, Copy)]
pub enum BlockSelectionPolicy {
    GasUsed { threshold: u8 },
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

                // TODO: if builder gas is x% less than l2 gas, select l2 payload
                if builder_gas < (1.0 - *threshold as f64 / 100_f64) * l2_gas {
                    (l2_payload, PayloadSource::L2)
                } else {
                    (builder_payload, PayloadSource::Builder)
                }
            }
        }
    }
}
