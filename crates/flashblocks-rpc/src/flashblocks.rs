use std::io::Read;

use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{Address, TxHash, U256};
use futures_util::StreamExt;
use jsonrpsee::core::{RpcResult, async_trait};
use op_alloy_network::Optimism;
use reth_rpc_eth_api::{RpcBlock, RpcReceipt};
use rollup_boost::FlashblocksPayloadV1;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info};
use url::Url;

use crate::EthApiOverrideServer;

pub struct FlashblocksOverlay {
    url: Url,
}

impl FlashblocksOverlay {
    pub fn new(url: Url) -> Self {
        Self { url }
    }

    pub fn start(&mut self) -> eyre::Result<()> {
        Ok(())
    }

    pub fn websocket_stream(&self) {
        let url = self.url.clone();

        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(10);

            loop {
                match connect_async(url.as_str()).await {
                    Ok((ws_stream, _)) => {
                        info!("WebSocket connection established");
                        let (_write, mut read) = ws_stream.split();

                        while let Some(msg) = read.next().await {
                            debug!("Received message: {:?}", msg);

                            match msg {
                                Ok(Message::Binary(bytes)) => match try_decode_message(&bytes) {
                                    Ok(payload) => {
                                        info!("Received payload: {:?}", payload);
                                    }
                                    Err(e) => {
                                        error!("failed to parse fb message: {}", e);
                                    }
                                },
                                Ok(Message::Close(_)) => break,
                                Err(e) => {}
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "WebSocket connection error, retrying in {:?}: {}",
                            backoff, e
                        );
                        tokio::time::sleep(backoff).await;
                        // Double the backoff time, but cap at MAX_BACKOFF
                        backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
                        continue;
                    }
                }
            }
        });
    }
}

fn try_decode_message(bytes: &[u8]) -> eyre::Result<FlashblocksPayloadV1> {
    let text = try_parse_message(bytes)?;

    let payload: FlashblocksPayloadV1 = match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            return Err(eyre::eyre!("failed to parse message: {}", e));
        }
    };

    Ok(payload)
}

fn try_parse_message(bytes: &[u8]) -> eyre::Result<String> {
    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        if text.trim_start().starts_with("{") {
            return Ok(text);
        }
    }

    let mut decompressor = brotli::Decompressor::new(bytes, 4096);
    let mut decompressed = Vec::new();
    decompressor.read_to_end(&mut decompressed)?;

    let text = String::from_utf8(decompressed)?;
    Ok(text)
}

#[async_trait]
impl EthApiOverrideServer for FlashblocksOverlay {
    async fn block_by_number(
        &self,
        _number: BlockNumberOrTag,
        _full: bool,
    ) -> RpcResult<Option<RpcBlock<Optimism>>> {
        Ok(None)
    }

    async fn get_transaction_receipt(
        &self,
        _tx_hash: TxHash,
    ) -> RpcResult<Option<RpcReceipt<Optimism>>> {
        Ok(None)
    }

    async fn get_balance(
        &self,
        _address: Address,
        _block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        Ok(U256::ZERO)
    }

    async fn get_transaction_count(
        &self,
        _address: Address,
        _block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        Ok(U256::ZERO)
    }
}
