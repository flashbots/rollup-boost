#[cfg(test)]
mod tests {
    use crate::{EthApiOverrideServer, FlashblocksApiExt, FlashblocksOverlay, cache::Metadata};
    use alloy_eips::Encodable2718;
    use alloy_genesis::Genesis;
    use alloy_primitives::{B256, Bytes, TxKind, U256, address, hex};
    use alloy_provider::{Provider, RootProvider};
    use alloy_rpc_client::RpcClient;
    use alloy_rpc_types_engine::PayloadId;
    use op_alloy_consensus::{OpDepositReceipt, TxDeposit};
    use reth_node_builder::{EngineNodeLauncher, Node, NodeBuilder, NodeConfig, NodeHandle};
    use reth_node_core::{args::RpcServerArgs, exit::NodeExitFuture};
    use reth_optimism_chainspec::OpChainSpecBuilder;
    use reth_optimism_node::{OpNode, args::RollupArgs};
    use reth_optimism_primitives::OpReceipt;
    use reth_provider::providers::BlockchainProvider;
    use reth_tasks::TaskManager;
    use rollup_boost::{
        ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashblocksPayloadV1,
    };
    use std::collections::HashMap;
    use std::{any::Any, net::SocketAddr, sync::Arc};
    use tokio::sync::{mpsc, oneshot};
    use url::Url;

    pub struct NodeContext {
        _node: Box<dyn Any + Sync + Send>,
        _node_exit_future: NodeExitFuture,
        sender: mpsc::Sender<(FlashblocksPayloadV1, oneshot::Sender<()>)>,
        http_api_addr: SocketAddr,
    }

    impl NodeContext {
        pub async fn send_payload(&self, payload: FlashblocksPayloadV1) -> eyre::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.sender.send((payload, tx)).await?;
            rx.await?;
            Ok(())
        }

