use op_alloy_rpc_types_engine::OpExecutionPayloadV4;

pub enum BlockSelectionPolicy {
    GasUsed(u64),
}

impl BlockSelectionPolicy {
    fn select_block(
        &self,
        builder_payload: OpExecutionPayloadV4,
        l2_payload: OpExecutionPayloadV4,
    ) -> OpExecutionPayloadV4 {
        match self {
            BlockSelectionPolicy::GasUsed(threshold) => {
                todo!()
            }
        }
    }
}

