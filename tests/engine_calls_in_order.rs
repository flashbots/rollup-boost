mod common;

use common::RollupBoostTestHarnessBuilder;

#[tokio::test]
async fn engine_calls_in_order() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("engine_calls_in_order")
        .build()
        .await?;

    let mut block_generator = harness.block_generator().await?;
    block_generator.set_block_time(0);

    for _ in 0..100 {
        let (_block, _block_creator) = block_generator.generate_block(false).await?;
    }

    Ok(())
}
