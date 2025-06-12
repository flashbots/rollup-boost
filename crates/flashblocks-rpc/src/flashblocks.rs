use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{Address, TxHash, U256};
use jsonrpsee::core::{RpcResult, async_trait};
use op_alloy_network::Optimism;
use reth_rpc_eth_api::{RpcBlock, RpcReceipt};

use crate::EthApiOverrideServer;

pub struct FlashblocksOverlay {}

impl FlashblocksOverlay {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl EthApiOverrideServer for FlashblocksOverlay {
    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> RpcResult<Option<RpcBlock<Optimism>>> {
        todo!()
    }

    async fn get_transaction_receipt(
        &self,
        tx_hash: TxHash,
    ) -> RpcResult<Option<RpcReceipt<Optimism>>> {
        todo!()
    }

    async fn get_balance(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        todo!()
    }

    async fn get_transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
    ) -> RpcResult<U256> {
        todo!()
    }
}
