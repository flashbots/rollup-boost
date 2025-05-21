use serde::{Deserialize, Serialize};

use crate::{OpExecutionPayloadEnvelope, PayloadSource};

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, clap::ValueEnum)]
pub enum BlockSelectionPolicy {
    GasUsed,
}

impl BlockSelectionPolicy {
    pub fn select_block(
        &self,
        builder_payload: OpExecutionPayloadEnvelope,
        l2_payload: OpExecutionPayloadEnvelope,
    ) -> (OpExecutionPayloadEnvelope, PayloadSource) {
        match self {
            BlockSelectionPolicy::GasUsed => {
                let builder_gas = builder_payload.gas_used() as f64;
                let l2_gas = l2_payload.gas_used() as f64;

                // If the builder gas
                if builder_gas < l2_gas * 0.5 {
                    (l2_payload, PayloadSource::L2)
                } else {
                    (builder_payload, PayloadSource::Builder)
                }
            }
        }
    }
}
