// SPDX-License-Identifier: Apache-2.0
//! End-to-end test for producer and follower scripts using Rust system calls.

mod docker;
mod kube;

use crate::kube_namespace::default_kube_namespace;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer_local::PrivateKeySigner;
use rbft_utils::constants::DEFAULT_ADMIN_KEY;
use serde_json::{json, Value};
use std::fs;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::sleep;

/// Get RSS (Resident Set Size) in KB for a process by PID.
/// Returns None if the process doesn't exist or RSS can't be read.
fn get_rss_kb(pid: u32) -> Option<u64> {
    let status_path = format!("/proc/{}/status", pid);
    let file = fs::File::open(status_path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line.ok()?;
        if line.starts_with("VmRSS:") {
            // Format: "VmRSS:     12345 kB"
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return parts[1].parse().ok();
            }
        }
    }
    None
}

async fn wait_for_kube_pod_ready(
    pod_name: &str,
    namespace: &str,
    timeout: Duration,
) -> eyre::Result<()> {
    let start = std::time::Instant::now();
    let mut last_phase = String::new();
    let mut last_error = String::new();

    loop {
        let output = std::process::Command::new("kubectl")
            .args([
                "get",
                "pod",
                pod_name,
                "-n",
                namespace,
                "-o",
                "jsonpath={.status.phase}",
            ])
            .output()?;

        if output.status.success() {
            let phase = String::from_utf8_lossy(&output.stdout);
            let phase = phase.trim();
            last_phase = phase.to_string();

            if phase == "Running" {
                let ready_output = std::process::Command::new("kubectl")
                    .args([
                        "get",
                        "pod",
                        pod_name,
                        "-n",
                        namespace,
                        "-o",
                        "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}",
                    ])
                    .output()?;

                if ready_output.status.success() {
                    let ready = String::from_utf8_lossy(&ready_output.stdout);
                    if ready.trim() == "True" {
                        return Ok(());
                    }
                } else {
                    last_error = String::from_utf8_lossy(&ready_output.stderr)
                        .trim()
                        .to_string();
                }
            }
        } else {
            last_error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        }

        if start.elapsed() > timeout {
            let summary = if !last_phase.is_empty() {
                format!("last phase: {}", last_phase)
            } else if !last_error.is_empty() {
                format!("last error: {}", last_error)
            } else {
                "no status returned".to_string()
            };
            return Err(eyre::eyre!(
                "Timed out waiting for pod {} to be ready ({})",
                pod_name,
                summary
            ));
        }

        sleep(Duration::from_secs(2)).await;
    }
}

// ANSI color codes
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// Format RSS value in human-readable format with color based on memory pressure.
/// Green: < 1GB, Yellow: 1-2GB, Red: > 2GB
fn format_rss(kb: u64) -> String {
    let color = if kb >= 2_097_152 {
        RED // > 2GB
    } else if kb >= 1_048_576 {
        YELLOW // 1-2GB
    } else {
        GREEN // < 1GB
    };

    let value = if kb >= 1_048_576 {
        format!("{:.1}G", kb as f64 / 1_048_576.0)
    } else if kb >= 1024 {
        format!("{:.0}M", kb as f64 / 1024.0)
    } else {
        format!("{}K", kb)
    };

    format!("{}{}{}", color, value, RESET)
}

/// Format txpool size with color based on pressure (max 25000).
/// Green: < 50%, Yellow: 50-80%, Red: > 80%
fn format_txpool(size: u64) -> String {
    let color = if size >= 20000 {
        RED // > 80%
    } else if size >= 12500 {
        YELLOW // 50-80%
    } else {
        GREEN // < 50%
    };

    format!("{}{}{}", color, size, RESET)
}

/// Create a custom reth config file with increased session buffer sizes.
/// This prevents "session command buffer full, dropping message" errors
/// during high transaction throughput.
fn create_node_config(data_dir: &Path) -> PathBuf {
    let config_path = data_dir.join("reth-config.toml");
    let config_content = r#"# Custom RBFT node configuration
# Tuned for high-throughput transaction processing

[sessions]
# Default is 32, increased to handle extreme transaction bursts
session_command_buffer = 393216
# Default is 260, increased for better event handling
session_event_buffer = 393216
"#;
    let mut file = fs::File::create(&config_path).expect("failed to create config file");
    file.write_all(config_content.as_bytes())
        .expect("failed to write config file");
    config_path
}

/// Determine data and logs directory for test runs.
///
/// Priority: CLI options -> RBFT_TESTNET_DIR env var -> ~/.rbft/testnet defaults
fn test_dirs(
    name: &str,
    cli_db_dir: Option<PathBuf>,
    cli_logs_dir: Option<PathBuf>,
) -> (PathBuf, PathBuf) {
    if let (Some(db), Some(logs)) = (cli_db_dir, cli_logs_dir) {
        (db, logs)
    } else {
        let home = std::env::var("HOME").expect("HOME environment variable not set");
        let base = PathBuf::from(home).join(".rbft").join(name);
        (base.join("db"), base.join("logs"))
    }
}

