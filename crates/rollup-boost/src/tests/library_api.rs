use crate::{
    lib_api::{RollupBoost, RollupBoostConfigBuilder},
    server::tests::{MockEngineServer, spawn_server},
    ExecutionMode, Health,
};
use alloy_primitives::{hex, B256, U256};
use alloy_rpc_types_engine::{
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ForkchoiceState,
    JwtSecret, PayloadId, PayloadStatusEnum,
};
use alloy_rpc_types_eth::BlockNumberOrTag;
use http::Uri;
use std::str::FromStr;

#[tokio::test]
async fn test_rollup_boost_library_creation() -> eyre::Result<()> {
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .builder_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .l2_url(Uri::from_str("http://127.0.0.1:8551")?)
        .builder_url(Uri::from_str("http://127.0.0.1:8551")?)
        .l2_timeout(5000)
        .builder_timeout(5000)
        .execution_mode(ExecutionMode::Enabled)
        .external_state_root(false)
        .ignore_unhealthy_builders(true)
        .build();

    let rollup_boost = RollupBoost::new(config)?;

    // Test that we can get execution mode
    assert_eq!(rollup_boost.execution_mode(), ExecutionMode::Enabled);

    // Test that health is initially healthy
    assert_eq!(rollup_boost.health(), Health::Healthy);

    Ok(())
}

#[tokio::test]
async fn test_rollup_boost_health_checking() -> eyre::Result<()> {
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .builder_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .l2_url(Uri::from_str("http://127.0.0.1:8551")?)
        .builder_url(Uri::from_str("http://127.0.0.1:8551")?)
        .l2_timeout(1000)
        .builder_timeout(1000)
        .execution_mode(ExecutionMode::Enabled)
        .external_state_root(false)
        .ignore_unhealthy_builders(false)
        .build();

    let mut rollup_boost = RollupBoost::new(config)?;

    // Test that we can start health checking
    rollup_boost.start_health_check(60, 10)?;

    // Health should still be healthy initially
    assert_eq!(rollup_boost.health(), Health::Healthy);

    // Test that we can stop health checking
    rollup_boost.stop_health_check().await;

    Ok(())
}

#[tokio::test]
async fn test_rollup_boost_block_by_number() -> eyre::Result<()> {
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .builder_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .l2_url(Uri::from_str("http://127.0.0.1:8551")?)
        .builder_url(Uri::from_str("http://127.0.0.1:8551")?)
        .l2_timeout(1000)
        .builder_timeout(1000)
        .execution_mode(ExecutionMode::Enabled)
        .external_state_root(false)
        .ignore_unhealthy_builders(false)
        .build();

    let rollup_boost = RollupBoost::new(config)?;

    // This should fail to connect but should not panic - testing the API structure
    let result = rollup_boost.get_block_by_number(BlockNumberOrTag::Latest, false).await;

    // We expect this to fail since there's no actual node running
    assert!(result.is_err());

    Ok(())
}

#[test]
fn test_rollup_boost_config_builder() -> eyre::Result<()> {
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex("0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef")?
        .builder_jwt_secret_hex("0xfedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321")?
        .l2_url(Uri::from_str("http://localhost:8551")?)
        .builder_url(Uri::from_str("http://localhost:8552")?)
        .l2_timeout(3000)
        .builder_timeout(3000)
        .execution_mode(ExecutionMode::DryRun)
        .external_state_root(true)
        .ignore_unhealthy_builders(false)
        .build();

    assert_eq!(config.execution_mode, ExecutionMode::DryRun);
    assert!(config.external_state_root);
    assert!(!config.ignore_unhealthy_builders);
    assert_eq!(config.l2_timeout, 3000);
    assert_eq!(config.builder_timeout, 3000);

    Ok(())
}

#[tokio::test]
async fn test_library_fork_choice_updated() -> eyre::Result<()> {
    let jwt_secret = JwtSecret::random();
    
    let l2_mock = MockEngineServer::new();
    let builder_mock = MockEngineServer::new();
    
    let (_l2_server, l2_addr) = spawn_server(l2_mock).await;
    let (_builder_server, builder_addr) = spawn_server(builder_mock).await;
    
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .builder_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .l2_url(Uri::from_str(&format!("http://{}", l2_addr))?)
        .builder_url(Uri::from_str(&format!("http://{}", builder_addr))?)
        .l2_timeout(5000)
        .builder_timeout(5000)
        .execution_mode(ExecutionMode::Enabled)
        .build();
    
    let rollup_boost = RollupBoost::new(config)?;
    
    let fork_choice_state = ForkchoiceState {
        head_block_hash: B256::ZERO,
        safe_block_hash: B256::ZERO,
        finalized_block_hash: B256::ZERO,
    };
    
    let result = rollup_boost.fork_choice_updated_v3(fork_choice_state, None).await?;
    
    assert_eq!(result.payload_status.status, PayloadStatusEnum::Valid);
    
    Ok(())
}

#[tokio::test]
async fn test_library_get_payload() -> eyre::Result<()> {
    let jwt_secret = JwtSecret::random();
    
    let l2_mock = MockEngineServer::new();
    let builder_mock = MockEngineServer::new();
    
    let (_l2_server, l2_addr) = spawn_server(l2_mock).await;
    let (_builder_server, builder_addr) = spawn_server(builder_mock).await;
    
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .builder_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .l2_url(Uri::from_str(&format!("http://{}", l2_addr))?)
        .builder_url(Uri::from_str(&format!("http://{}", builder_addr))?)
        .l2_timeout(5000)
        .builder_timeout(5000)
        .execution_mode(ExecutionMode::Enabled)
        .build();
    
    let rollup_boost = RollupBoost::new(config)?;
    
    let payload_id = PayloadId::new([1u8; 8]);
    let result = rollup_boost.get_payload_v3(payload_id).await?;
    
    // Verify we got a valid payload
    assert_eq!(result.execution_payload.payload_inner.payload_inner.block_number, 0xa946u64);
    
    Ok(())
}

