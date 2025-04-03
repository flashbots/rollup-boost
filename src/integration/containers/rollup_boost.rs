use std::{
    fs::File,
    io::Write,
    sync::{Arc, Mutex},
};

use clap::Parser;
use http::Uri;
use tokio::task::JoinHandle;
use tracing::{Level, instrument::WithSubscriber as _};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
struct SharedFileWriter {
    file: Arc<Mutex<File>>,
}

impl<'a> MakeWriter<'a> for SharedFileWriter {
    type Writer = SharedFileGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SharedFileGuard {
            guard: self.file.lock().unwrap(), // you can handle poisoning if needed
        }
    }
}

pub struct SharedFileGuard<'a> {
    guard: std::sync::MutexGuard<'a, File>,
}

impl Write for SharedFileGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.guard.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.guard.flush()
    }
}

use crate::Args;

#[derive(Debug)]
pub struct RollupBoost {
    pub args: Args,
    pub log_file: Arc<Mutex<File>>,
    pub handle: JoinHandle<eyre::Result<()>>,
}

impl RollupBoost {
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
                "--l2-jwt-path=../testdata/jwt_secret.hex",
                "--builder-jwt-path=../testdata/jwt_secret.hex",
                "--tracing",
                "--metrics",
            ]),
        }
    }
}

impl RollupBoostConfig {
    pub fn start(self, log_file: File) -> RollupBoost {
        let log_file = Arc::new(Mutex::new(log_file));
        let writer = SharedFileWriter {
            file: log_file.clone(),
        };
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_env_filter(format!("rollup_boost={}", Level::TRACE))
            .finish();

        let handle = tokio::spawn(self.args.clone().run().with_subscriber(subscriber));

        RollupBoost {
            args: self.args,
            log_file,
            handle,
        }
    }
}