fn resolve_dir(dir: &str) -> PathBuf {
    let path = PathBuf::from(dir);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .expect("Failed to retrieve current directory")
            .join(path)
    }
}

/// Helper to run a command and return the child process.
fn spawn_command(cmd: &str, log: &Path) -> Child {
    eprintln!("----- Spawning command: {} ----", cmd);
    let mut parts = cmd.split_whitespace();
    let program = parts.next().expect("command string must not be empty");
    let args: Vec<&str> = parts.collect();
    // let mut out = std::fs::File::create(log).unwrap();
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent).expect("failed to create log directory");
    }
    let mut out = OpenOptions::new()
        .create(true) // create if it doesn't exist
        .append(true) // append if it does
        .open(log)
        .expect("failed to open log file");

    use std::io::Write;
    writeln!(out, "# {cmd}").expect("failed to write to log file");
    writeln!(out).expect("failed to write to log file");

    let mut command = Command::new(program);
    command.args(&args);

    // Always set RUST_LOG_STYLE to never for cleaner logs
    command.env("RUST_LOG_STYLE", "never");

    command
        .stdout(std::process::Stdio::from(out))
        // .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to start process")
}

fn _data_dir(tmp_dir: &Path, name: &str) -> String {
    let dir = tmp_dir.join(name);
    // if dir.exists() {
    //     fs::remove_dir_all(&dir).unwrap();
    // }
    dir.to_str()
        .expect("data dir path contains invalid UTF-8")
        .to_string()
}

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

fn ensure_validator_assets(num_nodes: u32) -> PathBuf {
    let assets = assets_dir();
    let have_all_keys = (0..num_nodes).all(|i| {
        assets.join(format!("validator-key{i}.txt")).exists()
            && assets.join(format!("p2p-secret-key{i}.txt")).exists()
    });
    if !have_all_keys
        || !assets.join("enodes.txt").exists()
        || !assets.join("genesis.json").exists()
    {
        panic!(
            "Missing genesis assets in {}. Please run 'make testnet_start' to generate them.",
            assets.display()
        );
    }
    assets
}

