// SPDX-License-Identifier: Apache-2.0
use anyhow::{anyhow, Context, Result};
use shell_words::split as shell_split;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

use crate::config::ControlConfig;
use crate::models::{ActiveChain, AppAction, AppEvent, AppState, ValidatorEntry};

pub async fn dispatch_action(
    action: AppAction,
    app: &AppState,
    tx: mpsc::Sender<AppEvent>,
) -> Result<String> {
    let control = app.control.clone();
    let entry = app
        .entries
        .get(action.index())
        .ok_or_else(|| anyhow!("No validator selected"))?;
    let label = entry.target.label.clone();

    match action {
        AppAction::Start(_) => handle_validator_start(&control, app, entry, &tx, &label).await,
        AppAction::Stop(_) => handle_validator_stop(&control, entry, tx, &label).await,
        AppAction::Restart(_) => handle_validator_restart(&control, app, entry, &tx, &label).await,
    }
}

pub async fn handle_validator_start(
    control: &ControlConfig,
    app: &AppState,
    entry: &ValidatorEntry,
    tx: &mpsc::Sender<AppEvent>,
    label: &str,
) -> Result<String> {
    ensure_validator_stopped_with_timeout(control, entry)
        .await
        .map_err(|err| anyhow!("Failed to stop existing {} before start: {}", label, err))?;

    if let Some(template) = control.start_cmd.clone() {
        let command = render_command(&template, entry)?;
        spawn_single_command(tx.clone(), label.to_string(), "start".to_string(), command);
    } else if let Some(chain) = app.active_chain.clone() {
        let command = build_validator_command(&chain, entry)?;
        spawn_background_command(
            tx.clone(),
            label.to_string(),
            "start",
            command,
            Some(format!("Launched validator {}", label)),
            entry.target.log_path.clone(),
        )
        .await?;
    } else {
        return Err(anyhow!(
            "No start command template available and no active chain configured"
        ));
    }

    track_validator_ready(tx.clone(), entry.clone(), "start");
    Ok(format!("Starting {}...", label))
}

pub async fn handle_validator_stop(
    control: &ControlConfig,
    entry: &ValidatorEntry,
    tx: mpsc::Sender<AppEvent>,
    label: &str,
) -> Result<String> {
    if let Some(template) = control.stop_cmd.clone() {
        let command = render_command(&template, entry)?;
        spawn_single_command(tx, label.to_string(), "stop".to_string(), command);
        Ok(format!("Stopping {}...", label))
    } else if let Some(port) = entry.target.http_port() {
        // Fixed lifetime issue by cloning label before moving into async block
        let tx_clone = tx.clone();
        let label_clone = label.to_string();
        tokio::spawn(async move {
            let result = kill_by_port(port).await;
            let (success, message) = match result {
                Ok(msg) => (true, Some(msg)),
                Err(err) => (false, Some(err.to_string())),
            };
            let _ = tx_clone
                .send(AppEvent::CommandResult {
                    label: label_clone, // Use cloned label
                    action: "stop".to_string(),
                    success,
                    message,
                })
                .await;
        });
        Ok(format!("Stopping validator on port {}...", port))
    } else {
        Err(anyhow!(
            "Stop command not configured and port unknown for validator"
        ))
    }
}

pub async fn handle_validator_restart(
    control: &ControlConfig,
    app: &AppState,
    entry: &ValidatorEntry,
    tx: &mpsc::Sender<AppEvent>,
    label: &str,
) -> Result<String> {
    // Try dedicated restart command first
    if let Some(template) = control.restart_cmd.clone() {
        let command = render_command(&template, entry)?;
        spawn_single_command(
            tx.clone(),
            label.to_string(),
            "restart".to_string(),
            command,
        );
        track_validator_ready(tx.clone(), entry.clone(), "restart");
        return Ok(format!("Restarting {}...", label));
    }

    // Fall back to stop-then-start with template commands
    if control.can_stop() && control.can_start() {
        let stop_template = control
            .stop_cmd
            .clone()
            .ok_or_else(|| anyhow!("Stop command not configured"))?;
        let start_template = control
            .start_cmd
            .clone()
            .ok_or_else(|| anyhow!("Start command not configured"))?;

        let stop_cmd = render_command(&stop_template, entry)?;
        let start_cmd = render_command(&start_template, entry)?;
        spawn_restart_command(tx.clone(), label.to_string(), stop_cmd, start_cmd);
        track_validator_ready(tx.clone(), entry.clone(), "restart");
        return Ok(format!("Restarting {}...", label));
    }

    // Fall back to chain-based restart
    let chain = app.active_chain.clone().ok_or_else(|| {
        anyhow!("Restart command not configured and no chain launch metadata available")
    })?;

    ensure_validator_stopped_with_timeout(control, entry)
        .await
        .map_err(|err| anyhow!("Failed to stop existing {} before restart: {}", label, err))?;

    let command = build_validator_command(&chain, entry)?;
    spawn_background_command(
        tx.clone(),
        label.to_string(),
        "restart",
        command,
        Some(format!("Restarted validator {}", label)),
        entry.target.log_path.clone(),
    )
    .await?;
    track_validator_ready(tx.clone(), entry.clone(), "restart");
    Ok(format!("Restarting {}...", label))
}

