use common::RollupBoostTestHarnessBuilder;

mod common;

#[tokio::test]
async fn miner_set_max_da_size() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("miner set max da size")
        .with_isthmus_block(5)
        .build()
        .await?;
    let mut block_generator = harness.block_generator().await?;
    block_generator.generate_builder_blocks(10).await?;

    Ok(())
}
