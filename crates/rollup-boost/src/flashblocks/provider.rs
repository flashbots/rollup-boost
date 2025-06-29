use std::sync::Arc;

use crate::RpcClient;

pub struct FlashblocksProvider {
    pub builder_client: Arc<RpcClient>,
}

impl FlashblocksProvider {
    pub fn new(builder_client: Arc<RpcClient>) -> Self {
        Self { builder_client }
    }
}