pub async fn ping_selected_validator(app: &AppState, idx: usize) -> Result<String> {
    let entry = app
        .entries
        .get(idx)
        .ok_or_else(|| anyhow!("No validator selected"))?
        .clone();
    ping_validator(entry).await
}

async fn ping_validator(entry: ValidatorEntry) -> Result<String> {
    let host = entry
        .target
        .url
        .host_str()
        .ok_or_else(|| anyhow!("Validator URL missing host"))?
        .to_string();
    let port = entry
        .target
        .http_port()
        .or_else(|| entry.target.url.port_or_known_default())
        .ok_or_else(|| anyhow!("Validator URL missing port"))?;
    let addr = format!("{host}:{port}");
    let timeout_dur = Duration::from_secs(2);
    match timeout(timeout_dur, tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(_stream)) => Ok(format!("Ping to {} succeeded ({addr})", entry.target.label)),
        Ok(Err(err)) => Err(anyhow!("connection to {addr} failed: {err}")),
        Err(_) => Err(anyhow!("ping to {addr} timed out after {timeout_dur:?}")),
    }
}

fn render_command(template: &str, entry: &ValidatorEntry) -> Result<Vec<String>> {
    let mut rendered = template
        .replace("{label}", &entry.target.label)
        .replace("{url}", entry.target.url.as_str());
    let port = entry
        .target
        .http_port()
        .map(|p| p.to_string())
        .unwrap_or_default();
    rendered = rendered
        .replace("{port}", &port)
        .replace("{http_port}", &port);

    let parts =
        shell_split(&rendered).map_err(|err| anyhow!("failed to parse command template: {err}"))?;
    if parts.is_empty() {
        return Err(anyhow!("command template produced an empty command"));
    }
    Ok(parts)
}

async fn execute_command(parts: Vec<String>) -> Result<Option<String>> {
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| anyhow!("command template produced an empty command"))?;
    let mut cmd = TokioCommand::new(program);
    cmd.args(args);
    cmd.kill_on_drop(true);
    let output = cmd.output().await?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if !stdout.is_empty() {
            Some(stdout)
        } else if !stderr.is_empty() {
            Some(stderr)
        } else {
            None
        };
        Ok(message)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut msg = if !stderr.is_empty() { stderr } else { stdout };
        if msg.is_empty() {
            msg = format!("command exited with status {}", output.status);
        }
        Err(anyhow!(msg))
    }
}

fn spawn_single_command(
    tx: mpsc::Sender<AppEvent>,
    label: String,
    action: String,
    command: Vec<String>,
) {
    tokio::spawn(async move {
        let result = execute_command(command).await;
        let (success, message) = match result {
            Ok(msg) => (true, msg),
            Err(err) => (false, Some(err.to_string())),
        };
        let _ = tx
            .send(AppEvent::CommandResult {
                label,
                action,
                success,
                message,
            })
            .await;
    });
}

fn spawn_restart_command(
    tx: mpsc::Sender<AppEvent>,
    label: String,
    stop_cmd: Vec<String>,
    start_cmd: Vec<String>,
) {
    tokio::spawn(async move {
        let stop_res = execute_command(stop_cmd).await;
        if let Err(err) = stop_res {
            let _ = tx
                .send(AppEvent::CommandResult {
                    label,
                    action: "restart".to_string(),
                    success: false,
                    message: Some(format!("stop failed: {err}")),
                })
                .await;
            return;
        }

        let start_res = execute_command(start_cmd).await;
        let (success, message) = match start_res {
            Ok(msg) => (
                true,
                Some(msg.unwrap_or_else(|| "restart completed".to_string())),
            ),
            Err(err) => (false, Some(format!("start failed: {err}"))),
        };
        let _ = tx
            .send(AppEvent::CommandResult {
                label,
                action: "restart".to_string(),
                success,
                message,
            })
            .await;
    });
}

