use integration::RollupBoostTestHarnessBuilder;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod integration;

#[tokio::test]
async fn builder_full_delay() -> eyre::Result<()> {
    // Create a dynamic handler that delays all the calls by 2 seconds
    let delay = Arc::new(Mutex::new(Duration::from_secs(0)));

    let delay_for_handler = delay.clone();
    let handler = Box::new(move |_method: &str, _params: Value, _result: Value| {
        let delay = delay_for_handler.lock().unwrap();
        // sleep the amount of time specified in the delay
        std::thread::sleep(*delay);
        None
    });

    // This integration test checks that if the builder has a general delay in processing ANY of the requests,
    // rollup-boost does not stop building blocks.
    let harness = RollupBoostTestHarnessBuilder::new("builder_full_delay")
        .proxy_handler(handler)
        .build()
        .await?;

    let mut block_generator = harness.get_block_generator().await?;

    // create 3 blocks that are processed by the builder
    for _ in 0..3 {
        let (_block, block_creator) = block_generator.generate_block(false).await?;
        assert!(
            block_creator.is_builder(),
            "Block creator should be the builder"
        );
    }

    // add the delay
    *delay.lock().unwrap() = Duration::from_secs(2);

    // create 3 blocks that are processed by the builder
    for _ in 0..3 {
        let (_block, block_creator) = block_generator.generate_block(false).await?;
        assert!(block_creator.is_l2(), "Block creator should be the builder");
    }

    Ok(())
}
