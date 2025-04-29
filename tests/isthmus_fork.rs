use common::{GenesisBuilder, RollupBoostTestHarnessBuilder};

mod common;

#[tokio::test]
async fn isthmus_fork() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("isthmus_fork")
        .with_genesis(GenesisBuilder::default().with_isthmus_block(5))
        .build()
        .await?;
    let mut block_generator = harness.block_generator().await?;

    for _ in 0..10 {
        let (_block, block_creator) = block_generator.generate_block(false).await?;
        assert!(
            block_creator.is_builder(),
            "Block creator should be the builder"
        );
    }

    Ok(())
}
