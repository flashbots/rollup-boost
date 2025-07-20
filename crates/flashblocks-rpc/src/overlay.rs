use crate::{FlashblocksApi, rpc};
use alloy_consensus::Transaction;
use alloy_eips::Decodable2718;
use alloy_primitives::{Address, TxHash, TxKind, U256};
use alloy_rpc_types::AccessList;
use futures_util::StreamExt;
use jsonrpsee::core::async_trait;
use op_alloy_consensus::{OpTxEnvelope, OpTypedTransaction};
use op_alloy_network::Optimism;
use reth_node_api::NodeTypesWithDB;
use reth_optimism_chainspec::OpChainSpec;
use reth_provider::{ProviderFactory, StateProvider, providers::ProviderNodeTypes};
use reth_revm::{
    Context, ExecuteEvm, Journal, MainBuilder, MainContext, MainnetEvm,
    context::{BlockEnv, CfgEnv, Evm, Transaction, TxEnv},
    database::StateProviderDatabase,
    db::CacheDB,
    handler::instructions::EthInstructions,
    primitives::hardfork::SpecId,
};
use reth_rpc_eth_api::{RpcBlock, RpcReceipt};
use rollup_boost::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashblocksPayloadV1,
};
use std::{io::Read, sync::Arc};
use tokio::sync::{RwLock, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info};
use url::Url;

pub struct FlashblocksRpcOverlay<N: ProviderNodeTypes> {
    url: Url,
    provider_factory: ProviderFactory<N>,
    cache: Arc<RwLock<FlashblocksCache>>,
    // TODO: stream handle
    // TODO: track latest payload id
}

impl<N> FlashblocksRpcOverlay<N>
where
    N: ProviderNodeTypes,
{
    pub fn new(url: Url, chain_spec: Arc<OpChainSpec>) -> Self {
        todo!()
        // Self {
        //     url,
        // }
    }

    pub fn spawn(&mut self) -> eyre::Result<()> {
        let url = self.url.clone();
        // TODO: return handle
        self.handle_flashblocks_stream(url);
        Ok(())
    }

    // pub fn process_payload(&self, payload: FlashblocksPayloadV1) -> eyre::Result<()> {
    //     self.cache.process_payload(payload)
    // }

    fn handle_flashblocks_stream(&self, url: Url) {
        let (tx, mut rx) = mpsc::channel(100);
        tokio::spawn(async move {
            //     let mut backoff = std::time::Duration::from_secs(1);
            //     const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(10);

            loop {
                if let Ok((mut stream, _)) = connect_async(url.as_str()).await {
                    // TODO: logging

                    while let Some(res) = stream.next().await {
                        match res {
                            Ok(msg) => match msg {
                                Message::Text(bytes) => {
                                    // TODO: decode
                                    tx.send(bytes).await.expect("TODO: handle error");
                                }

                                Message::Close(_) => {
                                    todo!();
                                }

                                _ => {}
                            },

                            Err(e) => {
                                // TODO: error logging
                            }
                        }
                    }
                }
            }
        });

        let cache = self.cache.clone();
        let provider_factory = self.provider_factory.clone();
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                let flashblock = serde_json::from_str::<FlashblocksPayloadV1>(&bytes)
                    .expect("TODO: handle error ");

                if flashblock.index == 0 {
                    if let Some(base) = flashblock.base {
                        // TODO: create new cache
                        let new_cache = FlashblocksCache::new(provider_factory.clone(), base);

                        // TODO: process flashblock delta
                        // new_cache.process_delta(flashblock.diff);
                        // *cache.write().await = new_cache;
                    }
                } else {
                    // TODO: proces flashblocks delta
                    // cache.write().await.process_delta(flashblock.diff);
                }
            }
        });
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

// #[async_trait]
// impl FlashblocksApi for FlashblocksRpcOverlay {
//     async fn block_by_number(&self, full: bool) -> Option<RpcBlock<Optimism>> {
//         self.cache.get_block(full)
//     }
//
//     async fn get_transaction_receipt(&self, tx_hash: TxHash) -> Option<RpcReceipt<Optimism>> {
//         self.cache.get_receipt(&tx_hash)
//     }
//
//     async fn get_balance(&self, address: Address) -> Option<U256> {
//         self.cache.get_balance(address)
//     }
//
//     async fn get_transaction_count(&self, address: Address) -> Option<u64> {
//         self.cache.get_transaction_count(address)
//     }
// }
//

pub type CacheEvm = MainnetEvm<
    Context<
        BlockEnv,
        TxEnv,
        CfgEnv,
        CacheDB<StateProviderDatabase<Box<dyn StateProvider>>>,
        Journal<CacheDB<StateProviderDatabase<Box<dyn StateProvider>>>>,
        (),
    >,
>;

pub struct FlashblocksCache {
    pub evm: CacheEvm,
}

