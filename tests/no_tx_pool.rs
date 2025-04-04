use alloy_primitives::B256;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod integration;

use crate::integration::RollupBoostTestHarnessBuilder;
// use crate::server::ExecutionMode;
use op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV3;

#[tokio::test]
async fn test_integration_simple() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("test_integration_simple")
        .build()
        .await?;
    let mut block_generator = harness.get_block_generator().await?;

    for _ in 0..5 {
        let (_block, block_creator) = block_generator.generate_block(false).await?;
        assert!(
            block_creator.is_builder(),
            "Block creator should be the builder"
        );
    }

    Ok(())
}
