// SPDX-License-Identifier: Apache-2.0
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc, RwLock};
use tokio::time::sleep;

use crate::config::{assets_dir, default_test_dirs, load_trusted_peers};
use crate::models::{ActiveChain, AppEvent, LaunchRequest, ValidatorTarget};
use crate::utils::build_targets_from_ports;

fn resolve_rbft_binary(rbft_bin_dir: &Path) -> Result<PathBuf> {
    if rbft_bin_dir.is_file() {
        return Ok(rbft_bin_dir.to_path_buf());
    }

    [rbft_bin_dir.join("rbft2"), rbft_bin_dir.join("rbft-node2")]
        .into_iter()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| {
            anyhow!(
                "No rbft binary found in {} (expected rbft2 or rbft-node2)",
                rbft_bin_dir.display()
            )
        })
}

pub async fn handle_launch_chain(
    shared_targets: Arc<RwLock<Vec<ValidatorTarget>>>,
    tx: mpsc::Sender<AppEvent>,
    request: LaunchRequest,
    rbft_bin_dir: PathBuf,
) -> Result<(ActiveChain, String)> {
    let (db_dir, logs_dir) = default_test_dirs();
    std::fs::create_dir_all(&db_dir)?;
    std::fs::create_dir_all(&logs_dir)?;
    let assets_dir = assets_dir();

    // Create the targets first
    let ports: Vec<u16> = (0..request.count)
        .map(|idx| request.base_port.saturating_sub(idx as u16))
        .collect();

    let targets = build_targets_from_ports(&ports, &logs_dir)?;
    let rbft_binary = resolve_rbft_binary(&rbft_bin_dir)?;

    {
        let mut write = shared_targets.write().await;
        *write = targets.clone();
    }

    // Send target update to main app
    let _ = tx.send(AppEvent::TargetUpdate(targets)).await;

    // Spawn the chain command
    spawn_chain_command(
        tx.clone(),
        request.count,
        request.base_port,
        &logs_dir,
        &db_dir,
        rbft_binary.clone(),
    )?;

    let active_chain = ActiveChain {
        base_http_port: request.base_port,
        num_nodes: request.count,
        data_dir: db_dir,
        logs_dir,
        assets_dir,
        rbft_binary,
        trusted_peers: load_trusted_peers(),
    };

    // Wait for chain initialization
    sleep(Duration::from_secs(5)).await;

    Ok((
        active_chain,
        format!(
            "Launched chain: {} validators, base port {}",
            request.count, request.base_port
        ),
    ))
}

fn spawn_chain_command(
    tx: mpsc::Sender<AppEvent>,
    count: usize,
    base_port: u16,
    logs_dir: &Path,
    db_dir: &Path,
    rbft_binary: PathBuf,
) -> Result<()> {
    let mut cmd = TokioCommand::new(rbft_binary);
    cmd.env("RUST_LOG", "debug");

    cmd.arg("node")
        .arg("testnet")
        .arg("--init")
        .arg("--num-nodes")
        .arg(count.to_string())
        .arg("--base-http-port")
        .arg(base_port.to_string())
        .arg("--logs-dir")
        .arg(logs_dir.to_string_lossy().to_string())
        .arg("--db-dir")
        .arg(db_dir.to_string_lossy().to_string());
    cmd.kill_on_drop(true);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd.stdin(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    tokio::spawn(async move {
        let result = child.wait().await;
        let (success, message) = match result {
            Ok(status) if status.success() => (
                true,
                Some(format!(
                    "Chain command completed successfully (exit code {})",
                    status.code().unwrap_or_default()
                )),
            ),
            Ok(status) => (
                false,
                Some(format!("Chain command exited with status {status}")),
            ),
            Err(err) => (
                false,
                Some(format!("Failed to wait for chain command: {err}")),
            ),
        };
        let _ = tx
            .send(AppEvent::CommandResult {
                label: "chain".to_string(),
                action: "launch".to_string(),
                success,
                message,
            })
            .await;
    });
    Ok(())
}