#[allow(clippy::too_many_arguments)]
fn start_new_validator(
    index: u32,
    num_nodes: u32,
    base_http_port: u16,
    assets: &Path,
    trusted_peers: &str,
    current_exe: &Path,
    data_dir_path: &Path,
    logs_dir_path: &Path,
    extra_args: Option<&[String]>,
    docker: bool,
    is_follower: bool,
) -> (Child, String) {
    let db_path = data_dir_path.join(format!("d{}", index));
    fs::create_dir_all(&db_path).expect("failed to create node datadir");
    let db = db_path
        .to_str()
        .expect("failed to convert datadir to string")
        .to_string();
    let log_path = logs_dir_path.join(format!("node{}.log", index));
    let ipc_path = db_path.join("reth.ipc");
    let _ = fs::remove_file(&ipc_path);

    // Create custom config with increased session buffers
    let config_path = create_node_config(&db_path);

    // Port allocation strategy for up to 100 nodes:
    // - P2P ports: 30303 + index (30303-30402 for 100 nodes)
    // - HTTP ports: base_http_port + index (8545-8644 for 100 nodes with default base)
    // - AuthRPC ports: 10000 + index (10000-10099 for 100 nodes)
    //
    // AuthRPC is required by reth even though RBFT has embedded consensus.
    // We allocate it starting at 10000 to avoid conflicts with HTTP ports.
    let port = 30303 + index;
    let http_port = base_http_port
        .checked_add(index as u16)
        .expect("HTTP port overflow");
    let authrpc_port = 10000 + index;

    // Warn if trying to run more than 100 nodes (not tested)
    if num_nodes > 100 {
        eprintln!(
            "Warning: Port allocation tested for up to 100 nodes, you have {}",
            num_nodes
        );
    }

    let p2p_key = assets.join(format!("p2p-secret-key{index}.txt"));
    let validator_key = assets.join(format!("validator-key{index}.txt"));
    let genesis_path = assets.join("genesis.json");

    let node_cmd = if docker {
        // Build docker run command
        let mut cmd = vec!["docker".to_string(), "run".to_string()];

        // Remove old container if it exists
        let container_name = format!("rbft-node-testnet-{}", index);
        cmd.push("--rm".to_string());
        cmd.push("--name".to_string());
        cmd.push(container_name.clone());

        // Use host networking so containers can connect to 127.0.0.1 addresses
        cmd.push("--network".to_string());
        cmd.push("host".to_string());

        // Mount assets directory (read-only for genesis and keys)
        cmd.push("-v".to_string());
        cmd.push(format!(
            "{}:/assets:ro",
            assets.to_str().expect("assets path contains invalid UTF-8")
        ));

        // Mount data directory (read-write)
        cmd.push("-v".to_string());
        cmd.push(format!(
            "{}:/data",
            db_path.to_str().expect("db path contains invalid UTF-8")
        ));

        // Mount logs directory (read-write)
        cmd.push("-v".to_string());
        cmd.push(format!(
            "{}:/logs",
            logs_dir_path
                .to_str()
                .expect("logs dir path contains invalid UTF-8")
        ));

        // Image name
        cmd.push("rbft-node:testnet".to_string());

        // Now add the actual node command arguments
        cmd.push("./rbft-node".to_string());
        cmd.push("node".to_string());
        cmd.push("--http".to_string());
        cmd.push("--http.port".to_string());
        cmd.push(http_port.to_string()); // Use actual host port
        cmd.push("--http.addr".to_string());
        cmd.push("0.0.0.0".to_string());
        cmd.push("--http.corsdomain".to_string());
        cmd.push("*".to_string());
        cmd.push("--http.api".to_string());
        cmd.push("eth,txpool".to_string());
        cmd.push("--chain".to_string());
        cmd.push("/assets/genesis.json".to_string());
        cmd.push("--config".to_string());
        cmd.push("/data/reth-config.toml".to_string());
        cmd.push("--p2p-secret-key".to_string());
        cmd.push(format!("/assets/p2p-secret-key{}.txt", index));
        if !is_follower {
            cmd.push("--validator-key".to_string());
            cmd.push(format!("/assets/validator-key{}.txt", index));
        }
        if num_nodes != 1 && !trusted_peers.is_empty() {
            cmd.push("--trusted-peers".to_string());
            cmd.push(trusted_peers.to_string());
        }
        cmd.push("--datadir".to_string());
        cmd.push("/data".to_string());
        cmd.push("--ipcpath".to_string());
        cmd.push("/data/reth.ipc".to_string());
        cmd.push("--authrpc.port".to_string());
        cmd.push(authrpc_port.to_string());
        cmd.push("--port".to_string());
        cmd.push(port.to_string()); // Use actual host port
        cmd.push("--disable-discovery".to_string());

        // Add extra arguments if provided (split on whitespace)
        if let Some(extra) = extra_args {
            for arg in extra {
                // Split each argument on whitespace to handle cases like "--flag value"
                for part in arg.split_whitespace() {
                    cmd.push(part.to_string());
                }
            }
        }

        cmd
    } else {
        // Build regular command
        let mut cmd = vec![current_exe
            .to_str()
            .expect("failed to convert current exe to string")
            .to_string()];
        cmd.push("node".to_string());
        cmd.push("--http".to_string());
        cmd.push(format!("--http.port {http_port}"));
        cmd.push("--http.addr 0.0.0.0".to_string());
        cmd.push("--http.corsdomain".to_string());
        cmd.push("*".to_string());
        cmd.push("--http.api".to_string());
        cmd.push("eth,txpool".to_string());
        cmd.push(format!(
            "--chain {}",
            genesis_path
                .to_str()
                .expect("failed to convert genesis path to string")
        ));
        cmd.push(format!(
            "--config {}",
            config_path
                .to_str()
                .expect("failed to convert config path to string")
        ));
        cmd.push(format!(
            "--p2p-secret-key {}",
            p2p_key
                .to_str()
                .expect("failed to convert p2p key path to string")
        ));
        if !is_follower {
            cmd.push(format!(
                "--validator-key {}",
                validator_key
                    .to_str()
                    .expect("failed to convert validator key path to string")
            ));
        }
        if num_nodes != 1 && !trusted_peers.is_empty() {
            cmd.push(format!("--trusted-peers {trusted_peers}"));
        }
        cmd.push(format!("--datadir {db}"));
        cmd.push(format!("--ipcpath {}", ipc_path.to_string_lossy()));
        cmd.push(format!("--authrpc.port {authrpc_port}"));
        cmd.push(format!("--port {port}"));
        cmd.push("--disable-discovery".to_string());

        // Add extra arguments if provided (split on whitespace)
        if let Some(extra) = extra_args {
            for arg in extra {
                // Split each argument on whitespace to handle cases like "--flag value"
                for part in arg.split_whitespace() {
                    cmd.push(part.to_string());
                }
            }
        }

        cmd
    };

    let node_cmd = node_cmd.join(" ");
    let node = spawn_command(&node_cmd, &log_path);

    (node, format!("http://localhost:{http_port}"))
}

pub(crate) async fn _stop_validator(node: &mut Child) -> eyre::Result<()> {
    // Start with an abrupt kill so peers see the node disappear without a clean shutdown.
    match node.start_kill() {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::InvalidInput => return Ok(()),
        Err(err) => return Err(err.into()),
    }
    // Reap the process to avoid zombies; ignore exit status.
    let _ = node.wait().await;
    Ok(())
}

/// Get transaction pool status via raw RPC call
async fn get_txpool_status<P: Provider>(provider: &P) -> eyre::Result<u64> {
    // Use raw provider to make the RPC call
    let response: Value = provider
        .raw_request("txpool_status".into(), json!([]))
        .await
        .map_err(|e| eyre::eyre!("Failed to call txpool_status: {}", e))?;

    // Parse the response to get the pending count
    // Expected response format: {"pending": "0x1a", "queued": "0x0"}
    let pending_hex = response
        .get("pending")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("Missing 'pending' field in txpool_status response"))?;

    // Parse hex string to u64
    let pending_count = u64::from_str_radix(pending_hex.trim_start_matches("0x"), 16)
        .map_err(|e| eyre::eyre!("Failed to parse pending count '{}': {}", pending_hex, e))?;

    Ok(pending_count)
}

