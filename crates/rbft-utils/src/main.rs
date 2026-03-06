// SPDX-License-Identifier: Apache-2.0
//! RBFT Utilities
//!
//! This crate contains utility commands for the RBFT network:
//! - Genesis generation
//! - Testnet management
//! - Validator management

use alloy_primitives::Address;
use clap::Parser;
use rbft_utils::constants::DEFAULT_ADMIN_KEY;

mod genesis;
mod kube_namespace;
mod logjam;
mod testnet;
mod types;
mod validator_management;

#[derive(Debug, Parser)]
#[command(name = "rbft-utils")]
#[command(
    about = "RBFT network utilities for genesis, testnet, and validator management",
    long_about = None
)]
enum RbftCommands {
    /// Generate genesis configuration
    Genesis {
        /// Directory to write genesis.json and validator keys. Defaults to
        /// crates/rbft-node/assets
        #[arg(long)]
        assets_dir: Option<String>,

        /// Path to CSV file containing node data (validator keys, P2P keys, enodes).
        /// Required for genesis generation. Defaults to <assets_dir>/nodes.csv
        /// Generate this file first using the 'node-gen' command.
        /// The number of nodes is derived from the number of rows in the CSV file.
        #[arg(long, env = "RBFT_NODES_CSV")]
        nodes_csv: Option<String>,

        /// Number of initial validators to include in genesis contract.
        /// Must be <= total nodes in CSV. Remaining nodes can be added later.
        /// Defaults to all nodes in CSV.
        #[arg(long, env = "RBFT_INITIAL_NODES")]
        initial_nodes: Option<usize>,

        /// Validator contract address (default: 0x0000000000000000000000000000000000001001)
        #[arg(long, default_value = "0x0000000000000000000000000000000000001001")]
        validator_contract_address: Address,

        /// Gas limit for the genesis block (default: 600_000_000)
        #[arg(long, env = "RBFT_GAS_LIMIT")]
        gas_limit: Option<u64>,

        /// Block interval in seconds (default: 1.0)
        #[arg(long, default_value = "0.5", env = "RBFT_BLOCK_INTERVAL")]
        block_interval: f64,

        /// Epoch length in blocks (default: 32)
        #[arg(long, default_value = "32", env = "RBFT_EPOCH_LENGTH")]
        epoch_length: u64,

        /// Base fee per gas in wei (default: 4761904761905)
        #[arg(long, default_value = "4761904761905", env = "RBFT_BASE_FEE")]
        base_fee: u64,

        /// Maximum active validators (defaults to total number of nodes from CSV)
        #[arg(long, env = "RBFT_MAX_ACTIVE_VALIDATORS")]
        max_active_validators: Option<u64>,

        /// Admin private key for contract deployment (default: built-in test key)
        #[arg(
            long,
            env = "RBFT_ADMIN_KEY",
            default_value_t = DEFAULT_ADMIN_KEY.to_string()
        )]
        admin_key: String,

        /// Generate enodes for Docker environment (uses localhost and ports)
        #[arg(long, env = "RBFT_DOCKER")]
        docker: bool,

        /// Generate enodes for Kubernetes StatefulSet (pod DNS names)
        #[arg(long, env = "RBFT_KUBE")]
        kube: bool,

        /// Write enodes.txt file for trusted peers (use with traditional static peer setup)
        #[arg(long, env = "RBFT_USE_TRUSTED_PEERS")]
        use_trusted_peers: bool,
    },
    /// Run a testnet
    Testnet {
        /// Number of nodes to run in the test (default: 4)
        #[arg(short, long, default_value = "4", env = "RBFT_NUM_NODES")]
        num_nodes: u32,

        /// Base HTTP port to start from (default: 8545).
        #[arg(long, default_value = "8545")]
        base_http_port: u16,

        /// Delete the data directory before starting the testnet.
        #[arg(long)]
        init: bool,

        /// Directory for log files. Defaults to ~/.rbft/testnet/logs
        #[arg(long, env = "RBFT_LOGS_DIR")]
        logs_dir: Option<String>,

        /// Directory for database files. Defaults to ~/.rbft/testnet/db
        #[arg(long, env = "RBFT_DB_DIR")]
        db_dir: Option<String>,

        /// Extra arguments to pass to all nodes
        #[arg(long, allow_hyphen_values = true)]
        extra_args: Vec<String>,

        /// Directory containing genesis.json and validator keys. Defaults to
        /// crates/rbft-node/assets
        #[arg(long)]
        assets_dir: Option<String>,

        /// Monitor transaction pool sizes alongside block heights
        #[arg(long)]
        monitor_txpool: bool,

        /// Spawn "make megatx" concurrently to generate transaction load
        #[arg(long, env = "RBFT_RUN_MEGATX")]
        run_megatx: bool,

        /// Exit testnet after any node reaches this block height
        #[arg(long, env = "RBFT_EXIT_AFTER_BLOCK")]
        exit_after_block: Option<u64>,

        /// Comma-separated list of block heights to add validators at (e.g., "10,20,30")
        #[arg(long, env = "RBFT_ADD_AT_BLOCKS")]
        add_at_blocks: Option<String>,

        /// Number of initial validators (for tracking which validator to add next).
        /// Defaults to num_nodes if not specified.
        #[arg(long, env = "RBFT_INITIAL_NODES")]
        initial_nodes: Option<usize>,

        /// Run validators in Docker containers (megatx and monitoring stay on host)
        #[arg(long, env = "RBFT_DOCKER")]
        docker: bool,

        /// Run validators in Kubernetes cluster (megatx and monitoring stay on host)
        #[arg(long, env = "RBFT_KUBE")]
        kube: bool,

        /// Maximum log file size in MB before rotation (0=disabled)
        #[arg(long, default_value_t = 100, env = "RBFT_LOG_MAX_SIZE_MB")]
        log_max_size_mb: u64,

        /// Number of rotated log files to keep
        #[arg(long, default_value_t = 3, env = "RBFT_LOG_KEEP_ROTATED")]
        log_keep_rotated: usize,

        /// Admin private key for testnet operations (default: built-in test key)
        #[arg(
            long,
            env = "RBFT_ADMIN_KEY",
            default_value_t = DEFAULT_ADMIN_KEY.to_string()
        )]
        admin_key: String,
    },
    /// Tail and merge log files in chronological order
    Logjam {
        /// Directory for log files. Defaults to ~/.rbft/testnet/logs
        #[arg(long)]
        logs_dir: Option<String>,

        /// Continue to tail files as they grow (like tail -f)
        #[arg(short, long)]
        follow: bool,

        /// Maximum message delay in milliseconds before reporting as unreceived
        #[arg(
            short = 'm',
            long,
            default_value = "1000",
            env = "RBFT_LOGJAM_MAX_MESSAGE_DELAY"
        )]
        max_message_delay: u64,

        /// Histogram bucket size in milliseconds (default: max_message_delay / 10)
        #[arg(short = 'b', long, env = "RBFT_LOGJAM_HISTOGRAM_BUCKET_SIZE")]
        bucket_size: Option<u64>,

        /// Quiet mode: only print histogram and unreceived lines
        #[arg(short, long)]
        quiet: bool,

        /// Enable wire-level trace mode (requires RUST_LOG=msg_trace=debug on nodes)
        #[arg(short, long)]
        trace: bool,
    },
    /// Generate node enodes and secret keys in CSV format
    NodeGen {
        /// Number of nodes to generate (default: 4)
        #[arg(short, long, default_value = "4", env = "RBFT_NUM_NODES")]
        num_nodes: u32,

        /// Directory containing genesis.json and validator keys. Defaults to ~/.rbft/testnet
        #[arg(long)]
        assets_dir: Option<String>,

        /// Output CSV file path. Defaults to <assets_dir>/nodes.csv
        #[arg(long, env = "RBFT_NODES_CSV")]
        nodes_csv: Option<String>,

        /// Generate enodes for Docker environment (uses container hostnames)
        #[arg(long, env = "RBFT_DOCKER")]
        docker: bool,

        /// Generate enodes for Kubernetes StatefulSet (pod DNS names)
        #[arg(long, env = "RBFT_KUBE")]
        kube: bool,
    },
    /// Manage validators in the QBFTValidatorSet cqontract
    Validator {
        #[command(subcommand)]
        command: ValidatorCommand,
    },
}

