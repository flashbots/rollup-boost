#[cfg(test)]
mod tests {
    use crate::{EthApiOverrideServer, FlashblocksApiExt, FlashblocksOverlay, cache::Metadata};
    use alloy_eips::Encodable2718;
    use alloy_genesis::Genesis;
    use alloy_primitives::{B256, Bytes, TxKind, U256, address, hex, map::HashMap};
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
    use std::{any::Any, net::SocketAddr, sync::Arc};
    use tokio::sync::{mpsc, oneshot};
    use url::Url;

    pub struct NodeContext {
        _node: Box<dyn Any>,
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

    #[tokio::test]
    async fn setup() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let node = setup_node().await?;
        let provider = node.provider().await?;

        // TODO: Find a more ergonomic way to describe the build info tx which is mandatory as
        // the first transaction
        let (build_info_tx_hash, build_info_tx) = block_info_tx();
        let mut receipts = HashMap::new();
        receipts.insert(
            build_info_tx_hash.to_string(),
            OpReceipt::Deposit(OpDepositReceipt::default()),
        );

        let metadata = Metadata {
            receipts,
            ..Default::default()
        };

        node.send_payload(FlashblocksPayloadV1 {
            payload_id: PayloadId::default(),
            index: 0,
            base: Some(ExecutionPayloadBaseV1::default()),
            diff: ExecutionPayloadFlashblockDeltaV1 {
                transactions: vec![build_info_tx],
                ..Default::default()
            },
            metadata: serde_json::to_value(&metadata)?,
            ..Default::default()
        })
        .await?;

        let block = provider
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
            .await?
            .expect("pending block expected");

        let txs = block.transactions.as_hashes().unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0], build_info_tx_hash);

        Ok(())
    }
}
