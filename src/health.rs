use std::{sync::Arc, time::Duration};

use tokio::{
    task::JoinHandle,
    time::{Instant, sleep_until},
};
use tracing::{error, info};

use crate::{Health, Probes, RpcClient};

pub struct HealthHandle {
    pub probes: Arc<Probes>,
    pub builder_client: Arc<RpcClient>,
    pub l2_client: Arc<RpcClient>,
    pub health_check_interval: u64,
}

impl HealthHandle {
    pub fn spawn(self) -> JoinHandle<()> {
        let handle = tokio::spawn(async move {
            loop {
                let (l2_block_height, builder_block_height) = tokio::join!(
                    self.l2_client.block_number(),
                    self.builder_client.block_number()
                );
                match l2_block_height {
                    Err(e) => {
                        error!(target: "rollup_boost::health", "Failed to get block height from l2 client: {}", e);
                        self.probes.set_health(Health::ServiceUnavailable);
                    }
                    Ok(l2_block_height) => match builder_block_height {
                        Err(e) => {
                            error!(target: "rollup_boost::health", "Failed to get block height from builder client: {}", e);
                            self.probes.set_health(Health::PartialContent);
                        }
                        Ok(builder_block_height) => {
                            if builder_block_height != l2_block_height {
                                error!(target: "rollup_boost::health", "Builder and L2 client block heights do not match: builder: {}, l2: {}", builder_block_height, l2_block_height);
                                self.probes.set_health(Health::PartialContent);
                            } else {
                                info!(target: "rollup_boost::health", %builder_block_height, "Health Status Check Passed - Builder and L2 client block heights match");
                                self.probes.set_health(Health::Healthy);
                            }
                        }
                    },
                }
                sleep_until(Instant::now() + Duration::from_secs(self.health_check_interval)).await;
            }
        });

        handle
    }
}
