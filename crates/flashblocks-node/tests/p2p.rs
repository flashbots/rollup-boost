use alloy_consensus::Receipt;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256, Bytes, TxHash, U256, address, b256};
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::PayloadId;
use ed25519_dalek::SigningKey;
use flashblocks_p2p::protocol::handler::{FlashblocksHandle, FlashblocksP2PProtocol, PeerMsg};
use flashblocks_rpc::{EthApiOverrideServer, FlashblocksApiExt, FlashblocksOverlay, Metadata};
use op_alloy_consensus::{OpPooledTransaction, OpTxEnvelope};
use reth_eth_wire::BasicNetworkPrimitives;
use reth_ethereum::network::{NetworkProtocols, protocol::IntoRlpxSubProtocol};
use reth_network::{NetworkHandle, Peers, PeersInfo};
use reth_network_peers::{NodeRecord, PeerId};
use reth_node_builder::{Node, NodeBuilder, NodeConfig, NodeHandle};
use reth_node_core::{
    args::{DiscoveryArgs, NetworkArgs, RpcServerArgs},
    exit::NodeExitFuture,
};
use reth_optimism_chainspec::OpChainSpecBuilder;
use reth_optimism_node::{OpNode, args::RollupArgs};
use reth_optimism_primitives::{OpPrimitives, OpReceipt};
use reth_provider::providers::BlockchainProvider;
use reth_tasks::{TaskExecutor, TaskManager};
use reth_tracing::tracing_subscriber::{self, util::SubscriberInitExt};
use rollup_boost::{
    Authorization, Authorized, AuthorizedMsg, AuthorizedPayload, ExecutionPayloadBaseV1,
    ExecutionPayloadFlashblockDeltaV1, FlashblocksP2PMsg, FlashblocksPayloadV1, StartPublish,
};
use std::{any::Any, collections::HashMap, net::SocketAddr, str::FromStr, sync::Arc};
use tracing::{Dispatch, info};

type Network = NetworkHandle<
    BasicNetworkPrimitives<
        OpPrimitives,
        OpPooledTransaction,
        reth_network::types::NewBlock<alloy_consensus::Block<OpTxEnvelope>>,
    >,
>;

pub struct NodeContext {
    p2p_handle: FlashblocksHandle,
    pub local_node_record: NodeRecord,
    http_api_addr: SocketAddr,
    _node_exit_future: NodeExitFuture,
    _node: Box<dyn Any + Sync + Send>,
    network_handle: Network,
}

impl NodeContext {
    pub async fn provider(&self) -> eyre::Result<RootProvider> {
        let url = format!("http://{}", self.http_api_addr);
        let client = RpcClient::builder().http(url.parse()?);

        Ok(RootProvider::new(client))
    }
}

fn init_tracing(filter: &str) -> tracing::subscriber::DefaultGuard {
    let sub = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_target(false)
        .without_time()
        .finish();

    Dispatch::new(sub).set_default()
}

async fn setup_node(
    exec: TaskExecutor,
    authorizer_sk: SigningKey,
    builder_sk: SigningKey,
    peers: Vec<(PeerId, SocketAddr)>,
) -> eyre::Result<NodeContext> {
    let genesis: Genesis = serde_json::from_str(include_str!("assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        OpChainSpecBuilder::base_mainnet()
            .genesis(genesis)
            .ecotone_activated()
            .build(),
    );

    let network_config = NetworkArgs {
        discovery: DiscoveryArgs {
            disable_discovery: true,
            ..DiscoveryArgs::default()
        },
        ..NetworkArgs::default()
    };

    // Use with_unused_ports() to let Reth allocate random ports and avoid port collisions
    let node_config = NodeConfig::new(chain_spec.clone())
        .with_network(network_config.clone())
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http())
        .with_unused_ports();

    let node = OpNode::new(RollupArgs::default());

    let p2p_handle = FlashblocksHandle::new(authorizer_sk.verifying_key(), Some(builder_sk));
    let p2p_handle_clone = p2p_handle.clone();

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
            let flashblocks_overlay = FlashblocksOverlay::new(p2p_handle_clone, chain_spec);
            flashblocks_overlay.clone().start()?;

            let eth_api = ctx.registry.eth_api().clone();
            let api_ext = FlashblocksApiExt::new(eth_api.clone(), flashblocks_overlay.clone());

            ctx.modules.replace_configured(api_ext.into_rpc())?;

            Ok(())
        })
        .launch()
        .await?;

    let p2p_protocol = FlashblocksP2PProtocol {
        network: node.network.clone(),
        handle: p2p_handle.clone(),
    };

    node.network
        .add_rlpx_sub_protocol(p2p_protocol.into_rlpx_sub_protocol());

    for (peer_id, addr) in peers {
        // If a trusted peer is provided, add it to the network
        node.network.add_peer(peer_id, addr);
    }

    let http_api_addr = node
        .rpc_server_handle()
        .http_local_addr()
        .ok_or_else(|| eyre::eyre!("Failed to get http api address"))?;

    let network_handle = node.network.clone();
    let local_node_record = network_handle.local_node_record();

    Ok(NodeContext {
        p2p_handle,
        local_node_record,
        http_api_addr,
        _node_exit_future: node_exit_future,
        _node: Box::new(node),
        network_handle,
    })
}

