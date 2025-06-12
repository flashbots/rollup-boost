use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{Address, TxHash, U256};
use jsonrpsee::{
    core::{RpcResult, async_trait},
    proc_macros::rpc,
};
use op_alloy_network::Optimism;
use reth::providers::TransactionsProvider;
use reth_optimism_primitives::OpTransactionSigned;
use reth_rpc_eth_api::{
    RpcBlock, RpcNodeCore, RpcReceipt,
    helpers::{EthBlocks, EthState, EthTransactions, FullEthApi},
};
use tracing::debug;

#[cfg_attr(not(test), rpc(server, namespace = "eth"))]
#[cfg_attr(test, rpc(server, client, namespace = "eth"))]
pub trait EthApiOverride {
    #[method(name = "getBlockByNumber")]
    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> RpcResult<Option<RpcBlock<op_alloy_network::Optimism>>>;

    #[method(name = "getTransactionReceipt")]
    async fn get_transaction_receipt(
        &self,
        tx_hash: TxHash,
    ) -> RpcResult<Option<RpcReceipt<Optimism>>>;

    #[method(name = "getBalance")]
    async fn get_balance(&self, address: Address, block_number: Option<BlockId>)
    -> RpcResult<U256>;

    #[method(name = "getTransactionCount")]
    async fn get_transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256>;
}

pub struct FlashblocksApiExt<Eth, FB> {
    eth_api: Eth,
    flashblocks_api: FB,
}

impl<Eth, FB> FlashblocksApiExt<Eth, FB> {
    pub fn new(eth_api: Eth, flashblocks_api: FB) -> Self {
        Self {
            eth_api,
            flashblocks_api,
        }
    }
}

#[async_trait]
impl<Eth, FB> EthApiOverrideServer for FlashblocksApiExt<Eth, FB>
where
    Eth: FullEthApi<NetworkTypes = Optimism> + Send + Sync + 'static,
    Eth: RpcNodeCore,
    <Eth as RpcNodeCore>::Provider: TransactionsProvider<Transaction = OpTransactionSigned>,
    FB: EthApiOverrideServer,
{
    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> RpcResult<Option<RpcBlock<Optimism>>> {
        debug!("block_by_number: {:?}", number);

        if number.is_pending() {
            self.flashblocks_api.block_by_number(number, full).await
        } else {
            EthBlocks::rpc_block(&self.eth_api, number.into(), full)
                .await
                .map_err(Into::into)
        }
    }

    async fn get_transaction_receipt(
        &self,
        tx_hash: TxHash,
    ) -> RpcResult<Option<RpcReceipt<Optimism>>> {
        debug!("get_transaction_receipt: {:?}", tx_hash);

        let receipt = EthTransactions::transaction_receipt(&self.eth_api, tx_hash).await;
        if let Ok(None) = receipt {
            let fb_receipt = self.flashblocks_api.get_transaction_receipt(tx_hash).await;
            fb_receipt.map_err(Into::into)
        } else {
            receipt.map_err(Into::into)
        }
    }

    async fn get_balance(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        debug!("get_balance: {:?}, {:?}", address, block_number);

        let block_id = block_number.unwrap_or_default();
        if block_id.is_pending() {
            self.flashblocks_api
                .get_balance(address, block_number)
                .await
        } else {
            EthState::balance(&self.eth_api, address, block_number)
                .await
                .map_err(Into::into)
        }
    }

    async fn get_transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        debug!("get_transaction_count: {:?}, {:?}", address, block_number);

        let block_id = block_number.unwrap_or_default();
        if block_id.is_pending() {
            self.flashblocks_api
                .get_transaction_count(address, block_number)
                .await
        } else {
            EthState::transaction_count(&self.eth_api, address, block_number)
                .await
                .map_err(Into::into)
        }
    }
}
