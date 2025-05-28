use common::{RollupBoostTestHarnessBuilder, proxy::ProxyHandler};
use futures::FutureExt as _;
use serde_json::Value;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

mod common;

#[derive(Debug, Clone, Default)]
struct MaxDaSizeHandler {
    found_max_da_value: Arc<Mutex<u64>>,
}

impl MaxDaSizeHandler {
    fn get_found_max_da_value(&self) -> u64 {
        *self.found_max_da_value.lock().unwrap()
    }
}

impl ProxyHandler for MaxDaSizeHandler {
    fn handle(
        &self,
        method: String,
        params: Value,
        _result: Value,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send>> {
        println!("method: {:?}", method);
        if method == "miner_setMaxDASize" {
            // decode the params
            let params: Vec<u64> = serde_json::from_value(params).unwrap();
            assert_eq!(params.len(), 2);
            println!("params: {:?}", params);
            *self.found_max_da_value.lock().unwrap() = params[0];
        }
        async move { None }.boxed()
    }
}

#[tokio::test]
async fn miner_set_max_da_size() -> eyre::Result<()> {
    let handler = Arc::new(MaxDaSizeHandler::default());

    let harness = RollupBoostTestHarnessBuilder::new("miner_set_max_da_size")
        .with_isthmus_block(0)
        .proxy_handler(handler.clone())
        .build()
        .await?;
    let mut block_generator = harness.block_generator().await?;
    block_generator.generate_builder_blocks(1).await?;

    let engine_api = harness.engine_api()?;

    // send a max da size request
    let res = engine_api.set_max_da_size(1000000, 1000000).await?;
    println!("res: {:?}", res);
    let found = handler.get_found_max_da_value();
    println!("found: {}", found);

    Ok(())
}