fn base_payload(block_number: u64, payload_id: PayloadId, index: u64) -> FlashblocksPayloadV1 {
    FlashblocksPayloadV1 {
        payload_id,
        index,
        base: Some(ExecutionPayloadBaseV1 {
            parent_beacon_block_root: B256::default(),
            parent_hash: B256::default(),
            fee_recipient: Address::ZERO,
            prev_randao: B256::default(),
            block_number,
            gas_limit: 0,
            timestamp: 0,
            extra_data: Bytes::new(),
            base_fee_per_gas: U256::ZERO,
        }),
        diff: ExecutionPayloadFlashblockDeltaV1::default(),
        metadata: serde_json::to_value(Metadata {
            block_number: 1,
            receipts: HashMap::default(),
            new_account_balances: HashMap::default(),
        })
        .unwrap(),
    }
}

const TEST_ADDRESS: Address = address!("0x1234567890123456789012345678901234567890");
const PENDING_BALANCE: u64 = 4600;

const TX1_HASH: TxHash =
    b256!("0x2be2e6f8b01b03b87ae9f0ebca8bbd420f174bef0fbcc18c7802c5378b78f548");
const TX2_HASH: TxHash =
    b256!("0xa6155b295085d3b87a3c86e342fe11c3b22f9952d0d85d9d34d223b7d6a17cd8");

fn next_payload(payload_id: PayloadId, index: u64) -> FlashblocksPayloadV1 {
    let tx1 = Bytes::from_str("0x7ef8f8a042a8ae5ec231af3d0f90f68543ec8bca1da4f7edd712d5b51b490688355a6db794deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e200000044d000a118b00000000000000040000000067cb7cb0000000000077dbd4000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000014edd27304108914dd6503b19b9eeb9956982ef197febbeeed8a9eac3dbaaabdf000000000000000000000000fc56e7272eebbba5bc6c544e159483c4a38f8ba3").unwrap();
    let tx2 = Bytes::from_str("0xf8cd82016d8316e5708302c01c94f39635f2adf40608255779ff742afe13de31f57780b8646e530e9700000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000001bc16d674ec8000000000000000000000000000000000000000000000000000156ddc81eed2a36d68302948ba0a608703e79b22164f74523d188a11f81c25a65dd59535bab1cd1d8b30d115f3ea07f4cfbbad77a139c9209d3bded89091867ff6b548dd714109c61d1f8e7a84d14").unwrap();

    // Send another test flashblock payload
    FlashblocksPayloadV1 {
        payload_id,
        index,
        base: None,
        diff: ExecutionPayloadFlashblockDeltaV1 {
            state_root: B256::default(),
            receipts_root: B256::default(),
            gas_used: 0,
            block_hash: B256::default(),
            transactions: vec![tx1, tx2],
            withdrawals: Vec::new(),
            logs_bloom: Default::default(),
            withdrawals_root: Default::default(),
        },
        metadata: serde_json::to_value(Metadata {
            block_number: 1,
            receipts: {
                let mut receipts = HashMap::default();
                receipts.insert(
                    TX1_HASH.to_string(), // transaction hash as string
                    OpReceipt::Legacy(Receipt {
                        status: true.into(),
                        cumulative_gas_used: 21000,
                        logs: vec![],
                    }),
                );
                receipts.insert(
                    TX2_HASH.to_string(), // transaction hash as string
                    OpReceipt::Legacy(Receipt {
                        status: true.into(),
                        cumulative_gas_used: 45000,
                        logs: vec![],
                    }),
                );
                receipts
            },
            new_account_balances: {
                let mut map = HashMap::default();
                map.insert(
                    TEST_ADDRESS.to_string(),
                    format!("0x{:x}", U256::from(PENDING_BALANCE)),
                );
                map
            },
        })
        .unwrap(),
    }
}