async fn spawn_background_command(
    tx: mpsc::Sender<AppEvent>,
    label: String,
    action: &'static str,
    command: Vec<String>,
    info: Option<String>,
    log_path: Option<PathBuf>,
) -> Result<()> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| anyhow!("command template produced an empty command"))?;

    let mut cmd = TokioCommand::new(program);
    cmd.args(args);
    cmd.kill_on_drop(true);

    // Set working directory for cargo commands
    if program == "cargo" {
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

        if let Ok(absolute_path) = std::fs::canonicalize(&project_root) {
            cmd.current_dir(absolute_path);
        }
    }

    // Set RUST_LOG for better debugging
    cmd.env("RUST_LOG", "debug");

    let log_path = if let Some(path) = log_path {
        path
    } else {
        std::env::temp_dir().join(format!("{}-{}.log", label, action))
    };

    match prepare_log_stdio(&log_path, &command) {
        Ok((stdout, stderr)) => {
            cmd.stdout(stdout);
            cmd.stderr(stderr);
        }
        Err(err) => {
            return Err(anyhow!("failed to prepare log file: {}", err));
        }
    }

    cmd.stdin(Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            let tx_clone = tx.clone();
            let label_clone = label.clone();
            tokio::spawn(async move {
                let status = child.wait_with_output().await;
                match status {
                    Ok(output) => {
                        if !output.status.success() {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let _ = tx_clone
                                .send(AppEvent::CommandResult {
                                    label: label_clone.clone(),
                                    action: action.to_string(),
                                    success: false,
                                    message: Some(format!(
                                        "Process exited with status: {}\nstderr: {}",
                                        output.status, stderr
                                    )),
                                })
                                .await;
                        }
                    }
                    Err(err) => {
                        let _ = tx_clone
                            .send(AppEvent::CommandResult {
                                label: label_clone.clone(),
                                action: action.to_string(),
                                success: false,
                                message: Some(format!("Failed to wait for process: {}", err)),
                            })
                            .await;
                    }
                }
            });

            let _ = tx
                .send(AppEvent::CommandResult {
                    label,
                    action: action.to_string(),
                    success: true,
                    message: info,
                })
                .await;

            Ok(())
        }
        Err(err) => Err(anyhow!("failed to launch {action} command: {err}")),
    }
}

fn prepare_log_stdio(path: &PathBuf, command: &[String]) -> Result<(Stdio, Stdio)> {
    use std::fs::OpenOptions;
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    }
    {
        let mut header = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("failed to open log file {}", path.display()))?;
        writeln!(header, "# {}", command.join(" "))
            .with_context(|| format!("failed to write log header {}", path.display()))?;
        writeln!(header)
            .with_context(|| format!("failed to write log header {}", path.display()))?;
    }
    let stdout_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open log file {}", path.display()))?;
    let stderr_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open log file {}", path.display()))?;
    Ok((Stdio::from(stdout_file), Stdio::from(stderr_file)))
}

async fn kill_by_port(port: u16) -> Result<String> {
    let cmd = format!("lsof -ti tcp:{port} -sTCP:LISTEN");
    let output = TokioCommand::new("sh").arg("-c").arg(cmd).output().await?;
    if !output.status.success() {
        return Err(anyhow!(
            "Failed to identify listening process on port {port}. Is lsof installed?"
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut killed = 0usize;
    let self_pid = std::process::id().to_string();
    for line in stdout.lines() {
        let pid = line.trim();
        if pid.is_empty() || pid == self_pid {
            continue;
        }
        if TokioCommand::new("kill")
            .arg("-9")
            .arg(pid)
            .status()
            .await?
            .success()
        {
            killed += 1;
        }
    }
    if killed == 0 {
        Err(anyhow!(
            "No listening processes terminated on port {port}. Is the validator running?"
        ))
    } else {
        Ok(format!(
            "Terminated {killed} listening process(es) on port {port}"
        ))
    }
}

async fn ensure_port_free(port: u16) -> Result<()> {
    // First, try to kill any processes using the port
    if let Ok(output) = TokioCommand::new("sh")
        .arg("-c")
        .arg(format!("lsof -ti tcp:{port} -sTCP:LISTEN"))
        .output()
        .await
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let self_pid = std::process::id().to_string();

            for line in stdout.lines() {
                let pid = line.trim();
                if pid.is_empty() || pid == self_pid {
                    continue;
                }

                // Try graceful kill first, then force kill
                let _ = TokioCommand::new("kill").arg(pid).status().await;
                sleep(Duration::from_millis(100)).await;

                // Check if process is still running and force kill if needed
                if TokioCommand::new("kill")
                    .arg("-0")
                    .arg(pid)
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false)
                {
                    let _ = TokioCommand::new("kill").arg("-9").arg(pid).status().await;
                }
            }

            // Wait for processes to terminate
            sleep(Duration::from_millis(500)).await;
        }
    }

    // Verify port is actually free
    if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
        Ok(())
    } else {
        Err(anyhow!(
            "Port {} is still in use after cleanup attempts",
            port
        ))
    }
}