/// Rotate log files using a copy-truncate strategy.
///
/// For each `nodeN.log` in `logs_dir` (where N ranges from 0 to `num_nodes - 1`):
/// - Skip files smaller than `max_size_bytes`.
/// - Shift existing rotated files (`.1` → `.2`, etc.), deleting the oldest if it exceeds
///   `keep_rotated`.
/// - Copy the current log to `.1`, then truncate the original via `set_len(0)`.
///
/// The child process keeps writing to the same fd, so no restart is needed.
fn rotate_log_files(logs_dir: &Path, num_nodes: u32, max_size_bytes: u64, keep_rotated: usize) {
    for i in 0..num_nodes {
        let log_path = logs_dir.join(format!("node{}.log", i));
        let meta = match fs::metadata(&log_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() < max_size_bytes {
            continue;
        }

        // Shift existing rotated files: .3 → delete, .2 → .3, .1 → .2
        for j in (1..=keep_rotated).rev() {
            let src = if j == 1 {
                log_path.clone()
            } else {
                logs_dir.join(format!("node{}.log.{}", i, j - 1))
            };
            let dst = logs_dir.join(format!("node{}.log.{}", i, j));
            if j == keep_rotated {
                // Delete the oldest rotated file to make room
                let _ = fs::remove_file(&dst);
            }
            if j > 1 {
                // Rename intermediate rotated files
                let _ = fs::rename(&src, &dst);
            }
        }

        // Copy current log to .1, then truncate
        let rotated_path = logs_dir.join(format!("node{}.log.1", i));
        if fs::copy(&log_path, &rotated_path).is_ok() {
            if let Ok(file) = OpenOptions::new().write(true).open(&log_path) {
                let _ = file.set_len(0);
                eprintln!(
                    "Rotated node{}.log ({:.1} MB)",
                    i,
                    meta.len() as f64 / 1_048_576.0
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn testnet(
    num_nodes: u32,
    base_http_port: u16,
    init: bool,
    logs_dir: Option<&str>,
    db_dir: Option<&str>,
    extra_args: Option<&[String]>,
    assets_dir: Option<PathBuf>,
    monitor_txpool: bool,
    run_megatx: bool,
    exit_after_block: Option<u64>,
    add_at_blocks: Option<&str>,
    add_followers_at: Option<&str>,
    initial_nodes: Option<usize>,
    docker: bool,
    kube: bool,
    log_max_size_mb: u64,
    log_keep_rotated: usize,
    admin_key: String,
) -> eyre::Result<()> {
    // Record start time for calculating total running time
    let start_time = std::time::Instant::now();

    let cli_db_dir = db_dir.map(resolve_dir);
    let cli_logs_dir = logs_dir.map(resolve_dir);
    let (data_dir_path, logs_dir_path) = test_dirs("testnet", cli_db_dir, cli_logs_dir);

    // Create the parent directories
    std::fs::create_dir_all(data_dir_path.parent().unwrap_or(&data_dir_path))
        .expect("Failed to create parent directory for data");
    std::fs::create_dir_all(logs_dir_path.parent().unwrap_or(&logs_dir_path))
        .expect("Failed to create parent directory for logs");

    if init {
        eprintln!(
            "Initializing testnet data dir in {} and logs in {}",
            data_dir_path.display(),
            logs_dir_path.display()
        );
        // Clean up Docker containers first to avoid locked directories
        if docker {
            docker::cleanup_containers();
        }
        // Clean up Kubernetes resources if in kube mode
        if kube {
            let kube_namespace = default_kube_namespace();
            let _ = kube::cleanup_testnet(&kube_namespace);
        }
        if data_dir_path.exists() {
            if docker {
                // Use Docker to remove root-owned files
                docker::cleanup_directory(&data_dir_path)?;
            } else {
                // Try regular removal first
                match fs::remove_dir_all(&data_dir_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        // Permission denied - likely root-owned files from previous Docker run
                        eprintln!(
                            "Permission denied removing {}, trying Docker cleanup...",
                            data_dir_path.display()
                        );
                        docker::cleanup_directory(&data_dir_path)?;
                    }
                    Err(e) => {
                        return Err(eyre::eyre!(
                            "Failed to remove data directory {}. Error: {}",
                            data_dir_path.display(),
                            e
                        ));
                    }
                }
            }
        }
        std::fs::create_dir_all(&data_dir_path).expect("failed to create test data dir");
        if logs_dir_path.exists() {
            if docker {
                // Use Docker to remove root-owned files
                docker::cleanup_directory(&logs_dir_path)?;
            } else {
                // Try regular removal first
                match fs::remove_dir_all(&logs_dir_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        // Permission denied - likely root-owned files from previous Docker run
                        eprintln!(
                            "Permission denied removing {}, trying Docker cleanup...",
                            logs_dir_path.display()
                        );
                        docker::cleanup_directory(&logs_dir_path)?;
                    }
                    Err(e) => {
                        return Err(eyre::eyre!(
                            "Failed to remove logs directory {}. Error: {}",
                            logs_dir_path.display(),
                            e
                        ));
                    }
                }
            }
        }
        std::fs::create_dir_all(&logs_dir_path).expect("failed to create test logs dir");
    } else {
        eprintln!(
            "Testnet already initialized in {} and {}",
            data_dir_path.display(),
            logs_dir_path.display()
        );
    }

    let assets = assets_dir.unwrap_or_else(|| ensure_validator_assets(num_nodes));
    let kube_namespace = default_kube_namespace();

    let trusted_peers = fs::read_to_string(assets.join("enodes.txt"))
        .unwrap_or_default()
        .replace("\n", "");

    // Find the rbft-node binary in the same directory as the current executable
    let current_exe = std::env::current_exe().expect("failed to get current executable path");
    let exe_dir = current_exe
        .parent()
        .expect("executable has no parent directory");
    let rbft_node_exe = exe_dir.join("rbft-node");

    // Build Docker image if docker mode is enabled
    if docker {
        docker::build_image("rbft-node:testnet")?;
    }

    // Build and deploy to Kubernetes if kube mode is enabled
    if kube {
        let image = docker::prepare_kube_image(&assets)?;
        eprintln!(
            "Using image {} (tag {}, digest {})",
            image.registry_image, image.tag, image.digest
        );
        kube::create_configmap(&assets, num_nodes, &kube_namespace)?;
        kube::deploy_statefulset(
            num_nodes,
            extra_args,
            &kube_namespace,
            &image.registry_image,
        )?;
    }

    // Start the nodes
    let mut nodes = Vec::new();
    let mut _port_forwards = Vec::new(); // For Kubernetes port-forward processes (kept alive)

    let mut urls = Vec::new();

    // In Kubernetes mode, we don't start local processes
    if !kube {
        for i in 0..num_nodes {
            let (node, url) = start_new_validator(
                i,
                num_nodes,
                base_http_port,
                &assets,
                &trusted_peers,
                rbft_node_exe.as_path(),
                &data_dir_path,
                &logs_dir_path,
                extra_args,
                docker,
                false,
            );
            nodes.push(node);
            urls.push(url);
        }
    } else {
        // In Kubernetes mode, verify pods are ready before port-forwarding
        eprintln!("Verifying Kubernetes pods are ready...");

        // Check each pod individually
        for i in 0..num_nodes {
            let pod_name = format!("rbft-node-{}", i);
            wait_for_kube_pod_ready(&pod_name, &kube_namespace, Duration::from_secs(600)).await?;
            eprintln!("  ✓ Pod {} is ready", pod_name);
        }

        eprintln!("Setting up port forwarding to Kubernetes pods...");

        for i in 0..num_nodes {
            let local_port = base_http_port.saturating_sub(i as u16);
            let pod_name = format!("rbft-node-{}", i);

            eprintln!(
                "Starting port-forward for {} on port {}...",
                pod_name, local_port
            );

            // Start port-forward in background
            let port_forward_result = std::process::Command::new("kubectl")
                .args([
                    "port-forward",
                    &format!("pod/{}", pod_name),
                    &format!("{}:8545", local_port),
                    "-n",
                    &kube_namespace,
                ])
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .spawn();

            match port_forward_result {
                Ok(child) => {
                    eprintln!("  ✓ Port-forward process started (PID: {:?})", child.id());
                    _port_forwards.push(child);
                }
                Err(e) => {
                    eprintln!("  ✗ Failed to start port-forward: {}", e);
                }
            }

            urls.push(format!("http://localhost:{}", local_port));

            // Small delay between port-forwards to let them establish
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        // Wait for port forwards to establish with health checks
        eprintln!("Waiting for port forwards to establish...");
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        // Check if port-forward processes are still running
        eprintln!("Checking port-forward process status...");
        for (i, pf) in _port_forwards.iter().enumerate() {
            eprintln!("  Port-forward {}: PID {:?}", i, pf.id());
        }

        // Health check: try to connect to each port
        eprintln!("Checking port forward connectivity...");
        let mut ready_count = 0;
        for i in 0..num_nodes {
            let local_port = base_http_port.saturating_sub(i as u16);
            let mut retries = 0;
            let max_retries = 20; // Reduced since pods are already ready
            loop {
                match tokio::net::TcpStream::connect(format!("127.0.0.1:{}", local_port)).await {
                    Ok(_) => {
                        eprintln!("✓ Node {} port-forward ready (port {})", i, local_port);
                        ready_count += 1;
                        break;
                    }
                    Err(_) => {
                        retries += 1;
                        if retries >= max_retries {
                            eprintln!(
                                "⚠️  Warning: Node {} port-forward not responding after {} \
                                 attempts",
                                i, max_retries
                            );
                            break;
                        }
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    }
                }
            }
        }

        if ready_count < num_nodes {
            eprintln!();
            eprintln!("⚠️  Note: kubectl port-forward can be unreliable with multiple instances.");
            eprintln!("    The validators ARE running in Kubernetes. To verify:");
            eprintln!(
                "    - Check logs: kubectl logs -n {} rbft-node-0 --tail=20",
                kube_namespace
            );
            eprintln!("    - Check status: kubectl get pods -n {}", kube_namespace);
            eprintln!(
                "    - Monitor blocks: kubectl logs -n {} rbft-node-0 -f | grep \
                 'Status\\|latest_block'",
                kube_namespace
            );
            eprintln!();
        }
    }

    let signer: PrivateKeySigner = admin_key
        .parse()
        .expect("admin_key is not a valid private key");

    let node_urls_str = urls.join(",");

    let mut providers = Vec::new();
    for url in &urls {
        let provider = ProviderBuilder::new()
            .wallet(signer.clone())
            .connect_http(url.parse().expect("node URL is not valid"));

        providers.push(provider);
    }

    // Never log raw key material. The signer address is enough for diagnostics.
    eprintln!("Signer address: {}", signer.address());

    // Spawn make megatx if requested
    let mut megatx_process = if run_megatx {
        // Wait a moment for nodes to fully start up
        sleep(Duration::from_secs(5)).await;
        eprintln!("Starting megatx transaction generator...");

        // Create JSON file for megatx stdout
        let megatx_json_path = logs_dir_path.join("megatx.json");
        // Create log file for megatx stderr
        let megatx_log_path = logs_dir_path.join("megatx.log");

        // Create the JSON file for stdout
        let megatx_json_file =
            std::fs::File::create(&megatx_json_path).expect("Failed to create megatx JSON file");

        let megatx_log_file =
            std::fs::File::create(&megatx_log_path).expect("Failed to create megatx LOG file");

        let megatx_exe = exe_dir.join("rbft-megatx");
        let megatx_cmd = Command::new(&megatx_exe)
            .args(["spam", "-n", "100000", "-u", &node_urls_str])
            .stdout(std::process::Stdio::from(megatx_json_file))
            .stderr(std::process::Stdio::from(megatx_log_file))
            .kill_on_drop(true)
            .spawn();

        match megatx_cmd {
            Ok(process) => {
                eprintln!("✅ Megatx started successfully");
                Some(process)
            }
            Err(e) => {
                eprintln!("⚠️  Failed to start megatx: {}", e);
                None
            }
        }
    } else {
        None
    };

    let log_max_size_bytes = log_max_size_mb * 1_048_576;
    let mut monitor_tick: u64 = 0;
    // Parse add_at_blocks if provided
    let add_at_blocks_list: Vec<u64> = if let Some(blocks_str) = add_at_blocks {
        blocks_str
            .split(',')
            .filter_map(|s| s.trim().parse::<u64>().ok())
            .collect()
    } else {
        Vec::new()
    };

    // Track which blocks we've already added validators at
    let mut validators_added_at_blocks: std::collections::HashSet<u64> =
        std::collections::HashSet::new();

    // Track current validator count (start with initial_nodes, defaulting to num_nodes)
    let mut current_validator_count = initial_nodes.unwrap_or(num_nodes as usize);

    if !add_at_blocks_list.is_empty() {
        eprintln!(
            "Will add validators at blocks: {:?} (starting from validator index {})",
            add_at_blocks_list, current_validator_count
        );
    }

    // Parse add_followers_at if provided
    let add_followers_at_list: Vec<u64> = if let Some(blocks_str) = add_followers_at {
        blocks_str
            .split(',')
            .filter_map(|s| s.trim().parse::<u64>().ok())
            .collect()
    } else {
        Vec::new()
    };

    // Track which blocks have already caused a follower to be spawned
    let mut followers_added_at_blocks: std::collections::HashSet<u64> =
        std::collections::HashSet::new();

    // Running count of follower nodes launched so far (used to pick the node index).
    // Follower node indices start at num_nodes (after all initial validator slots).
    let mut current_follower_count: usize = 0;

    if !add_followers_at_list.is_empty() {
        eprintln!(
            "Will add follower nodes at blocks: {:?} (starting from node index {})",
            add_followers_at_list, num_nodes
        );
    }

    loop {
        sleep(Duration::from_millis(1000)).await;
        monitor_tick += 1;

        // Rotate log files every ~30 seconds when rotation is enabled
        if log_max_size_bytes > 0 && monitor_tick.is_multiple_of(30) {
            rotate_log_files(
                &logs_dir_path,
                num_nodes,
                log_max_size_bytes,
                log_keep_rotated,
            );
        }

        let mut heights: Vec<String> = Vec::new();
        let mut numeric_heights: Vec<Option<u64>> = Vec::new();
        let mut txpool_sizes: Vec<String> = Vec::new();

        for p in providers.iter() {
            match p.get_block_number().await {
                Ok(height) => {
                    heights.push(height.to_string());
                    numeric_heights.push(Some(height));
                }
                Err(_) => {
                    heights.push("?".to_string());
                    numeric_heights.push(None);
                }
            }

            if monitor_txpool {
                match get_txpool_status(p).await {
                    Ok(size) => {
                        txpool_sizes.push(format_txpool(size));
                    }
                    Err(_) => {
                        txpool_sizes.push("?".to_string());
                    }
                }
            }
        }

        // Collect RSS for each node
        let rss_values: Vec<String> = nodes
            .iter()
            .map(|node| {
                node.id()
                    .and_then(get_rss_kb)
                    .map(format_rss)
                    .unwrap_or_else(|| "?".to_string())
            })
            .collect();

        if monitor_txpool {
            eprintln!(
                "h: [{}] txpool: [{}] rss: [{}]",
                heights.join(", "),
                txpool_sizes.join(", "),
                rss_values.join(", ")
            );
        } else {
            eprintln!(
                "h: [{}] rss: [{}]",
                heights.join(", "),
                rss_values.join(", ")
            );
        }

        // Check if we should add a validator at this block height
        if !add_at_blocks_list.is_empty() {
            let max_height = numeric_heights
                .iter()
                .filter_map(|h| *h)
                .max()
                .unwrap_or_default();

            for &target_block in &add_at_blocks_list {
                if max_height >= target_block && !validators_added_at_blocks.contains(&target_block)
                {
                    validators_added_at_blocks.insert(target_block);
                    eprintln!(
                        "🔧 Block {} reached, adding validator {}...",
                        target_block, current_validator_count
                    );

                    // Wait a moment for the block to be fully propagated
                    sleep(Duration::from_millis(500)).await;

                    match add_next_validator(&admin_key, &assets, current_validator_count).await {
                        Ok(validator_info) => {
                            current_validator_count += 1;
                            eprintln!(
                                "✅ Added validator {} at block {} (total validators: {})",
                                validator_info.0, target_block, current_validator_count
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "❌ Failed to add validator at block {}: {:?}",
                                target_block, e
                            );
                        }
                    }
                }
            }
        }

        // Check if we should add a follower node at this block height
        if !add_followers_at_list.is_empty() {
            let max_height = numeric_heights
                .iter()
                .filter_map(|h| *h)
                .max()
                .unwrap_or_default();

            for &target_block in &add_followers_at_list {
                if max_height >= target_block && !followers_added_at_blocks.contains(&target_block)
                {
                    followers_added_at_blocks.insert(target_block);
                    let follower_index = num_nodes + current_follower_count as u32;
                    eprintln!(
                        "👀 Block {} reached, adding follower node at index {}...",
                        target_block, follower_index
                    );

                    // Short pause so the block is fully propagated before the new node connects
                    sleep(Duration::from_millis(500)).await;

                    match add_next_follower(
                        follower_index,
                        base_http_port,
                        &assets,
                        &trusted_peers,
                        rbft_node_exe.as_path(),
                        &data_dir_path,
                        &logs_dir_path,
                        extra_args,
                        docker,
                    ) {
                        Ok((child, url)) => {
                            current_follower_count += 1;
                            let provider = ProviderBuilder::new()
                                .wallet(signer.clone())
                                .connect_http(url.parse().expect("follower node URL is not valid"));
                            nodes.push(child);
                            providers.push(provider);
                            eprintln!(
                                "✅ Follower node {} started at block {} (total followers: {})",
                                follower_index, target_block, current_follower_count
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "❌ Failed to start follower node at block {}: {:?}",
                                target_block, e
                            );
                        }
                    }
                }
            }
        }

        let exit_condition =
            is_exit_condition_met(&numeric_heights, exit_after_block, &mut megatx_process);

        if exit_condition {
            // Calculate running time
            let running_time_seconds = start_time.elapsed().as_secs();

            // Check megatx.json file if run_megatx was enabled
            let mut final_status = serde_json::json!({
                "status": "completed",
                "max_height": numeric_heights.iter().filter_map(|h| *h).max(),
                "all_node_heights": numeric_heights.iter().map(|h| {
                    match h {
                        Some(height) => {
                            serde_json::Value::Number(serde_json::Number::from(*height))
                        }
                        None => serde_json::Value::Null
                    }
                }).collect::<Vec<_>>(),
                "running_time_seconds": running_time_seconds
            });

            let mut megatx_failed = false;
            if run_megatx {
                let megatx_json_path = logs_dir_path.join("megatx.json");
                match std::fs::read_to_string(&megatx_json_path) {
                    Ok(megatx_content) if !megatx_content.trim().is_empty() => {
                        match serde_json::from_str::<serde_json::Value>(&megatx_content) {
                            Ok(json) => {
                                final_status["megatx_summary"] = json;
                            }
                            Err(e) => {
                                eprintln!("Warning: Failed to parse megatx.json: {}", e);
                                final_status["megatx_summary"] =
                                    serde_json::json!({"error": "Failed to parse megatx output"});
                                megatx_failed = true;
                            }
                        }
                    }
                    Ok(_) => {
                        eprintln!("Warning: megatx.json is empty - megatx may not have completed");
                        final_status["megatx_summary"] =
                            serde_json::json!({"error": "megatx output is empty"});
                        megatx_failed = true;
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to read megatx.json: {}", e);
                        final_status["megatx_summary"] =
                            serde_json::json!({"error": "megatx.json not found"});
                        megatx_failed = true;
                    }
                }
            }

            println!(
                "{}",
                serde_json::to_string_pretty(&final_status)
                    .expect("failed to serialise final status")
            );

            // Clean up Docker containers if in docker mode
            if docker {
                eprintln!("Stopping Docker containers...");
                docker::cleanup_containers();
            }

            // Clean up Kubernetes resources if in kube mode
            if kube {
                kube::cleanup_testnet(&kube_namespace)?;
            }

            // Exit with status 1 if megatx was enabled but failed to produce valid status
            if megatx_failed {
                std::process::exit(1);
            }

            return Ok(());
        }
    }
    // Ok(())
}

/// Add the next validator from nodes.csv
/// Returns (validator_address, enode) on success
async fn add_next_validator(
    admin_private_key: &str,
    assets_dir: &Path,
    validator_index: usize,
) -> eyre::Result<(String, String)> {
    // Read nodes.csv to get the validator at the specified index
    let nodes_csv_path = assets_dir.join("nodes.csv");
    let csv_content = fs::read_to_string(&nodes_csv_path)
        .map_err(|e| eyre::eyre!("Failed to read nodes.csv: {}", e))?;

    let mut lines: Vec<&str> = csv_content.lines().collect();
    if lines.is_empty() || lines[0].starts_with("node_id") {
        lines.remove(0); // Remove header
    }

    if validator_index >= lines.len() {
        return Err(eyre::eyre!(
            "Validator index {} exceeds available nodes in CSV ({} total)",
            validator_index,
            lines.len()
        ));
    }

    // Get the validator at the specified index
    let line = lines[validator_index];
    let parts: Vec<&str> = line.split(',').collect();
    if parts.len() < 5 {
        return Err(eyre::eyre!(
            "Invalid CSV format in nodes.csv line: {}",
            line
        ));
    }

    let validator_address = parts[1].trim();
    let enode = parts[4].trim();

    eprintln!(
        "📝 Adding validator {} with enode {} (from nodes.csv line {})",
        validator_address,
        enode,
        validator_index + 1
    );

    // Use the validator_management module to add the validator
    crate::validator_management::add_validator(
        admin_private_key,
        validator_address
            .parse()
            .map_err(|e| eyre::eyre!("Invalid validator address: {}", e))?,
        enode,
        "http://localhost:8545",
    )
    .await?;

    Ok((validator_address.to_string(), enode.to_string()))
}

/// Spawn a follower node (non-validator) using key assets at `follower_node_index`.
///
/// The node is started with the key files from `assets` at the given index (p2p + validator
/// key) and connects to the existing validators via `trusted_peers`. It is **not** registered
/// in the on-chain `QBFTValidatorSet` contract, so the QBFT state machine treats it as a
/// follower: it only runs `upon_new_block` and never participates in proposing or voting.
///
/// Key files required:
///   `<assets>/validator-key<follower_node_index>.txt`
///   `<assets>/p2p-secret-key<follower_node_index>.txt`
///
/// These are produced by `rbft-utils node-gen` / `make genesis`. Ensure nodes.csv was
/// generated with enough entries to cover this index.
///
/// Returns `(Child process, RPC URL)` on success.
#[allow(clippy::too_many_arguments)]
fn add_next_follower(
    follower_node_index: u32,
    base_http_port: u16,
    assets: &Path,
    trusted_peers: &str,
    current_exe: &Path,
    data_dir_path: &Path,
    logs_dir_path: &Path,
    extra_args: Option<&[String]>,
    docker: bool,
) -> eyre::Result<(Child, String)> {
    let p2p_key_path = assets.join(format!("p2p-secret-key{follower_node_index}.txt"));
    if !p2p_key_path.exists() {
        return Err(eyre::eyre!(
            "Missing p2p key file for follower node index {follower_node_index}: expected {} in \
             {}. Re-generate with a larger --num-nodes to create more key slots.",
            p2p_key_path.display(),
            assets.display(),
        ));
    }

    // `start_new_validator` is purely about launching a node process; the `num_nodes`
    // argument is only used for a port-overflow warning, so pass index+1 to suppress it.
    let (child, url) = start_new_validator(
        follower_node_index,
        follower_node_index + 1,
        base_http_port,
        assets,
        trusted_peers,
        current_exe,
        data_dir_path,
        logs_dir_path,
        extra_args,
        docker,
        true,
    );

    Ok((child, url))
}

fn is_exit_condition_met(
    numeric_heights: &[Option<u64>],
    exit_after_block: Option<u64>,
    megatx_process: &mut Option<Child>,
) -> bool {
    // Check if megatx process has exited
    if let Some(ref mut process) = megatx_process {
        if let Ok(Some(_)) = process.try_wait() {
            eprintln!("🔚 Megatx process has completed - exiting testnet");
            return true;
        }
    }

    if let Some(exit_block) = exit_after_block {
        let max_height = numeric_heights
            .iter()
            .filter_map(|h| *h)
            .max()
            .unwrap_or_default();
        if max_height > exit_block {
            eprintln!("🔚 max block height reached - exiting testnet");
            return true;
        }
    }

    false
}