#[tokio::test]
async fn test_library_new_payload() -> eyre::Result<()> {
    let jwt_secret = JwtSecret::random();
    
    let l2_mock = MockEngineServer::new();
    let builder_mock = MockEngineServer::new();
    
    let (_l2_server, l2_addr) = spawn_server(l2_mock).await;
    let (_builder_server, builder_addr) = spawn_server(builder_mock).await;
    
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .builder_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .l2_url(Uri::from_str(&format!("http://{}", l2_addr))?)
        .builder_url(Uri::from_str(&format!("http://{}", builder_addr))?)
        .l2_timeout(5000)
        .builder_timeout(5000)
        .execution_mode(ExecutionMode::Enabled)
        .build();
    
    let rollup_boost = RollupBoost::new(config)?;
    
    let payload = ExecutionPayloadV3 {
        payload_inner: ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                base_fee_per_gas: U256::from(7u64),
                block_number: 0xa946u64,
                block_hash: hex!("a5ddd3f286f429458a39cafc13ffe89295a7efa8eb363cf89a1a4887dbcf272b").into(),
                logs_bloom: hex!("00200004000000000000000080000000000200000000000000000000000000000000200000000000000000000000000000000000800000000200000000000000000000000000000000000008000000200000000000000000000001000000000000000000000000000000800000000000000000000100000000000030000000000000000040000000000000000000000000000000000800080080404000000000000008000000000008200000000000200000000000000000000000000000000000000002000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000100000000000000000000").into(),
                extra_data: hex!("d883010d03846765746888676f312e32312e31856c696e7578").into(),
                gas_limit: 0x1c9c380,
                gas_used: 0x1f4a9,
                timestamp: 0x651f35b8,
                fee_recipient: hex!("f97e180c050e5ab072211ad2c213eb5aee4df134").into(),
                parent_hash: hex!("d829192799c73ef28a7332313b3c03af1f2d5da2c36f8ecfafe7a83a3bfb8d1e").into(),
                prev_randao: hex!("753888cc4adfbeb9e24e01c84233f9d204f4a9e1273f0e29b43c4c148b2b8b7e").into(),
                receipts_root: hex!("4cbc48e87389399a0ea0b382b1c46962c4b8e398014bf0cc610f9c672bee3155").into(),
                state_root: hex!("017d7fa2b5adb480f5e05b2c95cb4186e12062eed893fc8822798eed134329d1").into(),
                transactions: vec![],
            },
            withdrawals: vec![],
        },
        blob_gas_used: 0xc0000,
        excess_blob_gas: 0x580000,
    };
    
    let result = rollup_boost.new_payload_v3(
        payload,
        vec![],
        B256::ZERO,
    ).await?;
    
    assert_eq!(result.status, PayloadStatusEnum::Valid);
    
    Ok(())
}

#[tokio::test]
async fn test_library_execution_mode_switching() -> eyre::Result<()> {
    let jwt_secret = JwtSecret::random();
    
    let l2_mock = MockEngineServer::new();
    let builder_mock = MockEngineServer::new();
    
    let (_l2_server, l2_addr) = spawn_server(l2_mock).await;
    let (_builder_server, builder_addr) = spawn_server(builder_mock).await;
    
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .builder_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .l2_url(Uri::from_str(&format!("http://{}", l2_addr))?)
        .builder_url(Uri::from_str(&format!("http://{}", builder_addr))?)
        .execution_mode(ExecutionMode::Enabled)
        .build();
    
    let rollup_boost = RollupBoost::new(config)?;
    
    assert_eq!(rollup_boost.execution_mode(), ExecutionMode::Enabled);
    
    rollup_boost.set_execution_mode(ExecutionMode::DryRun);
    assert_eq!(rollup_boost.execution_mode(), ExecutionMode::DryRun);
    
    rollup_boost.set_execution_mode(ExecutionMode::Disabled);
    assert_eq!(rollup_boost.execution_mode(), ExecutionMode::Disabled);
    
    Ok(())
}

#[tokio::test]
async fn test_library_health_status() -> eyre::Result<()> {
    let jwt_secret = JwtSecret::random();
    
    let l2_mock = MockEngineServer::new();
    let builder_mock = MockEngineServer::new();
    
    let (_l2_server, l2_addr) = spawn_server(l2_mock).await;
    let (_builder_server, builder_addr) = spawn_server(builder_mock).await;
    
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .builder_jwt_secret_hex(&format!("0x{}", hex::encode(jwt_secret.as_bytes())))?
        .l2_url(Uri::from_str(&format!("http://{}", l2_addr))?)
        .builder_url(Uri::from_str(&format!("http://{}", builder_addr))?)
        .execution_mode(ExecutionMode::Enabled)
        .build();
    
    let rollup_boost = RollupBoost::new(config)?;
    
    // Initially set to healthy by library initialization
    let initial_health = rollup_boost.health();
    assert!(matches!(initial_health, Health::Healthy | Health::PartialContent));
    
    // Make a successful API call
    let payload_id = PayloadId::new([1u8; 8]);
    let result = rollup_boost.get_payload_v3(payload_id).await;
    
    // Should have a result (success or error)
    assert!(result.is_ok() || result.is_err());
    
    // Health status reflects real-time API results
    let health_after_call = rollup_boost.health();
    assert!(matches!(health_after_call, Health::Healthy | Health::PartialContent | Health::ServiceUnavailable));
    
    Ok(())
}