async fn setup_nodes(n: u8) -> eyre::Result<(Vec<NodeContext>, SigningKey)> {
    let mut nodes = Vec::new();
    let mut peers = Vec::new();
    let tasks = Box::leak(Box::new(TaskManager::current()));
    let exec = Box::leak(Box::new(tasks.executor()));
    let authorizer = SigningKey::from_bytes(&[0; 32]);

    for i in 0..n {
        let builder = SigningKey::from_bytes(&[(i + 1) % n; 32]);
        let node = setup_node(exec.clone(), authorizer.clone(), builder, peers.clone()).await?;
        let enr = node.local_node_record;
        peers.push((enr.id, enr.tcp_addr()));
        nodes.push(node);
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(6000)).await;

    Ok((nodes, authorizer))
}

#[tokio::test]
async fn test_double_failover() -> eyre::Result<()> {
    let _tracing = init_tracing("warn,flashblocks=trace");

    let (nodes, authorizer) = setup_nodes(3).await?;

    let mut publish_flashblocks = nodes[0].p2p_handle.ctx.flashblock_tx.subscribe();
    tokio::spawn(async move {
        while let Ok(payload) = publish_flashblocks.recv().await {
            println!("\n////////////////////////////////////////////////////////////////////\n");
            println!(
                "Received flashblock, payload_id: {}, index: {}",
                payload.payload_id, payload.index
            );
            println!("\n////////////////////////////////////////////////////////////////////\n");
        }
    });

    let latest_block = nodes[0]
        .provider()
        .await?
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await?
        .expect("latest block expected");
    assert_eq!(latest_block.number(), 0);

    // Querying pending block when it does not exists yet
    let pending_block = nodes[0]
        .provider()
        .await?
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
        .await?;
    assert!(pending_block.is_none());

    let payload_0 = base_payload(0, PayloadId::new([0; 8]), 0);
    let authorization_0 = Authorization::new(
        payload_0.payload_id,
        0,
        &authorizer,
        nodes[0].p2p_handle.builder_sk()?.verifying_key(),
    );
    let msg = payload_0.clone();
    let authorized_0 =
        AuthorizedPayload::new(nodes[0].p2p_handle.builder_sk()?, authorization_0, msg);
    nodes[0].p2p_handle.start_publishing(authorization_0)?;
    nodes[0].p2p_handle.publish_new(authorized_0).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let payload_1 = next_payload(payload_0.payload_id, 1);
    let authorization_1 = Authorization::new(
        payload_1.payload_id,
        0,
        &authorizer,
        nodes[1].p2p_handle.builder_sk()?.verifying_key(),
    );
    let authorized_1 = AuthorizedPayload::new(
        nodes[1].p2p_handle.builder_sk()?,
        authorization_1,
        payload_1.clone(),
    );
    nodes[1].p2p_handle.start_publishing(authorization_1)?;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    nodes[1].p2p_handle.publish_new(authorized_1).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Send a new block, this time from node 1
    let payload_2 = next_payload(payload_0.payload_id, 2);
    let msg = payload_2.clone();
    let authorization_2 = Authorization::new(
        payload_2.payload_id,
        0,
        &authorizer,
        nodes[2].p2p_handle.builder_sk()?.verifying_key(),
    );
    let authorized_2 = AuthorizedPayload::new(
        nodes[2].p2p_handle.builder_sk()?,
        authorization_2,
        msg.clone(),
    );
    nodes[2].p2p_handle.start_publishing(authorization_2)?;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    nodes[2].p2p_handle.publish_new(authorized_2).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(())
}