impl FlashblocksCache {
    pub fn new<N: ProviderNodeTypes>(
        state_provider: ProviderFactory<N>,
        base: ExecutionPayloadBaseV1,
    ) -> Self {
        let state = state_provider.latest().expect("TODO: handle error");
        let db = StateProviderDatabase::new(state);
        let cache_db = CacheDB::new(db);

        let mut ctx = Context::mainnet().with_db(cache_db);

        ctx.modify_block(|block| {
            *block = BlockEnv {
                number: U256::from(base.block_number),
                gas_limit: base.gas_limit,
                timestamp: U256::from(base.timestamp),
                basefee: base.base_fee_per_gas.try_into().unwrap_or(u64::MAX),
                beneficiary: base.fee_recipient,
                prevrandao: Some(base.prev_randao),
                ..Default::default()
            };
        });

        // TODO: update to config op stack evm
        let evm = ctx.build_mainnet();

        Self { evm }
    }

    pub fn process_delta(&mut self, delta: ExecutionPayloadFlashblockDeltaV1) -> eyre::Result<()> {
        // Process each transaction in the delta
        for tx_bytes in delta.transactions {
            // Decode the transaction from bytes
            let tx_envelope =
                OpTxEnvelope::decode_2718(&mut tx_bytes.as_ref()).expect("TODO: Handle error");

            // Convert transaction envelope to TxEnv for EVM execution
            let tx_env = match &tx_envelope {
                OpTxEnvelope::Legacy(tx) => {
                    let signer = tx.recover_signer().expect("TODO: handle error");

                    let tx_kind = if let Some(to) = tx.to() {
                        TxKind::Call(to)
                    } else {
                        TxKind::Create
                    };

                    TxEnv {
                        caller: signer,
                        gas_limit: tx.gas_limit(),
                        gas_price: tx.gas_price().unwrap_or_default(),
                        kind: tx_kind,
                        value: tx.value(),
                        data: tx.input().to_owned(),
                        nonce: tx.nonce(),
                        ..Default::default()
                    }
                }
                OpTxEnvelope::Eip2930(tx) => {
                    let signer = tx.recover_signer().expect("TODO: handle error");

                    let tx_kind = match tx.to() {
                        Some(to) => TxKind::Call(to),
                        None => TxKind::Create,
                    };

                    TxEnv {
                        caller: signer,
                        gas_limit: tx.gas_limit(),
                        gas_price: tx.gas_price().unwrap_or_default(),
                        kind: tx_kind,
                        value: tx.value(),
                        data: tx.input().to_owned(),
                        nonce: tx.nonce(),
                        access_list: *tx.access_list().unwrap(),
                        ..Default::default()
                    }
                }
                OpTxEnvelope::Eip1559(tx) => {
                    let signer = tx.recover_signer().expect("TODO: handle error");

                    let tx_kind = match tx.to() {
                        Some(to) => TxKind::Call(to),
                        None => TxKind::Create,
                    };

                    TxEnv {
                        caller: signer,
                        gas_limit: tx.gas_limit(),
                        gas_price: tx.gas_price().unwrap_or_default(),
                        kind: tx_kind,
                        value: tx.value(),
                        data: tx.input().to_owned(),
                        nonce: tx.nonce(),
                        access_list: *tx.access_list().unwrap(),
                        ..Default::default()
                    }
                }

                OpTxEnvelope::Eip7702(tx) => {
                    let signer = tx.recover_signer().expect("TODO: handle error");
                    let tx_kind = match tx.to() {
                        Some(to) => TxKind::Call(to),
                        None => TxKind::Create,
                    };

                    TxEnv {
                        caller: signer,
                        gas_limit: tx.gas_limit(),
                        gas_price: tx.gas_price().unwrap_or_default(),
                        kind: tx_kind,
                        value: tx.value(),
                        data: tx.input().to_owned(),
                        nonce: tx.nonce(),
                        ..Default::default()
                    }
                }

                OpTxEnvelope::Deposit(tx) => {
                    let tx_kind = match tx.to() {
                        Some(to) => TxKind::Call(to),
                        None => TxKind::Create,
                    };

                    TxEnv {
                        caller: tx.from,
                        gas_limit: tx.gas_limit(),
                        gas_price: tx.gas_price().unwrap_or_default(),
                        kind: tx_kind,
                        value: tx.value(),
                        data: tx.input().to_owned(),
                        nonce: tx.nonce(),
                        ..Default::default()
                    }
                }
            };

            self.evm.modify_tx_env(|env| *env = tx_env);
            let result = self
                .evm
                .transact()
                .map_err(|e| eyre::eyre!("Transaction execution failed: {:?}", e))?;

            // Commit the transaction result to the database
            self.evm.db_mut().commit(result.state);
        }

        Ok(())
    }
}
