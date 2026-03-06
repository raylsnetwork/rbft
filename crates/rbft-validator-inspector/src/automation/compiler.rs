// SPDX-License-Identifier: Apache-2.0
use std::path::{Path, PathBuf};
use std::time::Duration;

use alloy_dyn_abi::{DynSolType, DynSolValue};
use alloy_primitives::keccak256;
use anyhow::{anyhow, bail, Context, Result};
use revm::context::{Context as EvmContext, TxEnv};
use revm::database::{CacheDB, EmptyDB};
use revm::primitives::{Address, Bytes, TxKind};
use revm::{ExecuteEvm, MainBuilder, MainContext};
use serde::Deserialize;
use tokio::process::Command;
use tokio::{fs, task};

use crate::models::{AutomationAction, LaunchRequest, SpamMode, SpamRequest};

#[derive(Clone, Debug)]
pub struct AutomationConfig {
    pub project_root: PathBuf,
    pub script_path: String,
    pub contract: String,
}

#[derive(Clone)]
pub struct ScheduledCommand {
    pub action: AutomationAction,
    pub offset: Duration,
    pub interval: Option<Duration>,
    pub repeats: u32,
}

#[derive(Deserialize)]
struct BytecodeObject {
    object: String,
}

#[derive(Deserialize)]
struct ForgeArtifact {
    bytecode: BytecodeObject,
}

pub async fn load_commands(config: &AutomationConfig) -> Result<Vec<ScheduledCommand>> {
    let source_file = Path::new(&config.script_path)
        .file_name()
        .ok_or_else(|| anyhow!("Invalid contract path {}", config.script_path.clone()))?;
    let artifact_path = config
        .project_root
        .join("out")
        .join(source_file)
        .join(format!("{}.json", config.contract));
    let artifact_bytes = fs::read_to_string(&artifact_path)
        .await
        .with_context(|| format!("Failed to read artifact {}", artifact_path.display()))?;
    let artifact: ForgeArtifact = serde_json::from_str(&artifact_bytes)
        .with_context(|| format!("Invalid artifact JSON at {}", artifact_path.display()))?;

    let bytecode = decode_hex(&artifact.bytecode.object)
        .with_context(|| format!("Invalid bytecode for {}", config.contract))?;
    let return_data = task::spawn_blocking(move || execute_commands(bytecode))
        .await
        .context("Automation execution task panicked")??;

    decode_commands(&return_data)
}

pub async fn run_forge_build(project_root: &Path) -> Result<String> {
    let output = Command::new("forge")
        .arg("build")
        .arg("--force")
        .current_dir(project_root)
        .output()
        .await
        .context("Failed to run forge build")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("forge build failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let cleaned: Vec<String> = stdout
        .replace('\r', "\n")
        .lines()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    Ok(cleaned.join("\n"))
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim_start_matches("0x");
    alloy_primitives::hex::decode(trimmed).map_err(|err| anyhow!("Invalid hex: {err}"))
}

fn execute_commands(bytecode: Vec<u8>) -> Result<Vec<u8>> {
    let deployer = Address::with_last_byte(1);
    let ctx = EvmContext::mainnet().with_db(CacheDB::new(EmptyDB::default()));
    let mut evm = ctx.build_mainnet();

    // Deploy the automation script contract
    let deploy_tx = TxEnv::builder()
        .caller(deployer)
        .kind(TxKind::Create)
        .gas_limit(30_000_000)
        .data(Bytes::from(bytecode))
        .build()
        .map_err(|err| anyhow!("Failed to build deploy tx: {err:?}"))?;

    let deploy_out = evm
        .transact(deploy_tx)
        .map_err(|err| anyhow!("Automation deployment failed: {err}"))?;

    let contract = deploy_out
        .result
        .created_address()
        .ok_or_else(|| anyhow!("Automation deployment did not return an address"))?;

    // Apply the deployment state changes
    let db = &mut evm.ctx.journaled_state.database;
    for (addr, account) in deploy_out.state {
        if account.is_touched() {
            let info = account.info;
            db.insert_account_info(addr, info);
            for (slot, value) in account.storage {
                let _ = db.insert_account_storage(addr, slot, value.present_value);
            }
        }
    }

    // Call commands() function
    let mut call_data = Vec::with_capacity(4);
    call_data.extend_from_slice(&keccak256("commands()")[0..4]);

    let call_tx = TxEnv::builder()
        .caller(deployer)
        .nonce(1)
        .kind(TxKind::Call(contract))
        .gas_limit(15_000_000)
        .data(Bytes::from(call_data))
        .build()
        .map_err(|err| anyhow!("Failed to build call tx: {err:?}"))?;

    let call_out = evm
        .transact(call_tx)
        .map_err(|err| anyhow!("commands() invocation failed: {err}"))?;

    let output = call_out
        .result
        .into_output()
        .ok_or_else(|| anyhow!("commands() execution reverted"))?;

    Ok(output.to_vec())
}

fn decode_commands(data: &[u8]) -> Result<Vec<ScheduledCommand>> {
    let tuple_ty = DynSolType::Tuple(vec![
        DynSolType::Uint(64),
        DynSolType::Uint(64),
        DynSolType::Uint(32),
        DynSolType::Uint(8),
        DynSolType::Bytes,
    ]);
    let ty = DynSolType::Array(Box::new(tuple_ty));
    let decoded = ty
        .abi_decode(data)
        .map_err(|err| anyhow!("Failed to decode automation output: {err}"))?;
    let list = decoded
        .as_array()
        .ok_or_else(|| anyhow!("Automation output is not an array"))?;

    let mut commands = Vec::with_capacity(list.len());
    for (idx, entry) in list.iter().enumerate() {
        let tuple = entry
            .as_tuple()
            .ok_or_else(|| anyhow!("Command {idx} was not a tuple"))?;
        let offset_secs = as_u64(&tuple[0], idx, "offsetSeconds")?;
        let interval_secs = as_u64(&tuple[1], idx, "intervalSeconds")?;
        let repeat = as_u32(&tuple[2], idx, "repeatCount")?;
        let kind = as_u8(&tuple[3], idx, "kind")?;
        let payload = tuple[4]
            .as_bytes()
            .ok_or_else(|| anyhow!("Command {idx} payload missing"))?;

        let action = match decode_action(kind, payload) {
            Ok(action) => action,
            Err(err) => AutomationAction::Log(format!(
                "Decode error for command {idx}: {err} (payload 0x{})",
                alloy_primitives::hex::encode(payload)
            )),
        };
        commands.push(ScheduledCommand {
            action,
            offset: Duration::from_secs(offset_secs),
            interval: if interval_secs > 0 {
                Some(Duration::from_secs(interval_secs))
            } else {
                None
            },
            repeats: repeat,
        });
    }

    Ok(commands)
}

#[derive(Clone, Copy)]
enum CommandKind {
    LaunchChain = 0,
    StartValidator = 1,
    StopValidator = 2,
    RestartValidator = 3,
    LaunchValidator = 4,
    SpamJob = 5,
    PingValidator = 6,
    Log = 7,
}

impl TryFrom<u8> for CommandKind {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(CommandKind::LaunchChain),
            1 => Ok(CommandKind::StartValidator),
            2 => Ok(CommandKind::StopValidator),
            3 => Ok(CommandKind::RestartValidator),
            4 => Ok(CommandKind::LaunchValidator),
            5 => Ok(CommandKind::SpamJob),
            6 => Ok(CommandKind::PingValidator),
            7 => Ok(CommandKind::Log),
            _ => Err(anyhow!("Unknown automation command kind {}", value)),
        }
    }
}