#[tokio::test]
async fn test_force_race_condition() -> eyre::Result<()> {
    let _tracing = init_tracing("warn,flashblocks=trace");

    let (nodes, authorizer) = setup_nodes(3).await?;

    let mut publish_flashblocks = nodes[0].p2p_handle.ctx.flashblock_tx.subscribe();
    tokio::spawn(async move {
        while let Ok(payload) = publish_flashblocks.recv().await {
            println!("\n////////////////////////////////////////////////////////////////////\n");
            println!(
                "Received flashblock, payload_id: {}, index: {}",
                payload.payload_id, payload.index
            );
            println!("\n////////////////////////////////////////////////////////////////////\n");
        }
    });

    let latest_block = nodes[0]
        .provider()
        .await?
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await?
        .expect("latest block expected");
    assert_eq!(latest_block.number(), 0);

    // Querying pending block when it does not exists yet
    let pending_block = nodes[0]
        .provider()
        .await?
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
        .await?;
    assert!(pending_block.is_none());

    let payload_0 = base_payload(0, PayloadId::new([0; 8]), 0);
    info!("Sending payload 0, index 0");
    let authorization = Authorization::new(
        payload_0.payload_id,
        0,
        &authorizer,
        nodes[0].p2p_handle.builder_sk()?.verifying_key(),
    );
    let msg = payload_0.clone();
    let authorized = AuthorizedPayload::new(nodes[0].p2p_handle.builder_sk()?, authorization, msg);
    nodes[0].p2p_handle.start_publishing(authorization)?;
    nodes[0].p2p_handle.publish_new(authorized).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Query pending block after sending the base payload with an empty delta
    let pending_block = nodes[1]
        .provider()
        .await?
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
        .await?
        .expect("pending block expected");

    assert_eq!(pending_block.number(), 0);
    assert_eq!(pending_block.transactions.hashes().len(), 0);

    info!("Sending payload 0, index 1");
    let payload_1 = next_payload(payload_0.payload_id, 1);
    let authorization = Authorization::new(
        payload_1.payload_id,
        0,
        &authorizer,
        nodes[0].p2p_handle.builder_sk()?.verifying_key(),
    );
    let authorized = AuthorizedPayload::new(
        nodes[0].p2p_handle.builder_sk()?,
        authorization,
        payload_1.clone(),
    );
    nodes[0].p2p_handle.publish_new(authorized).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Query pending block after sending the second payload with two transactions
    let block = nodes[1]
        .provider()
        .await?
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
        .await?
        .expect("pending block expected");

    assert_eq!(block.number(), 0);
    assert_eq!(block.transactions.hashes().len(), 2);

    // Send a new block, this time from node 1
    let payload_2 = base_payload(1, PayloadId::new([1; 8]), 0);
    info!("Sending payload 1, index 0");
    let authorization_1 = Authorization::new(
        payload_2.payload_id,
        1,
        &authorizer,
        nodes[1].p2p_handle.builder_sk()?.verifying_key(),
    );
    let authorization_2 = Authorization::new(
        payload_2.payload_id,
        1,
        &authorizer,
        nodes[2].p2p_handle.builder_sk()?.verifying_key(),
    );
    let msg = payload_2.clone();
    let authorized_1 = AuthorizedPayload::new(
        nodes[1].p2p_handle.builder_sk()?,
        authorization_1,
        msg.clone(),
    );
    nodes[1].p2p_handle.start_publishing(authorization_1)?;
    nodes[2].p2p_handle.start_publishing(authorization_2)?;
    // Wait for clearance to go through
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    tracing::error!(
        "{}",
        nodes[1]
            .p2p_handle
            .publish_new(authorized_1.clone())
            .unwrap_err()
    );
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    nodes[2].p2p_handle.stop_publishing()?;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    nodes[1].p2p_handle.publish_new(authorized_1)?;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(())
}

