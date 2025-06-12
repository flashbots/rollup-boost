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
