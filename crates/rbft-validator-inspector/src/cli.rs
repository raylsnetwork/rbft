// SPDX-License-Identifier: Apache-2.0
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "validator-inspector",
    version,
    about = "Interactive RBFT validator inspector"
)]
pub struct Cli {
    /// Comma-separated list of label=url pairs (e.g. v0=http://127.0.0.1:8545)
    #[arg(
        long = "rpc",
        env = "RPC_URLS",
        value_delimiter = ',',
        value_name = "LABEL=URL"
    )]
    pub rpc_entries: Vec<String>,
    /// Maximum number of validators to track
    #[arg(long, default_value_t = 15)]
    pub max_validators: usize,
    /// Refresh interval in seconds
    #[arg(long, default_value_t = 1)]
    pub refresh_secs: u64,
    /// Command template to start a validator (use {label}, {url}, {port})
    #[arg(long, env = "VALIDATOR_START_CMD")]
    pub start_cmd: Option<String>,
    /// Command template to stop a validator (use {label}, {url}, {port})
    #[arg(long, env = "VALIDATOR_STOP_CMD")]
    pub stop_cmd: Option<String>,
    /// Command template to restart a validator (fallback to stop+start if unset)
    #[arg(long, env = "VALIDATOR_RESTART_CMD")]
    pub restart_cmd: Option<String>,
    /// Foundry project directory that contains the automation DSL
    #[arg(long, env = "AUTOMATION_PROJECT")]
    pub automation_project: Option<PathBuf>,
    /// Directory containing the rbft2 / rbft-node2 executable
    #[arg(long, env = "RBFT_BIN_DIR", value_name = "DIR", required = true)]
    pub rbft_bin_dir: PathBuf,
}