fn decode_action(kind: u8, payload: &[u8]) -> Result<AutomationAction> {
    match CommandKind::try_from(kind)? {
        CommandKind::LaunchChain => Ok(AutomationAction::LaunchChain(decode_launch(payload)?)),
        CommandKind::StartValidator => Ok(AutomationAction::StartValidator(decode_label(payload)?)),
        CommandKind::StopValidator => Ok(AutomationAction::StopValidator(decode_label(payload)?)),
        CommandKind::RestartValidator => {
            Ok(AutomationAction::RestartValidator(decode_label(payload)?))
        }
        CommandKind::LaunchValidator => Ok(AutomationAction::LaunchValidator {
            auto_start: decode_bool(payload)?,
        }),
        CommandKind::SpamJob => Ok(AutomationAction::SpamJob(decode_spam(payload)?)),
        CommandKind::PingValidator => Ok(AutomationAction::PingValidator(decode_label(payload)?)),
        CommandKind::Log => Ok(AutomationAction::Log(decode_log(payload)?)),
    }
}

fn decode_launch(payload: &[u8]) -> Result<LaunchRequest> {
    // Prefer the current 2-field payload: (count, basePort)
    let ty_strict = DynSolType::Tuple(vec![DynSolType::Uint(32), DynSolType::Uint(16)]);
    if let Ok(decoded) = ty_strict.abi_decode(payload) {
        if let Some(tuple) = decoded.as_tuple() {
            let count = as_u32(&tuple[0], 0, "count")? as usize;
            let base_port = as_u16(&tuple[1], 0, "basePort")?;
            return Ok(LaunchRequest { count, base_port });
        }
    }

    // Fallback to the legacy 3-field payload: (count, basePort, mode)
    let ty_strict_with_mode = DynSolType::Tuple(vec![
        DynSolType::Uint(32),
        DynSolType::Uint(16),
        DynSolType::String,
    ]);
    if let Ok(decoded) = ty_strict_with_mode.abi_decode(payload) {
        if let Some(tuple) = decoded.as_tuple() {
            let count = as_u32(&tuple[0], 0, "count")? as usize;
            let base_port = as_u16(&tuple[1], 0, "basePort")?;
            return Ok(LaunchRequest { count, base_port });
        }
    }

    // Fallback to full 256-bit ints in case sizes differ
    let ty_loose = DynSolType::Tuple(vec![DynSolType::Uint(256), DynSolType::Uint(256)]);
    let decoded = ty_loose
        .abi_decode(payload)
        .map_err(|err| anyhow!("Failed to decode launch payload: {err}"))?;
    let tuple = decoded
        .as_tuple()
        .ok_or_else(|| anyhow!("Launch payload was not a tuple"))?;
    let count = as_u32(&tuple[0], 0, "count")? as usize;
    let base_port = as_u16(&tuple[1], 0, "basePort")?;
    Ok(LaunchRequest { count, base_port })
}

