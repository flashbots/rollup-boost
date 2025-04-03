use std::{
    borrow::Cow,
    collections::HashMap,
    fs::File,
    io::BufReader,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use http::Uri;
use serde_json::Value;
use testcontainers::{
    ContainerAsync, CopyToContainer, Image,
    core::{ContainerPort, WaitFor},
};

use crate::integration::L2_P2P_ENODE;

const NAME: &str = "ghcr.io/paradigmxyz/op-reth";
const TAG: &str = "v1.3.4";

const AUTH_RPC_PORT: u16 = 8551;
const P2P_PORT: u16 = 30303;

#[derive(Debug, Clone)]
pub struct OpRethConfig {
    chain: PathBuf,
    jwt_secret: PathBuf,
    p2p_secret: Option<PathBuf>,
    pub trusted_peers: Vec<String>,
    pub datadir: String,
    pub disable_discovery: bool,
    pub color: String,
    pub ipcdisable: bool,
    pub env_vars: HashMap<String, String>,
}

impl Default for OpRethConfig {
    fn default() -> Self {
        Self {
            chain: PathBuf::from(format!(
                "{}/src/integration/testdata/genesis.json",
                env!("CARGO_MANIFEST_DIR")
            )),
            jwt_secret: PathBuf::from(format!(
                "{}/src/integration/testdata/jwt_secret.hex",
                env!("CARGO_MANIFEST_DIR")
            )),
            p2p_secret: None,
            trusted_peers: vec![],
            datadir: "data".to_string(),
            disable_discovery: true,
            color: "never".to_string(),
            ipcdisable: true,
            env_vars: Default::default(),
        }
    }
}

impl OpRethConfig {
    pub fn set_trusted_peers(mut self, trusted_peers: Vec<String>) -> Self {
        self.trusted_peers = trusted_peers;
        self
    }

    pub fn set_jwt_secret(mut self, jwt_secret: PathBuf) -> Self {
        self.jwt_secret = jwt_secret;
        self
    }

    pub fn set_p2p_secret(mut self, p2p_secret: Option<PathBuf>) -> Self {
        self.p2p_secret = p2p_secret;
        self
    }

    pub fn build(self) -> OpRethImage {
        // Write the genesis file to the test directory and update the timestamp to the current time
        let genesis = {
            let file = File::open(&self.chain).unwrap();
            let reader = BufReader::new(file);
            let mut genesis: Value = serde_json::from_reader(reader).unwrap();

            // Update the timestamp field
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if let Some(config) = genesis.as_object_mut() {
                // Assuming timestamp is at the root level - adjust path as needed
                config["timestamp"] = Value::String(format!("0x{:x}", timestamp));
            }

            serde_json::to_string_pretty(&genesis).unwrap()
        };

        let mut copy_to_sources = vec![
            CopyToContainer::new(
                // std::fs::read(&self.jwt_secret).unwrap(),
                "688f5d737bad920bdfb2fc2f488d6b6209eebda1dae949a8de91398d932c517a"
                    .to_string()
                    .into_bytes(),
                "/jwt_secret.hex".to_string(),
            ),
            CopyToContainer::new(genesis.into_bytes(), "/genesis.json".to_string()),
        ];

        if let Some(p2p_secret) = &self.p2p_secret {
            copy_to_sources.push(CopyToContainer::new(
                // std::fs::read(p2p_secret).unwrap(),
                "a11ac89899cd86e36b6fb881ec1255b8a92a688790b7d950f8b7d8dd626671fb"
                    .to_string()
                    .into_bytes(),
                "/p2p_secret.hex".to_string(),
            ));
        }

        let expose_ports = vec![ContainerPort::Tcp(8551), ContainerPort::Tcp(30303)];

        OpRethImage {
            config: self,
            copy_to_sources,
            expose_ports,
        }
    }
}

impl OpRethImage {
    pub fn config(&self) -> &OpRethConfig {
        &self.config
    }
}

#[derive(Debug, Clone)]
pub struct OpRethImage {
    config: OpRethConfig,
    copy_to_sources: Vec<CopyToContainer>,
    expose_ports: Vec<ContainerPort>,
}

impl Image for OpRethImage {
    fn name(&self) -> &str {
        NAME
    }

    fn tag(&self) -> &str {
        TAG
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("Starting consensus")]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        &self.config.env_vars
    }

    fn copy_to_sources(&self) -> impl IntoIterator<Item = &CopyToContainer> {
        self.copy_to_sources.iter()
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        let mut cmd = vec![
            "node".to_string(),
            "--http".to_string(),
            "--http.addr=0.0.0.0".to_string(),
            "--authrpc.port=8551".to_string(),
            "--authrpc.addr=0.0.0.0".to_string(),
            "--authrpc.jwtsecret=/jwt_secret.hex".to_string(),
            "--chain=/genesis.json".to_string(),
            "--datadir".to_string(),
            self.config.datadir.clone(),
            "--port=30303".to_string(),
            "--color".to_string(),
            self.config.color.clone(),
        ];
        if self.config.p2p_secret.is_some() {
            cmd.push("--p2p-secret-key=/p2p_secret.hex".to_string());
        }
        if !self.config.trusted_peers.is_empty() {
            cmd.extend([
                "--trusted-peers".to_string(),
                self.config.trusted_peers.join(","),
            ]);
        }
        if self.config.disable_discovery {
            cmd.push("--disable-discovery".to_string());
        }
        if self.config.ipcdisable {
            cmd.push("--ipcdisable".to_string());
        }
        cmd
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.expose_ports
    }
}

pub trait OpRethMehods {
    async fn auth_rpc(&self) -> eyre::Result<Uri>;
    async fn auth_rpc_port(&self) -> eyre::Result<u16>;
    async fn enode(&self) -> eyre::Result<Uri>;
}

impl OpRethMehods for ContainerAsync<OpRethImage> {
    async fn auth_rpc_port(&self) -> eyre::Result<u16> {
        Ok(self.get_host_port_ipv4(AUTH_RPC_PORT).await?)
    }

    async fn auth_rpc(&self) -> eyre::Result<Uri> {
        Ok(format!(
            "http://{}:{}",
            self.get_host().await?,
            self.get_host_port_ipv4(AUTH_RPC_PORT).await?
        )
        .parse()?)
    }

    async fn enode(&self) -> eyre::Result<Uri> {
        Ok(format!(
            "enode://{}@{}:{}",
            L2_P2P_ENODE,
            self.get_host().await?,
            self.get_host_port_ipv4(P2P_PORT).await?
        )
        .parse()?)
    }
}