#[tokio::test]
async fn test_get_block_by_number_pending() -> eyre::Result<()> {
    let _tracing = init_tracing("warn,flashblocks=trace");

    let (nodes, authorizer) = setup_nodes(1).await?;

    let provider = nodes[0].provider().await?;

    let latest_block = provider
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await?
        .expect("latest block expected");
    assert_eq!(latest_block.number(), 0);

    // Querying pending block when it does not exists yet
    let pending_block = provider
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
        .await?;
    assert!(pending_block.is_none());

    let payload_id = PayloadId::new([0; 8]);
    let base_payload = base_payload(0, payload_id, 0);
    let authorization = Authorization::new(
        base_payload.payload_id,
        0,
        &authorizer,
        nodes[0].p2p_handle.builder_sk()?.verifying_key(),
    );
    let authorized = AuthorizedPayload::new(
        nodes[0].p2p_handle.builder_sk()?,
        authorization,
        base_payload,
    );
    nodes[0].p2p_handle.start_publishing(authorization)?;
    nodes[0].p2p_handle.publish_new(authorized).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Query pending block after sending the base payload with an empty delta
    let pending_block = provider
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
        .await?
        .expect("pending block expected");

    assert_eq!(pending_block.number(), 0);
    assert_eq!(pending_block.transactions.hashes().len(), 0);

    let next_payload = next_payload(payload_id, 1);
    let authorization = Authorization::new(
        next_payload.payload_id,
        0,
        &authorizer,
        nodes[0].p2p_handle.builder_sk()?.verifying_key(),
    );
    let authorized = AuthorizedPayload::new(
        nodes[0].p2p_handle.builder_sk()?,
        authorization,
        next_payload,
    );
    nodes[0].p2p_handle.start_publishing(authorization)?;
    nodes[0].p2p_handle.publish_new(authorized).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Query pending block after sending the second payload with two transactions
    let block = provider
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Pending)
        .await?
        .expect("pending block expected");

    assert_eq!(block.number(), 0);
    assert_eq!(block.transactions.hashes().len(), 2);

    Ok(())
}

#[tokio::test]
async fn test_peer_reputation() -> eyre::Result<()> {
    let _tracing = init_tracing("warn,flashblocks=trace");

    let (nodes, _authorizer) = setup_nodes(2).await?;

    let mut publish_flashblocks = nodes[0].p2p_handle.ctx.flashblock_tx.subscribe();
    tokio::spawn(async move {
        while let Ok(payload) = publish_flashblocks.recv().await {
            println!("\n////////////////////////////////////////////////////////////////////\n");
            println!(
                "Received flashblock, payload_id: {}, index: {}",
                payload.payload_id, payload.index
            );
            println!("\n////////////////////////////////////////////////////////////////////\n");
        }
    });

    let invalid_authorizer = SigningKey::from_bytes(&[99; 32]);

    let payload_0 = base_payload(0, PayloadId::new([0; 8]), 0);
    info!("Sending bad authorization");
    let authorization = Authorization::new(
        payload_0.payload_id,
        0,
        &invalid_authorizer,
        nodes[0].p2p_handle.builder_sk()?.verifying_key(),
    );

    let authorized_msg = AuthorizedMsg::StartPublish(StartPublish);
    let authorized_payload = Authorized::new(
        nodes[0].p2p_handle.builder_sk()?,
        authorization,
        authorized_msg,
    );
    let p2p_msg = FlashblocksP2PMsg::Authorized(authorized_payload);
    let peer_msg = PeerMsg::StartPublishing(p2p_msg.encode());

    let peers = nodes[1].network_handle.get_all_peers().await?;
    let peer_0 = &peers[0].remote_id;

    for _ in 0..100 {
        nodes[0].p2p_handle.ctx.peer_tx.send(peer_msg.clone()).ok();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        let rep_0 = nodes[1].network_handle.reputation_by_id(*peer_0).await?;
        if let Some(rep) = rep_0 {
            assert!(rep < 0, "Peer reputation should be negative");
        }
    }

    // Assert that the peer is banned
    assert!(nodes[1].network_handle.get_all_peers().await?.is_empty());

    Ok(())
}