async fn ensure_validator_stopped(_control: &ControlConfig, entry: &ValidatorEntry) -> Result<()> {
    // Always attempt port-based cleanup as a fallback
    if let Some(port) = entry
        .target
        .http_port()
        .or_else(|| entry.target.url.port_or_known_default())
    {
        // Give the stop command a moment to take effect
        sleep(Duration::from_millis(500)).await;

        // Multiple attempts to ensure port is free
        const MAX_ATTEMPTS: u8 = 3;
        for attempt in 1..=MAX_ATTEMPTS {
            match ensure_port_free(port).await {
                Ok(_) => break,
                Err(err) if attempt == MAX_ATTEMPTS => {
                    return Err(anyhow!(
                        "Failed to free port {} after {} attempts: {}",
                        port,
                        MAX_ATTEMPTS,
                        err
                    ));
                }
                Err(_) => {
                    // Wait before retrying
                    sleep(Duration::from_millis(1000 * attempt as u64)).await;
                }
            }
        }
    }

    // Clean up database lock files
    cleanup_database_locks(entry).await?;

    Ok(())
}

async fn cleanup_database_locks(entry: &ValidatorEntry) -> Result<()> {
    let label = entry.target.label.trim_start_matches('v');
    let idx: usize = label
        .parse()
        .map_err(|e| anyhow!("invalid validator label: {e}"))?;

    let (db_dir, _) = crate::config::default_test_dirs();
    let datadir = db_dir.join(format!("d{idx}"));

    // First, kill any processes that might be using the database
    if let Some(port) = entry.target.http_port() {
        let _ = kill_by_port(port).await;
    }

    // Kill any processes that might be using the IPC file
    let ipc_path = datadir.join("reth.ipc");
    if ipc_path.exists() {
        let _ = std::fs::remove_file(&ipc_path);
    }

    // Define all possible lock file locations
    let lock_locations = vec![
        datadir.join("db/lock"),            // Main database lock
        datadir.join("db/.lock"),           // Alternative lock location
        datadir.join("static_files/lock"),  // Static files lock (THIS IS THE ONE WE MISSED!)
        datadir.join("static_files/.lock"), // Alternative static files lock
        datadir.join(".lock"),              // Root directory lock
        datadir.join("LOCK"),               // Uppercase lock
    ];

    // Remove all lock files
    for lock_path in lock_locations {
        if lock_path.exists() {
            // If it's a directory, remove it recursively
            if lock_path.is_dir() {
                let _ = std::fs::remove_dir_all(&lock_path);
            } else {
                let _ = std::fs::remove_file(&lock_path);
            }
        }
    }

    // Also clean up any temporary files in all subdirectories
    let subdirs = vec!["db", "static_files"];
    for subdir in subdirs {
        let subdir_path = datadir.join(subdir);
        if subdir_path.exists() {
            if let Ok(entries) = std::fs::read_dir(&subdir_path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.ends_with(".tmp")
                            || name.ends_with(".lock")
                            || name.starts_with("temp-")
                        {
                            if path.is_dir() {
                                let _ = std::fs::remove_dir_all(&path);
                            } else {
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    }
                }
            }
        }
    }

    // Wait a moment for cleanup to take effect
    sleep(Duration::from_millis(500)).await;

    Ok(())
}

async fn ensure_validator_stopped_with_timeout(
    control: &ControlConfig,
    entry: &ValidatorEntry,
) -> Result<()> {
    timeout(
        Duration::from_secs(30),
        ensure_validator_stopped(control, entry),
    )
    .await
    .map_err(|_| anyhow!("Timeout stopping validator after 30 seconds"))?
}

