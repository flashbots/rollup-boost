use crate::{FlashblocksApi, cache::FlashblocksCache};
use alloy_primitives::{Address, TxHash, U256};
use ed25519_dalek::VerifyingKey;
use flashblocks_p2p::protocol::event::FlashblocksP2PEvent;
use jsonrpsee::core::async_trait;
use op_alloy_network::Optimism;
use reth_optimism_chainspec::OpChainSpec;
use reth_rpc_eth_api::{RpcBlock, RpcReceipt};
use rollup_boost::FlashblocksPayloadV1;
use std::{io::Read, sync::Arc};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub struct FlashblocksOverlayBuilder {
    events: mpsc::UnboundedReceiver<FlashblocksP2PEvent>,
    flashblocks_authorizor: VerifyingKey,
    cache: FlashblocksCache,
}

#[derive(Clone)]
pub struct FlashblocksOverlay {
    cache: FlashblocksCache,
}

impl FlashblocksOverlayBuilder {
    pub fn new(
        chain_spec: Arc<OpChainSpec>,
        flashblocks_authorizor: VerifyingKey,
        events: mpsc::UnboundedReceiver<FlashblocksP2PEvent>,
    ) -> Self {
        Self {
            events,
            flashblocks_authorizor,
            cache: FlashblocksCache::new(chain_spec),
        }
    }

    pub fn start(mut self) -> eyre::Result<FlashblocksOverlay> {
        let cache_cloned = self.cache.clone();
        let overlay = FlashblocksOverlay {
            cache: self.cache.clone(),
        };
        tokio::spawn(async move {
            while let Some(message) = self.events.recv().await {
                match message {
                    FlashblocksP2PEvent::Established { .. } => todo!(),
                    FlashblocksP2PEvent::FlashblocksPayloadV1(authorized) => {
                        match authorized.verify(self.flashblocks_authorizor) {
                            Ok(_) => {
                                if let Err(e) = cache_cloned.process_payload(authorized.payload) {
                                    error!("failed to process payload: {}", e);
                                }
                            }
                            Err(e) => {
                                error!("{e:?}");
                            }
                        }
                    }
                }
            }
        });

        Ok(overlay)
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
