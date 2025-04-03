use clap::Parser;
use tokio::task::JoinHandle;

use crate::Args;

#[derive(Debug)]
pub struct RollupBoost {
    args: Args,
    pub handle: JoinHandle<eyre::Result<()>>,
}

impl RollupBoost {
    pub fn args(&self) -> &Args {
        &self.args
    }

    pub fn rpc_endpoint(&self) -> String {
        format!("http://localhost:{}", self.args.rpc_port)
    }

    pub fn debug_endpoint(&self) -> String {
        format!("http://localhost:{}", self.args.debug_server_port)
    }
}

#[derive(Clone, Debug)]
pub struct RollupBoostConfig {
    pub args: Args,
}

impl Default for RollupBoostConfig {
    fn default() -> Self {
        Self {
            args: Args::parse_from([
                "rollup-boost",
                &format!(
                    "--l2-jwt-path={}/src/integration/testdata/jwt_secret.hex",
                    env!("CARGO_MANIFEST_DIR")
                ),
                &format!(
                    "--builder-jwt-path={}/src/integration/testdata/jwt_secret.hex",
                    env!("CARGO_MANIFEST_DIR")
                ),
                "--log-level=trace",
                "--tracing",
                "--metrics",
            ]),
        }
    }
}

impl RollupBoostConfig {
    pub fn start(self) -> RollupBoost {
        let args = self.args.clone();
        let handle = tokio::spawn(async move {
            let res = args.clone().run().await;
            if let Err(e) = &res {
                eprintln!("Error: {:?}", e);
            }
            res
        });

        RollupBoost {
            args: self.args,
            handle,
        }
    }
}
