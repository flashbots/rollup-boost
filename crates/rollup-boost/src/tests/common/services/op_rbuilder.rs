use http::Uri;
use std::{borrow::Cow, collections::HashMap, path::PathBuf};
use testcontainers::{
    ContainerAsync, CopyToContainer, Image,
    core::{ContainerPort, WaitFor},
};

use crate::tests::common::TEST_DATA;

const NAME: &str = "ghcr.io/flashbots/op-rbuilder";
const TAG: &str = "sha-4f1931b"; // temporal

pub const AUTH_RPC_PORT: u16 = 8551;
pub const P2P_PORT: u16 = 30303;
pub const FLASHBLOCKS_PORT: u16 = 1112;

#[derive(Debug, Clone)]
pub struct OpRbuilderConfig {
    jwt_secret: PathBuf,
    p2p_secret: Option<PathBuf>,
    pub trusted_peers: Vec<String>,
    pub color: String,
    pub ipcdisable: bool,
    pub env_vars: HashMap<String, String>,
    pub genesis: Option<String>,
    flashblocks: bool,
}

impl Default for OpRbuilderConfig {
    fn default() -> Self {
        Self {
            jwt_secret: PathBuf::from(format!("{}/jwt_secret.hex", *TEST_DATA)),
            p2p_secret: None,
            trusted_peers: vec![],
            color: "never".to_string(),
            ipcdisable: true,
            env_vars: Default::default(),
            genesis: None,
            flashblocks: false,
        }
    }
}

impl OpRbuilderConfig {
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

    pub fn set_genesis(mut self, genesis: String) -> Self {
        self.genesis = Some(genesis);
        self
    }

    pub fn set_flashblocks(mut self, flashblocks: bool) -> Self {
        self.flashblocks = flashblocks;
        self
    }

    pub fn build(self) -> eyre::Result<OpRbuilderImage> {
        let genesis = self
            .genesis
            .clone()
            .ok_or_else(|| eyre::eyre!("Genesis configuration not found"))?;

        let mut copy_to_sources = vec![
            CopyToContainer::new(
                std::fs::read_to_string(&self.jwt_secret)?.into_bytes(),
                "/jwt_secret.hex".to_string(),
            ),
            CopyToContainer::new(genesis.into_bytes(), "/genesis.json".to_string()),
        ];

        if let Some(p2p_secret) = &self.p2p_secret {
            let p2p_string = std::fs::read_to_string(p2p_secret)
                .unwrap()
                .replace("\n", "");
            copy_to_sources.push(CopyToContainer::new(
                p2p_string.into_bytes(),
                "/p2p_secret.hex".to_string(),
            ));
        }

        let expose_ports = vec![];

        Ok(OpRbuilderImage {
            config: self,
            copy_to_sources,
            expose_ports,
        })
    }
}

impl OpRbuilderImage {
    pub fn config(&self) -> &OpRbuilderConfig {
        &self.config
    }
}

#[derive(Debug, Clone)]
pub struct OpRbuilderImage {
    config: OpRbuilderConfig,
    copy_to_sources: Vec<CopyToContainer>,
    expose_ports: Vec<ContainerPort>,
}

impl Image for OpRbuilderImage {
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
            "--port=30303".to_string(),
            "--addr=0.0.0.0".to_string(),
            "--http".to_string(),
            "--http.addr=0.0.0.0".to_string(),
            "--http.api=eth,net,web3,debug,miner".to_string(),
            "--authrpc.port=8551".to_string(),
            "--authrpc.addr=0.0.0.0".to_string(),
            "--authrpc.jwtsecret=/jwt_secret.hex".to_string(),
            "--chain=/genesis.json".to_string(),
            "-vvv".to_string(),
            "--disable-discovery".to_string(),
            "--color".to_string(),
            self.config.color.clone(),
        ];
        if self.config.flashblocks {
            cmd.extend([
                "--flashblocks.enabled".to_string(),
                "--flashblocks.addr=0.0.0.0".to_string(),
                "--flashblocks.port=1112".to_string(),
            ]);
        }
        if self.config.p2p_secret.is_some() {
            cmd.push("--p2p-secret-key=/p2p_secret.hex".to_string());
        }
        if !self.config.trusted_peers.is_empty() {
            println!("Trusted peers: {:?}", self.config.trusted_peers);
            cmd.extend([
                "--trusted-peers".to_string(),
                self.config.trusted_peers.join(","),
            ]);
        }
        if self.config.ipcdisable {
            cmd.push("--ipcdisable".to_string());
        }

        println!("cmd: {}", cmd.join(" "));

        cmd
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &self.expose_ports
    }
}

pub trait OpRbuilderMehods {
    async fn auth_rpc(&self) -> eyre::Result<Uri>;
    async fn auth_rpc_port(&self) -> eyre::Result<u16>;
}

impl OpRbuilderMehods for ContainerAsync<OpRbuilderImage> {
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
}