#[derive(Debug, Parser)]
enum ValidatorCommand {
    /// Add a validator to the validator set
    Add {
        /// Admin private key (with authority to add validators)
        #[arg(long, env = "RBFT_ADMIN_KEY")]
        admin_key: String,

        /// Address of the validator to add
        #[arg(long)]
        validator_address: Address,

        /// Enode URL for the validator (e.g., enode://pubkey@ip:port)
        #[arg(long)]
        enode: String,

        /// JSON-RPC URL to connect to
        #[arg(long, default_value = "http://localhost:8545")]
        rpc_url: String,
    },
    /// Remove a validator from the validator set
    Remove {
        /// Admin private key (with authority to remove validators)
        #[arg(long, env = "RBFT_ADMIN_KEY")]
        admin_key: String,

        /// Address of the validator to remove
        #[arg(long)]
        validator_address: Address,

        /// JSON-RPC URL to connect to
        #[arg(long, default_value = "http://localhost:8545")]
        rpc_url: String,
    },
    /// Get the validator set status (max validators, base fee, validator list)
    Status {
        /// JSON-RPC URL to connect to
        #[arg(long, default_value = "http://localhost:8545")]
        rpc_url: String,
    },
    /// Set the maximum active validators value
    SetMaxActiveValidators {
        /// Admin private key (with authority to set values)
        #[arg(long, env = "RBFT_ADMIN_KEY")]
        admin_key: String,

        /// New maximum active validators value
        #[arg(long)]
        value: u64,

        /// JSON-RPC URL to connect to
        #[arg(long, default_value = "http://localhost:8545")]
        rpc_url: String,
    },
    /// Set the base fee value
    SetBaseFee {
        /// Admin private key (with authority to set values)
        #[arg(long, env = "RBFT_ADMIN_KEY")]
        admin_key: String,

        /// New base fee value
        #[arg(long)]
        value: u64,

        /// JSON-RPC URL to connect to
        #[arg(long, default_value = "http://localhost:8545")]
        rpc_url: String,
    },
    /// Set the block interval in milliseconds
    SetBlockIntervalMs {
        /// Admin private key (with authority to set values)
        #[arg(long, env = "RBFT_ADMIN_KEY")]
        admin_key: String,

        /// New block interval value in milliseconds
        #[arg(long)]
        value: u64,

        /// JSON-RPC URL to connect to
        #[arg(long, default_value = "http://localhost:8545")]
        rpc_url: String,
    },
    /// Set the epoch length in blocks
    SetEpochLength {
        /// Admin private key (with authority to set values)
        #[arg(long, env = "RBFT_ADMIN_KEY")]
        admin_key: String,

        /// New epoch length value in blocks
        #[arg(long)]
        value: u64,

        /// JSON-RPC URL to connect to
        #[arg(long, default_value = "http://localhost:8545")]
        rpc_url: String,
    },
}

