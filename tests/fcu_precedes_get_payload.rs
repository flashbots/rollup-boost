mod common;

use crate::common::RollupBoostTestHarnessBuilder;

#[tokio::test]
async fn fcu_precedes_get_payload() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("fcu_precedes_get_payload")
        .build()
        .await?;
    let mut block_generator = harness.block_generator().await?;
    block_generator.set_block_time(0);

    for _ in 0..100 {
        let (_block, _block_creator) = block_generator.generate_block(false).await?;
    }

    // Unknwon payload appears on logs.

    Ok(())
}
