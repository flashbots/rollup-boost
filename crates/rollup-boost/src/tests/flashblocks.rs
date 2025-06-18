use super::common::RollupBoostTestHarnessBuilder;

#[tokio::test]
async fn test_integration_flashblocks_simple() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("flashblocks_simple")
        .with_flashblocks(true)
        .with_isthmus_block(0)
        .build()
        .await?;
    let mut block_generator = harness.block_generator().await?;
    block_generator.generate_builder_blocks(10).await?;

    Ok(())
}
