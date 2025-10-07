use crate::{
    lib_api::{RollupBoost, RollupBoostConfig, RollupBoostConfigBuilder},
    ExecutionMode, Health,
};
use http::Uri;
use std::str::FromStr;

/// Example showing how a consensus layer (like kona-node) would integrate with rollup-boost
struct ConsensusLayer {
    rollup_boost: RollupBoost,
}

impl ConsensusLayer {
    fn new(rollup_boost: RollupBoost) -> Self {
        Self { rollup_boost }
    }

    /// Get current health status
    fn health_check(&self) -> Health {
        self.rollup_boost.health()
    }
}

#[tokio::test]
async fn test_consensus_layer_integration() -> eyre::Result<()> {
    let config = RollupBoostConfigBuilder::new()
        .l2_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .builder_jwt_secret_hex("0x0000000000000000000000000000000000000000000000000000000000000000")?
        .l2_url(Uri::from_str("http://127.0.0.1:8551")?)
        .builder_url(Uri::from_str("http://127.0.0.1:8552")?)
        .l2_timeout(5000)
        .builder_timeout(5000)
        .execution_mode(ExecutionMode::Enabled)
        .external_state_root(false)
        .ignore_unhealthy_builders(false)
        .build();

    let rollup_boost = RollupBoost::new(config)?;
    let consensus_layer = ConsensusLayer::new(rollup_boost);

    // Test health check functionality
    assert_eq!(consensus_layer.health_check(), Health::Healthy);

    Ok(())
}

#[test]
fn test_rollup_boost_config_validation() -> eyre::Result<()> {
    // Test that config builder properly validates and sets values
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

#[test]
fn test_default_config() -> eyre::Result<()> {
    let config = RollupBoostConfig::default();

    assert_eq!(config.execution_mode, ExecutionMode::Enabled);
    assert!(!config.external_state_root);
    assert!(!config.ignore_unhealthy_builders);
    assert_eq!(config.l2_timeout, 1000);
    assert_eq!(config.builder_timeout, 1000);

    Ok(())
}
