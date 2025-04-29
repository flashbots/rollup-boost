mod common;

use common::{RollupBoostTestHarnessBuilder, proxy::ProxyHandler};
use futures::FutureExt as _;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;

struct Handler;

impl ProxyHandler for Handler {
    fn handle(
        &self,
        _method: String,
        _params: Value,
        _result: Value,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send>> {
        async move { None }.boxed()
    }
}

#[tokio::test]
async fn engine_calls_in_order() -> eyre::Result<()> {
    let harness = RollupBoostTestHarnessBuilder::new("engine_calls_in_order")
        .proxy_handler(Arc::new(Handler))
        .build()
        .await?;

    let mut block_generator = harness.block_generator().await?;
    block_generator.set_block_time(0);

    for _ in 0..100 {
        let (_block, _block_creator) = block_generator.generate_block(false).await?;
    }

    Ok(())
}
