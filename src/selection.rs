use crate::{OpExecutionPayloadEnvelope, PayloadSource};
use serde::{Deserialize, Serialize};

/// Defines the strategy for choosing between the builder block and the L2 client block
/// during block production.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, clap::ValueEnum)]
pub enum BlockSelectionPolicy {
    /// Selects the block based on gas usage.
    ///
    /// If the builder block uses less than 90% of the gas used by the L2 client block,
    /// the L2 block is selected instead. This prevents propagation of valid but empty
    /// builder blocks and mitigates issues where the builder is not receiving enough
    /// transactions due to networking or peering failures.
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

                // Select the L2 block if the builder block uses less than 90% of the gas.
                // This avoids selecting empty or severely underfilled blocks,
                if builder_gas < l2_gas * 0.1 {
                    (l2_payload, PayloadSource::L2)
                } else {
                    (builder_payload, PayloadSource::Builder)
                }
            }
        }
    }
}
