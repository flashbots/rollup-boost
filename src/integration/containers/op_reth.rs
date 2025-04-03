use std::{
    borrow::Cow,
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use testcontainers::{CopyToContainer, Image, core::WaitFor};

use crate::integration::JWT_SECRET;

const NAME: &str = "ghcr.io/paradigmxyz/op-reth";
const TAG: &str = "v1.3.4";

#[derive(Debug, Clone)]
pub struct OpRethConfig {
    chain: PathBuf,
    jwt_secret: PathBuf,
    p2p_secret: Option<PathBuf>,
    pub trusted_peers: Vec<String>,
    pub authrpc_port: u16,
    pub datadir: String,
    pub disable_discovery: bool,
    pub p2p_port: u16,
    pub color: String,
    pub ipcdisable: bool,

    pub env_vars: HashMap<String, String>,
    copy_to_sources: Vec<CopyToContainer>,
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
            authrpc_port: 8551,
            datadir: "data".to_string(),
            disable_discovery: true,
            p2p_port: 30303,
            color: "never".to_string(),
            ipcdisable: true,

            env_vars: Default::default(),
            copy_to_sources: vec![],
        }
        .update_copy_to_sources()
    }
}

impl OpRethConfig {
    pub fn with_env_vars(
        mut self,
        env_vars: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.env_vars
            .extend(env_vars.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    pub fn set_trusted_peers(mut self, trusted_peers: Vec<String>) -> Self {
        self.trusted_peers = trusted_peers;
        self
    }

    fn update_copy_to_sources(mut self) -> Self {
        // Write the genesis file to the test directory and update the timestamp to the current time
        let genesis = {
            // Read the template file
            let template = include_str!("../testdata/genesis.json");

            // Parse the JSON
            let mut genesis: Value = serde_json::from_str(template).unwrap();

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

        self.copy_to_sources = vec![
            CopyToContainer::new(self.jwt_secret.clone(), "jwt_secret.hex".to_string()),
            CopyToContainer::new(genesis.into_bytes(), "genesis.json".to_string()),
        ];

        if let Some(p2p_secret) = &self.p2p_secret {
            self.copy_to_sources.push(CopyToContainer::new(
                p2p_secret.clone(),
                "p2p_secret.hex".to_string(),
            ));
        }

        self
    }

    pub fn set_jwt_secret(mut self, jwt_secret: PathBuf) -> Self {
        self.jwt_secret = jwt_secret;
        self.update_copy_to_sources()
    }

    pub fn set_p2p_secret(mut self, p2p_secret: Option<PathBuf>) -> Self {
        self.p2p_secret = p2p_secret;
        self.update_copy_to_sources()
    }
}

impl Image for OpRethConfig {
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
        &self.env_vars
    }

    fn copy_to_sources(&self) -> impl IntoIterator<Item = &CopyToContainer> {
        self.copy_to_sources.iter()
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        let mut cmd = vec![
            "node".to_string(),
            "--authrpc.port".to_string(),
            self.authrpc_port.to_string(),
            "--authrpc.jwtsecret=jwt_secret.hex".to_string(),
            "--chain=genesis.json".to_string(),
            "--datadir".to_string(),
            self.datadir.clone(),
            "--port".to_string(),
            self.p2p_port.to_string(),
            "--color".to_string(),
            self.color.clone(),
        ];
        if self.p2p_secret.is_some() {
            cmd.push("--p2p-secret-key=p2p_secret.hex".to_string());
        }
        if !self.trusted_peers.is_empty() {
            cmd.extend(["--trusted-peers".to_string(), self.trusted_peers.join(",")]);
        }
        if self.disable_discovery {
            cmd.push("--disable-discovery".to_string());
        }
        if self.ipcdisable {
            cmd.push("--ipcdisable".to_string());
        }
        cmd
    }
}
