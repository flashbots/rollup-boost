use std::{
    fs::File,
    io::Write,
    sync::{Arc, Mutex},
};

use clap::Parser;
use tokio::task::JoinHandle;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
struct SharedFileWriter {
    file: Arc<Mutex<File>>,
}

impl<'a> MakeWriter<'a> for SharedFileWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for SharedFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        println!("Writing to file...");
        self.file.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        println!("Flushing file...");
        self.file.lock().unwrap().flush()
    }
}

use crate::Args;

#[derive(Debug)]
pub struct RollupBoost {
    pub args: Args,
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
    pub fn start(self) -> RollupBoost {
        let handle = tokio::spawn(self.args.clone().run());

        RollupBoost {
            args: self.args,
            handle,
        }
    }
}