        pub async fn provider(&self) -> eyre::Result<RootProvider> {
            let url = format!("http://{}", self.http_api_addr);
            let client = RpcClient::builder().http(url.parse()?);

            Ok(RootProvider::new(client))
        }
    }

    fn block_info_tx() -> (B256, Bytes) {
        // L1 block info for OP mainnet block 124665056 (stored in input of tx at index 0)
        //
        // https://optimistic.etherscan.io/tx/0x312e290cf36df704a2217b015d6455396830b0ce678b860ebfcc30f41403d7b1
        const DATA: &[u8] = &hex!(
            "440a5e200000146b000f79c500000000000000040000000066d052e700000000013ad8a3000000000000000000000000000000000000000000000000000000003ef1278700000000000000000000000000000000000000000000000000000000000000012fdf87b89884a61e74b322bbcf60386f543bfae7827725efaaf0ab1de2294a590000000000000000000000006887246668a3b87f54deb3b94ba47a6f63f32985"
        );

        let deposit_tx = TxDeposit {
            source_hash: B256::default(),
            from: address!("DeaDDEaDDeAdDeAdDEAdDEaddeAddEAdDEAd0001"),
            to: TxKind::Call(address!("4200000000000000000000000000000000000015")),
            mint: 0,
            value: U256::default(),
            gas_limit: 210000,
            is_system_transaction: false,
            input: DATA.into(),
        };

        let encoded_tx = deposit_tx.encoded_2718();
        (deposit_tx.tx_hash(), encoded_tx.into())
    }

    async fn setup_node() -> eyre::Result<NodeContext> {
        let tasks = TaskManager::current();
        let exec = tasks.executor();

        let genesis: Genesis = serde_json::from_str(include_str!("assets/genesis.json")).unwrap();
        let chain_spec = Arc::new(
            OpChainSpecBuilder::base_mainnet()
                .genesis(genesis)
                .ecotone_activated()
                .build(),
        );

        // Use with_unused_ports() to let Reth allocate random ports and avoid port collisions
        let node_config = NodeConfig::new(chain_spec.clone())
            .with_rpc(RpcServerArgs::default().with_unused_ports().with_http())
            .with_unused_ports();

        let node = OpNode::new(RollupArgs::default());

        // Start websocket server to simulate the builder and send payloads back to the node
        let (sender, mut receiver) =
            mpsc::channel::<(FlashblocksPayloadV1, oneshot::Sender<()>)>(100);

        let NodeHandle {
            node,
            node_exit_future,
        } = NodeBuilder::new(node_config.clone())
            .testing_node(exec.clone())
            .with_types_and_provider::<OpNode, BlockchainProvider<_>>()
            .with_components(node.components_builder())
            .with_add_ons(node.add_ons())
            .extend_rpc_modules(move |ctx| {
                // We are not going to use the websocket connection to send payloads so we use
                // a dummy url.
                let flashblocks_overlay =
                    FlashblocksOverlay::new(Url::parse("ws://localhost:8546")?, chain_spec);

                let eth_api = ctx.registry.eth_api().clone();
                let api_ext = FlashblocksApiExt::new(eth_api.clone(), flashblocks_overlay.clone());

                ctx.modules.replace_configured(api_ext.into_rpc())?;

                tokio::spawn(async move {
                    while let Some((payload, tx)) = receiver.recv().await {
                        flashblocks_overlay.process_payload(payload).unwrap();
                        tx.send(()).unwrap();
                    }
                });

                Ok(())
            })
            .launch_with_fn(|builder| {
                let launcher = EngineNodeLauncher::new(
                    builder.task_executor().clone(),
                    builder.config().datadir(),
                    Default::default(),
                );
                builder.launch_with(launcher)
            })
            .await?;

        let http_api_addr = node
            .rpc_server_handle()
            .http_local_addr()
            .ok_or_else(|| eyre::eyre!("Failed to get http api address"))?;

        Ok(NodeContext {
            _node: Box::new(node),
            _node_exit_future: node_exit_future,
            sender,
            http_api_addr,
        })
    }

    struct FlashblocksTransaction {
        tx: Bytes,
        hash: B256,
        receipt: OpReceipt,
        balance: U256,
    }

    impl FlashblocksTransaction {
        pub fn with_balance(mut self, balance: U256) -> Self {
            self.balance = balance;
            self
        }

        pub fn with_receipt(mut self, receipt: OpReceipt) -> Self {
            self.receipt = receipt;
            self
        }
    }

    struct FlashblocksPayloadBuilder {
        index: u64,
        txns: Vec<FlashblocksTransaction>,
    }

    impl FlashblocksPayloadBuilder {
        pub fn new() -> Self {
            Self {
                index: 0,
                txns: vec![],
            }
        }

        pub fn with_transaction(mut self, tx: FlashblocksTransaction) -> Self {
            self.txns.push(tx);
            self
        }

        pub fn with_index(mut self, index: u64) -> Self {
            self.index = index;
            self
        }

        pub fn build(self) -> eyre::Result<FlashblocksPayloadV1> {
            // Add the build info tx to the payload if it is index 0
            let mut receipts = HashMap::new();
            let mut transactions = vec![];

            if self.index == 0 {
                let (build_info_tx_hash, build_info_tx) = block_info_tx();
                receipts.insert(
                    build_info_tx_hash.to_string(),
                    OpReceipt::Deposit(OpDepositReceipt::default()),
                );
                transactions.push(build_info_tx);
            }

            for tx in self.txns {
                receipts.insert(tx.hash.to_string(), tx.receipt);
                transactions.push(tx.tx);
            }

            let metadata = Metadata {
                receipts,
                ..Default::default()
            };

            Ok(FlashblocksPayloadV1 {
                payload_id: PayloadId::default(),
                index: self.index,
                base: Some(ExecutionPayloadBaseV1::default()),
                diff: ExecutionPayloadFlashblockDeltaV1 {
                    transactions: transactions,
                    ..Default::default()
                },
                metadata: serde_json::to_value(&metadata)?,
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn test_get_block_by_number_pending() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let node = setup_node().await?;
        let provider = node.provider().await?;

        let fb_payload = FlashblocksPayloadBuilder::new().with_index(0).build()?;
        node.send_payload(fb_payload).await?;

        let block = provider
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
            .await?
            .expect("pending block expected");

        let txs = block.transactions.as_hashes().unwrap();
        assert_eq!(txs.len(), 1);

        Ok(())
    }
}
