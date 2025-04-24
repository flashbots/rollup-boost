use std::{sync::Arc, time::Duration};

use tokio::{
    task::JoinHandle,
    time::{Instant, sleep_until},
};
use tracing::{error, info};

use crate::{Health, Probes, RpcClient};

pub struct HealthHandle {
    pub probes: Arc<Probes>,
    pub builder_client: Arc<RpcClient>,
    pub l2_client: Arc<RpcClient>,
    pub health_check_interval: u64,
}

impl HealthHandle {
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let (l2_block_height, builder_block_height) = tokio::join!(
                    self.l2_client.block_number(),
                    self.builder_client.block_number()
                );
                match l2_block_height {
                    Err(e) => {
                        error!(target: "rollup_boost::health", "Failed to get block height from l2 client: {}", e);
                        self.probes.set_health(Health::ServiceUnavailable);
                    }
                    Ok(l2_block_height) => match builder_block_height {
                        Err(e) => {
                            error!(target: "rollup_boost::health", "Failed to get block height from builder client: {}", e);
                            self.probes.set_health(Health::PartialContent);
                        }
                        Ok(builder_block_height) => {
                            if builder_block_height != l2_block_height {
                                error!(target: "rollup_boost::health", "Builder and L2 client block heights do not match: builder: {}, l2: {}", builder_block_height, l2_block_height);
                                self.probes.set_health(Health::PartialContent);
                            } else {
                                info!(target: "rollup_boost::health", %builder_block_height, "Health Status Check Passed - Builder and L2 client block heights match");
                                self.probes.set_health(Health::Healthy);
                            }
                        }
                    },
                }
                sleep_until(Instant::now() + Duration::from_secs(self.health_check_interval)).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use alloy_primitives::U256;
    use http::Uri;
    use http_body_util::BodyExt;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use reth_rpc_layer::JwtSecret;
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;
    use crate::{PayloadSource, Probes};

    pub struct MockHttpServer {
        addr: SocketAddr,
        join_handle: JoinHandle<()>,
    }

    impl Drop for MockHttpServer {
        fn drop(&mut self) {
            self.join_handle.abort();
        }
    }

    impl MockHttpServer {
        async fn serve<S>(f: fn(hyper::Request<hyper::body::Incoming>) -> S) -> eyre::Result<Self>
        where
            S: Future<Output = Result<hyper::Response<String>, hyper::Error>>
                + Send
                + Sync
                + 'static,
        {
            {
                let listener = TcpListener::bind("0.0.0.0:0").await?;
                let addr = listener.local_addr()?;

                let handle = tokio::spawn(async move {
                    loop {
                        match listener.accept().await {
                            Ok((stream, _)) => {
                                let io = TokioIo::new(stream);
                                tokio::spawn(async move {
                                    if let Err(err) = hyper::server::conn::http1::Builder::new()
                                        .serve_connection(io, service_fn(move |req| f(req)))
                                        .await
                                    {
                                        eprintln!("Error serving connection: {}", err);
                                    }
                                });
                            }
                            Err(e) => eprintln!("Error accepting connection: {}", e),
                        }
                    }
                });

                Ok(Self {
                    addr,
                    join_handle: handle,
                })
            }
        }
    }

    async fn handler_0(
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<String>, hyper::Error> {
        let body_bytes = match req.into_body().collect().await {
            Ok(buf) => buf.to_bytes(),
            Err(_) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": "Failed to read request body" },
                    "id": null
                });
                return Ok(hyper::Response::new(error_response.to_string()));
            }
        };

        let request_body: serde_json::Value = match serde_json::from_slice(&body_bytes) {
            Ok(json) => json,
            Err(_) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": "Invalid JSON format" },
                    "id": null
                });
                return Ok(hyper::Response::new(error_response.to_string()));
            }
        };

        let method = request_body["method"].as_str().unwrap_or_default();

        let response = match method {
            "eth_blockNumber" => json!({
                "jsonrpc": "2.0",
                "result": format!("{}", U256::ZERO),
                "id": request_body["id"]
            }),
            _ => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32601, "message": "Method not found" },
                    "id": request_body["id"]
                });
                return Ok(hyper::Response::new(error_response.to_string()));
            }
        };

        Ok(hyper::Response::new(response.to_string()))
    }

    async fn handler_1(
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<String>, hyper::Error> {
        let body_bytes = match req.into_body().collect().await {
            Ok(buf) => buf.to_bytes(),
            Err(_) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": "Failed to read request body" },
                    "id": null
                });
                return Ok(hyper::Response::new(error_response.to_string()));
            }
        };

        let request_body: serde_json::Value = match serde_json::from_slice(&body_bytes) {
            Ok(json) => json,
            Err(_) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": "Invalid JSON format" },
                    "id": null
                });
                return Ok(hyper::Response::new(error_response.to_string()));
            }
        };

        let method = request_body["method"].as_str().unwrap_or_default();

        let response = match method {
            "eth_blockNumber" => json!({
                "jsonrpc": "2.0",
                "result": format!("{}", U256::from(1)),
                "id": request_body["id"]
            }),
            _ => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32601, "message": "Method not found" },
                    "id": request_body["id"]
                });
                return Ok(hyper::Response::new(error_response.to_string()));
            }
        };

        Ok(hyper::Response::new(response.to_string()))
    }

    #[tokio::test]
    async fn test_health_check_healthy() -> eyre::Result<()> {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let probes = Arc::new(Probes::default());
        let builder = MockHttpServer::serve(handler_0).await.unwrap();
        let l2 = MockHttpServer::serve(handler_0).await.unwrap();

        let builder_client = Arc::new(RpcClient::new(
            format!("http://{}", builder.addr).parse::<Uri>()?,
            JwtSecret::random(),
            100,
            PayloadSource::Builder,
        )?);
        let l2_client = Arc::new(RpcClient::new(
            format!("http://{}", l2.addr).parse::<Uri>()?,
            JwtSecret::random(),
            100,
            PayloadSource::L2,
        )?);
        let health_handle = HealthHandle {
            probes: probes.clone(),
            builder_client: builder_client.clone(),
            l2_client: l2_client.clone(),
            health_check_interval: 60,
        };

        let _ = health_handle.spawn();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(matches!(probes.health(), Health::Healthy));
        Ok(())
    }

    #[tokio::test]
    async fn test_health_check_partial_content() -> eyre::Result<()> {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let probes = Arc::new(Probes::default());
        let builder = MockHttpServer::serve(handler_0).await.unwrap();
        let l2 = MockHttpServer::serve(handler_1).await.unwrap();

        let builder_client = Arc::new(RpcClient::new(
            format!("http://{}", builder.addr).parse::<Uri>()?,
            JwtSecret::random(),
            100,
            PayloadSource::Builder,
        )?);
        let l2_client = Arc::new(RpcClient::new(
            format!("http://{}", l2.addr).parse::<Uri>()?,
            JwtSecret::random(),
            100,
            PayloadSource::L2,
        )?);
        let health_handle = HealthHandle {
            probes: probes.clone(),
            builder_client: builder_client.clone(),
            l2_client: l2_client.clone(),
            health_check_interval: 60,
        };

        let _ = health_handle.spawn();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(matches!(probes.health(), Health::PartialContent));
        Ok(())
    }
}