fn main() -> eyre::Result<()> {
    let cli = RbftCommands::parse();

    match cli {
        RbftCommands::Genesis {
            assets_dir,
            nodes_csv,
            initial_nodes,
            validator_contract_address,
            gas_limit,
            block_interval,
            epoch_length,
            base_fee,
            max_active_validators,
            admin_key,
            docker,
            kube,
            use_trusted_peers,
        } => {
            if epoch_length == 0 {
                return Err(eyre::eyre!(
                    "Epoch length must be greater than 0, got {}",
                    epoch_length
                ));
            }
            genesis::create_rbft_genesis(
                assets_dir.as_ref().map(std::path::PathBuf::from),
                nodes_csv.as_ref().map(std::path::PathBuf::from),
                initial_nodes,
                Some(validator_contract_address),
                gas_limit,
                Some(block_interval),
                Some(epoch_length),
                Some(base_fee),
                max_active_validators,
                Some(admin_key),
                docker,
                kube,
                use_trusted_peers,
            );
            Ok(())
        }
        RbftCommands::Testnet {
            num_nodes,
            base_http_port,
            init,
            logs_dir,
            db_dir,
            extra_args,
            assets_dir,
            monitor_txpool,
            run_megatx,
            exit_after_block,
            add_at_blocks,
            initial_nodes,
            docker,
            kube,
            log_max_size_mb,
            log_keep_rotated,
            admin_key,
        } => {
            if num_nodes < 4 {
                return Err(eyre::eyre!(
                    "RBFT consensus requires at least 4 validators, got {}",
                    num_nodes
                ));
            }
            if docker && kube {
                return Err(eyre::eyre!(
                    "Cannot use both --docker and --kube flags simultaneously"
                ));
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(testnet::testnet(
                num_nodes,
                base_http_port,
                init,
                logs_dir.as_deref(),
                db_dir.as_deref(),
                if extra_args.is_empty() {
                    None
                } else {
                    Some(&extra_args)
                },
                assets_dir.as_ref().map(std::path::PathBuf::from),
                monitor_txpool,
                run_megatx,
                exit_after_block,
                add_at_blocks.as_deref(),
                initial_nodes,
                docker,
                kube,
                log_max_size_mb,
                log_keep_rotated,
                admin_key,
            ))
        }
        RbftCommands::Logjam {
            logs_dir,
            follow,
            max_message_delay,
            bucket_size,
            quiet,
            trace,
        } => {
            let bucket_size = bucket_size.unwrap_or(max_message_delay / 10);
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(logjam::logjam(
                logs_dir.as_deref(),
                follow,
                max_message_delay,
                bucket_size,
                quiet,
                trace,
            ))
        }
        RbftCommands::NodeGen {
            num_nodes,
            assets_dir,
            nodes_csv,
            docker,
            kube,
        } => genesis::generate_nodes_csv(
            num_nodes,
            assets_dir.as_ref().map(std::path::PathBuf::from),
            nodes_csv.as_deref(),
            docker,
            kube,
        ),
        RbftCommands::Validator { command } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                match command {
                    ValidatorCommand::Add {
                        admin_key,
                        validator_address,
                        enode,
                        rpc_url,
                    } => {
                        validator_management::add_validator(
                            &admin_key,
                            validator_address,
                            &enode,
                            &rpc_url,
                        )
                        .await
                    }
                    ValidatorCommand::Remove {
                        admin_key,
                        validator_address,
                        rpc_url,
                    } => {
                        validator_management::remove_validator(
                            &admin_key,
                            validator_address,
                            &rpc_url,
                        )
                        .await
                    }
                    ValidatorCommand::Status { rpc_url } => {
                        validator_management::get_validator_status(&rpc_url)
                            .await
                            .map(|_| ())
                    }
                    ValidatorCommand::SetMaxActiveValidators {
                        admin_key,
                        value,
                        rpc_url,
                    } => {
                        validator_management::set_max_active_validators(&admin_key, value, &rpc_url)
                            .await
                    }
                    ValidatorCommand::SetBaseFee {
                        admin_key,
                        value,
                        rpc_url,
                    } => validator_management::set_base_fee(&admin_key, value, &rpc_url).await,
                    ValidatorCommand::SetBlockIntervalMs {
                        admin_key,
                        value,
                        rpc_url,
                    } => {
                        validator_management::set_block_interval_ms(&admin_key, value, &rpc_url)
                            .await
                    }
                    ValidatorCommand::SetEpochLength {
                        admin_key,
                        value,
                        rpc_url,
                    } => validator_management::set_epoch_length(&admin_key, value, &rpc_url).await,
                }
            })
        }
    }
}
