mod common;

use common::RollupBoostTestHarnessBuilder;
use common::proxy_delay::DelayHandler;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn fcu_precedes_get_payload() -> eyre::Result<()> {
    let delay = DelayHandler::new(Duration::from_millis(200));

    let harness = RollupBoostTestHarnessBuilder::new("fcu_precedes_get_payload")
        .proxy_handler(Arc::new(delay))
        .build()
        .await?;
    let mut block_generator = harness.block_generator().await?;
    block_generator.set_block_time(0);

    for _ in 0..300 {
        let (_block, _block_creator) = block_generator.generate_block(false).await?;
    }

    // Count occurrences of "Unknown payload" in the Rollup-boost logs
    // TODO: Do not rely on reading logs, maybe use a metric?
    let content = std::fs::read_to_string(harness.rollup_boost.args().log_file.clone().unwrap())?;
    let unknown_payload_count = content.matches("Unknown payload").count();

    // TODO: The first iteration of the loop always fails to get the payload.
    // That is why we are expecting at most one error.
    assert!(
        unknown_payload_count <= 1,
        "Expected at most one 'Unknown payload' error, got {unknown_payload_count}"
    );

    Ok(())
}
