use crate::{FlashblocksApi, cache::FlashblocksCache};
use alloy_primitives::{Address, TxHash, U256};
use jsonrpsee::core::async_trait;
use op_alloy_network::Optimism;
use reth_optimism_chainspec::OpChainSpec;
use reth_rpc_eth_api::{RpcBlock, RpcReceipt};
use rollup_boost::FlashblocksPayloadV1;
use std::{io::Read, sync::Arc};
use tokio::sync::broadcast;
use tracing::error;

pub struct FlashblocksOverlay {
    events: broadcast::Receiver<FlashblocksPayloadV1>,
    cache: FlashblocksCache,
}

impl Clone for FlashblocksOverlay {
    fn clone(&self) -> Self {
        Self {
            events: self.events.resubscribe(),
            cache: self.cache.clone(),
        }
    }
}

impl FlashblocksOverlay {
    pub fn new(
        chain_spec: Arc<OpChainSpec>,
        events: broadcast::Receiver<FlashblocksPayloadV1>,
    ) -> Self {
        Self {
            events,
            cache: FlashblocksCache::new(chain_spec),
        }
    }

    pub fn start(mut self) -> eyre::Result<()> {
        let cache_cloned = self.cache.clone();
        // let overlay = FlashblocksOverlay {
        //     cache: self.cache.clone(),
        // };
        tokio::spawn(async move {
            loop {
                // TODO: handle this error
                let payload = self.events.recv().await.unwrap();
                if let Err(e) = cache_cloned.process_payload(payload) {
                    error!("failed to process payload: {}", e);
                }
            }
        });

        Ok(())
    }

    pub fn process_payload(&self, payload: FlashblocksPayloadV1) -> eyre::Result<()> {
        self.cache.process_payload(payload)
    }
}

enum InternalMessage {
    NewPayload(FlashblocksPayloadV1),
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
impl FlashblocksApi for FlashblocksOverlay {
    async fn block_by_number(&self, full: bool) -> Option<RpcBlock<Optimism>> {
        self.cache.get_block(full)
    }

    async fn get_transaction_receipt(&self, tx_hash: TxHash) -> Option<RpcReceipt<Optimism>> {
        self.cache.get_receipt(&tx_hash)
    }

    async fn get_balance(&self, address: Address) -> Option<U256> {
        self.cache.get_balance(address)
    }

    async fn get_transaction_count(&self, address: Address) -> Option<u64> {
        self.cache.get_transaction_count(address)
    }
}
