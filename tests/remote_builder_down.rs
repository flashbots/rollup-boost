use alloy_primitives::B256;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod integration;

use crate::integration::RollupBoostTestHarnessBuilder;
// use crate::server::ExecutionMode;
use op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV3;

#[tokio::test]
async fn test_integration_remote_builder_down() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("test_integration_remote_builder_down")
        .build()
        .await?;
    let mut block_generator = harness.get_block_generator().await?;

    for _ in 0..3 {
        let (_block, block_creator) = block_generator.generate_block(false).await?;
        assert!(
            block_creator.is_builder(),
            "Block creator should be the builder"
        );
    }

    // stop the builder
    harness.builder.stop().await?;

    // create 3 new blocks that are processed by the l2 builder
    for _ in 0..3 {
        let (_block, block_creator) = block_generator.generate_block(false).await?;
        assert!(block_creator.is_l2(), "Block creator should be l2");
    }

    // start the builder again
    harness.builder.start().await?;

    // the next block is computed by the l2 builder because the builder is not synced with the previous 3 blocks
    // But, once the builder receives the FCU request from rollup-boost, it will sync up the blocks with the
    // L2 block builder and be ready again.
    let (_block, block_creator) = block_generator.generate_block(false).await?;
    assert!(block_creator.is_l2(), "Block creator should be l2");

    // Note: We might add some sleep here if the builder is not synced in time. I have not seen this happen yet.

    // create 3 new blocks that are processed by the l2 builder because the builder is not synced with the previous 3 blocks
    for _ in 0..3 {
        let (_block, block_creator) = block_generator.generate_block(false).await?;
        assert!(block_creator.is_builder(), "Block creator should be l2");
    }

    Ok(())
}
