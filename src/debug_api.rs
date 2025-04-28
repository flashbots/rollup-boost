use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::http_client::HttpClient;
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::Server;
use parking_lot::Mutex;
use std::sync::Arc;

use crate::server::ExecutionMode;

#[rpc(server, client, namespace = "debug")]
trait DebugApi {
    #[method(name = "setExecutionMode")]
    async fn set_execution_mode(&self, request: ExecutionMode) -> RpcResult<()>;

    #[method(name = "getExecutionMode")]
    async fn get_execution_mode(&self) -> RpcResult<ExecutionMode>;
}

pub struct DebugServer {
    execution_mode: Arc<Mutex<ExecutionMode>>,
}

impl DebugServer {
    pub fn new(execution_mode: Arc<Mutex<ExecutionMode>>) -> Self {
        Self { execution_mode }
    }

    pub async fn run(self, debug_addr: &str) -> eyre::Result<()> {
        let server = Server::builder().build(debug_addr).await?;

        let handle = server.start(self.into_rpc());

        tracing::info!("Debug server listening on addr {}", debug_addr);

        // In this example we don't care about doing shutdown so let's it run forever.
        // You may use the `ServerHandle` to shut it down or manage it yourself.
        tokio::spawn(handle.stopped());

        Ok(())
    }

    pub fn execution_mode(&self) -> ExecutionMode {
        *self.execution_mode.lock()
    }

    pub fn set_execution_mode(&self, mode: ExecutionMode) {
        *self.execution_mode.lock() = mode;
    }
}

#[async_trait]
impl DebugApiServer for DebugServer {
    async fn set_execution_mode(&self, request: ExecutionMode) -> RpcResult<()> {
        self.set_execution_mode(request);
        tracing::info!("Set execution mode to {:?}", request);
        Ok(())
    }

    async fn get_execution_mode(&self) -> RpcResult<ExecutionMode> {
        Ok(self.execution_mode())
    }
}

pub struct DebugClient {
    client: HttpClient,
}

impl DebugClient {
    pub fn new(url: &str) -> eyre::Result<Self> {
        let client = HttpClient::builder().build(url)?;

        Ok(Self { client })
    }

    pub async fn set_execution_mode(&self, execution_mode: ExecutionMode) -> eyre::Result<()> {
        let result = DebugApiClient::set_execution_mode(&self.client, execution_mode).await?;
        Ok(result)
    }

    pub async fn get_execution_mode(&self) -> eyre::Result<ExecutionMode> {
        let result = DebugApiClient::get_execution_mode(&self.client).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_ADDR: &str = "127.0.0.1:5555";

    #[tokio::test]
    async fn test_set_execution_mode() -> eyre::Result<()> {
        // spawn the server and try to modify it with the client
        let execution_mode = Arc::new(Mutex::new(ExecutionMode::Enabled));

        let server = DebugServer::new(execution_mode.clone());
        server.run(DEFAULT_ADDR).await.unwrap();

        let client = DebugClient::new(format!("http://{}", DEFAULT_ADDR).as_str()).unwrap();

        // Test setting execution mode to Disabled
        client.set_execution_mode(ExecutionMode::Disabled).await?;

        assert_eq!(*execution_mode.lock(), ExecutionMode::Disabled);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_execution_mode() -> eyre::Result<()> {
        // spawn the server and try to modify it with the client
        let execution_mode = Arc::new(Mutex::new(ExecutionMode::Enabled));
        let server = DebugServer::new(execution_mode.clone());
        server.run(DEFAULT_ADDR).await?;

        let client = DebugClient::new(format!("http://{}", DEFAULT_ADDR).as_str()).unwrap();
        client.set_execution_mode(ExecutionMode::Disabled).await?;

        // Test setting execution mode to Disabled
        let result = client.get_execution_mode().await?;
        assert_eq!(result, ExecutionMode::Disabled);

        Ok(())
    }
}