fn track_validator_ready(tx: mpsc::Sender<AppEvent>, entry: ValidatorEntry, phase: &'static str) {
    tokio::spawn(async move {
        const MAX_ATTEMPTS: usize = 30;
        let mut last_err: Option<anyhow::Error> = None;
        for _ in 0..MAX_ATTEMPTS {
            match ping_validator(entry.clone()).await {
                Ok(_) => {
                    let _ = tx
                        .send(AppEvent::CommandResult {
                            label: entry.target.label.clone(),
                            action: format!("{phase}-ready"),
                            success: true,
                            message: Some(format!("{} responded successfully", entry.target.label)),
                        })
                        .await;
                    return;
                }
                Err(err) => {
                    last_err = Some(err);
                }
            }
            sleep(Duration::from_secs(1)).await;
        }
        let msg = last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "validator did not come back online in time".to_string());
        let _ = tx
            .send(AppEvent::CommandResult {
                label: entry.target.label.clone(),
                action: format!("{phase}-ready"),
                success: false,
                message: Some(msg),
            })
            .await;
    });
}

fn build_validator_command(chain: &ActiveChain, entry: &ValidatorEntry) -> Result<Vec<String>> {
    let label = entry.target.label.trim_start_matches('v');
    let idx: usize = label
        .parse()
        .map_err(|e| anyhow!("invalid validator label: {e}"))?;
    let http_port = chain.base_http_port.saturating_sub(idx as u16);
    if http_port == 0 {
        return Err(anyhow!("Base HTTP port too small for validator {label}"));
    }
    let authrpc_port = 8551 + idx as u16 * 10;
    let p2p_port = 30303 + idx as u16;
    let (data_root, logs_root) = (chain.data_dir.clone(), chain.logs_dir.clone());
    let datadir = data_root.join(format!("d{idx}"));
    let logs_dir = logs_root.join(format!("node{idx}.log"));

    // Create necessary directories
    std::fs::create_dir_all(&datadir)
        .with_context(|| format!("failed to create data directory {}", datadir.display()))?;
    if let Some(parent) = logs_dir.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create logs directory {}", parent.display()))?;
    }

    let reth_log_dir = std::env::temp_dir()
        .join("validator-inspector")
        .join("reth-logs")
        .join(format!("node{idx}"));
    std::fs::create_dir_all(&reth_log_dir).with_context(|| {
        format!(
            "failed to create reth log directory {}",
            reth_log_dir.display()
        )
    })?;

    let ipc_path = datadir.join("reth.ipc");
    if ipc_path.exists() {
        let _ = std::fs::remove_file(&ipc_path);
    }

    let mut command = vec![
        chain.rbft_binary.to_string_lossy().to_string(),
        "node".to_string(),
        "--http".to_string(),
        "--http.port".to_string(),
        http_port.to_string(),
        "--http.addr".to_string(),
        "0.0.0.0".to_string(),
        "--http.corsdomain".to_string(),
        "*".to_string(),
        "--chain".to_string(),
        chain
            .assets_dir
            .join("genesis.json")
            .to_string_lossy()
            .to_string(),
        "--p2p-secret-key".to_string(),
        chain
            .assets_dir
            .join(format!("p2p-secret-key{idx}.txt"))
            .to_string_lossy()
            .to_string(),
        "--validator-key".to_string(),
        chain
            .assets_dir
            .join(format!("validator-key{idx}.txt"))
            .to_string_lossy()
            .to_string(),
        "--logs-dir".to_string(),
        logs_root.to_string_lossy().to_string(),
    ];

    if let Some(trusted) = &chain.trusted_peers {
        if !trusted.is_empty() {
            command.push("--trusted-peers".to_string());
            command.push(trusted.clone());
        }
    }

    command.push("--log.file.directory".to_string());
    command.push(reth_log_dir.to_string_lossy().to_string());
    command.push("--log.file.name".to_string());
    command.push(format!("node{idx}.reth.log"));
    command.extend([
        "--datadir".to_string(),
        datadir.to_string_lossy().to_string(),
        "--ipcpath".to_string(),
        datadir.join("reth.ipc").to_string_lossy().to_string(),
        "--authrpc.port".to_string(),
        authrpc_port.to_string(),
        "--port".to_string(),
        p2p_port.to_string(),
        "--disable-discovery".to_string(),
    ]);

    Ok(command)
}
