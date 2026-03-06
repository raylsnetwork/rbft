// SPDX-License-Identifier: Apache-2.0
use anyhow::{anyhow, Result};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc;

use crate::models::{AppEvent, SpamMode, SpamRequest};

pub async fn handle_spam_request(
    request: SpamRequest,
    validators: Vec<String>,
    tx: mpsc::Sender<AppEvent>,
) -> Result<()> {
    if validators.is_empty() {
        return Err(anyhow!("No validators available for spam transactions"));
    }

    if which::which("cast").is_err() {
        return Err(anyhow!(
            "Foundry 'cast' binary not found. Install with: curl -L https://foundry.paradigm.xyz \
             | bash"
        ));
    }

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/spam-txs.sh");
    if !script.exists() {
        return Err(anyhow!("Spam script not found at {}", script.display()));
    }

    ensure_validators_reachable(&validators).await?;

    let mut cmd = TokioCommand::new("bash");
    cmd.arg(&script);
    cmd.arg("--accounts").arg(request.accounts.to_string());
    cmd.arg("--count").arg(request.total_txs.to_string());
    cmd.arg("--parallel").arg(request.parallel.to_string());
    cmd.arg("--burst").arg(request.burst.to_string());
    let mode_str = match request.mode {
        SpamMode::RoundRobin => "round-robin",
        SpamMode::Target => "target",
    };
    cmd.arg("--mode").arg(mode_str);
    if matches!(request.mode, SpamMode::Target) {
        if let Some(target) = &request.target_url {
            cmd.arg("--target").arg(target);
        } else {
            return Err(anyhow!("Target URL required for target mode"));
        }
    }
    cmd.env("VALIDATOR_URLS", validators.join(","));
    // Ensure tools like cast don't emit debug/colored output that breaks parsing
    cmd.env_remove("RUST_LOG");
    cmd.env("RUST_LOG", "");
    cmd.env("FOUNDRY_LOG", "error");
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR", "0");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    if let Some(parent) = script.parent() {
        cmd.current_dir(parent);
    }

    let child = cmd
        .spawn()
        .map_err(|err| anyhow!("Failed to start spam script: {err}"))?;
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        match child.wait_with_output().await {
            Ok(output) => {
                let success = output.status.success();
                let log_path = persist_spam_output(&output);
                let message =
                    build_message(success, &output.stdout, &output.stderr, log_path.as_deref());
                let _ = tx_clone
                    .send(AppEvent::CommandResult {
                        label: "spam".to_string(),
                        action: "spam".to_string(),
                        success,
                        message: Some(message),
                    })
                    .await;
            }
            Err(err) => {
                let _ = tx_clone
                    .send(AppEvent::CommandResult {
                        label: "spam".to_string(),
                        action: "spam".to_string(),
                        success: false,
                        message: Some(format!("Spam job failed: {err}")),
                    })
                    .await;
            }
        }
    });

    Ok(())
}

fn build_message(success: bool, stdout: &[u8], stderr: &[u8], log_path: Option<&str>) -> String {
    let mut base = if success {
        "Spam job completed".to_string()
    } else {
        "Spam job failed".to_string()
    };
    if let Ok(text) = String::from_utf8(stdout.to_vec()) {
        if !text.trim().is_empty() {
            base.push_str("\n--- Output ---\n");
            base.push_str(&tail_lines(&text, 10));
        }
    }
    if let Ok(text) = String::from_utf8(stderr.to_vec()) {
        if !text.trim().is_empty() {
            base.push_str("\n--- Errors ---\n");
            base.push_str(&tail_lines(&text, 10));
        }
    }
    if let Some(path) = log_path {
        base.push_str("\nLog: ");
        base.push_str(path);
    }
    base
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

async fn ensure_validators_reachable(validators: &[String]) -> Result<()> {
    let client = reqwest::Client::new();
    let mut ok = false;
    for url in validators {
        let resp = client
            .post(url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }))
            .send()
            .await;
        if let Ok(r) = resp {
            if r.status().is_success() {
                ok = true;
                break;
            }
        }
    }
    if ok {
        Ok(())
    } else {
        Err(anyhow!(
            "Could not reach any validator RPC (eth_blockNumber failed)"
        ))
    }
}

fn persist_spam_output(output: &std::process::Output) -> Option<String> {
    let dir = std::env::temp_dir().join("validator-inspector");
    if fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("spam-{timestamp}.log"));
    let mut content = String::new();
    content.push_str(&format!("Status: {:?}\n\n", output.status));
    content.push_str("--- STDOUT ---\n");
    content.push_str(&String::from_utf8_lossy(&output.stdout));
    content.push_str("\n--- STDERR ---\n");
    content.push_str(&String::from_utf8_lossy(&output.stderr));
    if fs::write(&path, content).is_ok() {
        Some(path.display().to_string())
    } else {
        None
    }
}