fn decode_label(payload: &[u8]) -> Result<String> {
    let ty = DynSolType::Tuple(vec![DynSolType::String]);
    let decoded = ty
        .abi_decode(payload)
        .map_err(|err| anyhow!("Failed to decode label payload: {err}"))?;
    let tuple = decoded
        .as_tuple()
        .ok_or_else(|| anyhow!("Label payload invalid"))?;
    Ok(tuple[0]
        .as_str()
        .ok_or_else(|| anyhow!("Label missing string"))?
        .to_string())
}

fn decode_bool(payload: &[u8]) -> Result<bool> {
    let ty = DynSolType::Tuple(vec![DynSolType::Bool]);
    let decoded = ty
        .abi_decode(payload)
        .map_err(|err| anyhow!("Failed to decode bool payload: {err}"))?;
    let tuple = decoded
        .as_tuple()
        .ok_or_else(|| anyhow!("Bool payload invalid"))?;
    tuple[0]
        .as_bool()
        .ok_or_else(|| anyhow!("Bool payload missing value"))
}

fn decode_spam(payload: &[u8]) -> Result<SpamRequest> {
    let ty = DynSolType::Tuple(vec![
        DynSolType::Uint(64),
        DynSolType::Uint(64),
        DynSolType::Uint(64),
        DynSolType::Uint(32),
        DynSolType::String,
        DynSolType::String,
        DynSolType::Bool,
    ]);
    let decoded = ty
        .abi_decode(payload)
        .map_err(|err| anyhow!("Failed to decode spam payload: {err}"))?;
    let tuple = decoded
        .as_tuple()
        .ok_or_else(|| anyhow!("Spam payload invalid"))?;
    let total_txs = as_u64(&tuple[0], 0, "totalTxs")?;
    let parallel = as_u64(&tuple[1], 0, "parallel")?;
    let burst = as_u64(&tuple[2], 0, "burst")?;
    let accounts = as_u32(&tuple[3], 0, "accounts")? as usize;
    let mode_text = tuple[4]
        .as_str()
        .ok_or_else(|| anyhow!("Spam payload missing mode"))?;
    let mode =
        SpamMode::parse(mode_text).ok_or_else(|| anyhow!("Unsupported spam mode {}", mode_text))?;
    let target = tuple[5]
        .as_str()
        .ok_or_else(|| anyhow!("Spam payload missing target"))?
        .to_string();
    let has_target = tuple[6]
        .as_bool()
        .ok_or_else(|| anyhow!("Spam payload missing target flag"))?;
    Ok(SpamRequest {
        total_txs,
        parallel,
        burst,
        accounts,
        mode,
        target_url: if has_target && !target.trim().is_empty() {
            Some(target)
        } else {
            None
        },
    })
}

fn decode_log(payload: &[u8]) -> Result<String> {
    // Try tuple(string)
    let ty = DynSolType::Tuple(vec![DynSolType::String]);
    if let Ok(decoded) = ty.abi_decode(payload) {
        if let Some(tuple) = decoded.as_tuple() {
            if let Some(text) = tuple.first().and_then(|v| v.as_str()) {
                return Ok(text.to_string());
            }
        }
    }

    // Fallback to raw string
    if let Ok(decoded) = DynSolType::String.abi_decode(payload) {
        if let Some(text) = decoded.as_str() {
            return Ok(text.to_string());
        }
    }

    // Last resort: UTF-8 decode, otherwise hex
    if let Ok(text) = String::from_utf8(payload.to_vec()) {
        return Ok(text);
    }

    Ok(format!("0x{}", alloy_primitives::hex::encode(payload)))
}

fn as_u64(value: &DynSolValue, idx: usize, field: &str) -> Result<u64> {
    value
        .as_uint()
        .and_then(|(val, _)| u64::try_from(val).ok())
        .ok_or_else(|| anyhow!("Command {idx} missing {}", field))
}

fn as_u32(value: &DynSolValue, idx: usize, field: &str) -> Result<u32> {
    value
        .as_uint()
        .and_then(|(val, _)| u32::try_from(val).ok())
        .ok_or_else(|| anyhow!("Command {idx} missing {}", field))
}

fn as_u16(value: &DynSolValue, idx: usize, field: &str) -> Result<u16> {
    value
        .as_uint()
        .and_then(|(val, _)| u16::try_from(val).ok())
        .ok_or_else(|| anyhow!("Command {idx} missing {}", field))
}

fn as_u8(value: &DynSolValue, idx: usize, field: &str) -> Result<u8> {
    value
        .as_uint()
        .and_then(|(val, _)| u8::try_from(val).ok())
        .ok_or_else(|| anyhow!("Command {idx} missing {}", field))
}
