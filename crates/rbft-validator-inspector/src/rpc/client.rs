// SPDX-License-Identifier: Apache-2.0
use anyhow::{anyhow, Context, Result};
use futures::future::join_all;
use serde::Serialize;
use serde_json::Value;
use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;
use url::Url;

use crate::cli::Cli;
use crate::models::{AppEvent, PeerInfo, TxReceiptSummary, ValidatorReport, ValidatorTarget};

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: Vec<Value>,
}

pub async fn rpc_call(
    client: &reqwest::Client,
    url: &Url,
    method: &str,
    params: Vec<Value>,
) -> Result<Value> {
    let req = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method,
        params,
    };

    let resp = client
        .post(url.clone())
        .json(&req)
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("RPC error {status}"));
    }
    let json: Value = resp.json().await?;
    if let Some(err) = json.get("error") {
        return Err(anyhow!("RPC error: {err}"));
    }
    json.get("result")
        .cloned()
        .ok_or_else(|| anyhow!("RPC response missing result"))
}

pub async fn fetch_validator(
    client: &reqwest::Client,
    target: &ValidatorTarget,
) -> ValidatorReport {
    use std::time::Instant;

    let timestamp = Instant::now();
    let latency_start = Instant::now();
    let height_result = rpc_call(client, &target.url, "eth_blockNumber", vec![]).await;
    match height_result {
        Ok(value) => {
            let height = value
                .as_str()
                .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());
            let txs = match rpc_call(
                client,
                &target.url,
                "eth_getBlockTransactionCountByNumber",
                vec![Value::String("latest".to_string())],
            )
            .await
            {
                Ok(txs_value) => txs_value
                    .as_str()
                    .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok()),
                Err(e) => {
                    return ValidatorReport {
                        label: target.label.clone(),
                        height,
                        block_txs: None,
                        timestamp,
                        ok: false,
                        error: Some(format!("tx count failed: {e}")),
                        peer_count: None,
                        peers: Vec::new(),
                        latency: None,
                        latest_receipts: Vec::new(),
                    }
                }
            };

            let latency = Some(latency_start.elapsed());
            let (peer_count, peers) = fetch_peer_info(client, &target.url).await;
            let latest_receipts = fetch_receipts(client, &target.url).await;

            ValidatorReport {
                label: target.label.clone(),
                height,
                block_txs: txs,
                timestamp,
                ok: true,
                error: None,
                peer_count,
                peers,
                latency,
                latest_receipts,
            }
        }
        Err(e) => ValidatorReport {
            label: target.label.clone(),
            height: None,
            block_txs: None,
            timestamp,
            ok: false,
            error: Some(e.to_string()),
            peer_count: None,
            peers: Vec::new(),
            latency: None,
            latest_receipts: Vec::new(),
        },
    }
}

pub async fn fetch_all(
    client: &reqwest::Client,
    targets: &[ValidatorTarget],
) -> Vec<ValidatorReport> {
    if targets.is_empty() {
        return Vec::new();
    }
    let futures = targets.iter().map(|target| fetch_validator(client, target));
    join_all(futures).await
}

async fn fetch_peer_info(client: &reqwest::Client, url: &Url) -> (Option<usize>, Vec<PeerInfo>) {
    let peer_count = match rpc_call(client, url, "net_peerCount", vec![]).await {
        Ok(value) => value
            .as_str()
            .and_then(|hex| usize::from_str_radix(hex.trim_start_matches("0x"), 16).ok()),
        Err(_) => None,
    };

    let peers = match rpc_call(client, url, "admin_peers", vec![]).await {
        Ok(value) => value
            .as_array()
            .map(|arr| arr.iter().map(parse_peer_entry).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    (peer_count, peers)
}

fn parse_peer_entry(value: &Value) -> PeerInfo {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = value.get("name").and_then(|v| v.as_str()).map(String::from);
    let caps = value
        .get("caps")
        .and_then(|caps| caps.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let enode = value
        .get("enode")
        .and_then(|v| v.as_str())
        .map(String::from);
    let remote_address = value
        .get("remoteAddress")
        .and_then(|v| v.as_str())
        .map(String::from);
    let (remote_addr, remote_port) = if let Some(addr) = remote_address.clone() {
        parse_address(&addr)
    } else if let Some(enode_str) = enode.as_ref() {
        parse_enode(enode_str)
    } else {
        (None, None)
    };

    PeerInfo {
        id,
        name,
        caps,
        enode,
        remote_addr,
        remote_port,
    }
}

fn parse_address(addr: &str) -> (Option<String>, Option<u16>) {
    if let Some((host, port)) = addr.rsplit_once(':') {
        if let Ok(parsed) = port.parse::<u16>() {
            return (Some(host.trim_start_matches('/').to_string()), Some(parsed));
        }
    }
    (Some(addr.to_string()), None)
}

fn parse_enode(enode: &str) -> (Option<String>, Option<u16>) {
    if let Some(at_idx) = enode.find('@') {
        let after_at = &enode[at_idx + 1..];
        return parse_address(after_at);
    }
    (None, None)
}

async fn fetch_receipts(client: &reqwest::Client, url: &Url) -> Vec<TxReceiptSummary> {
    let block = match rpc_call(
        client,
        url,
        "eth_getBlockByNumber",
        vec![Value::String("latest".into()), Value::Bool(true)],
    )
    .await
    {
        Ok(block) => block,
        Err(_) => return Vec::new(),
    };

    let block_number = block
        .get("number")
        .and_then(|n| n.as_str())
        .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());

    let txs = block
        .get("transactions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut receipts = Vec::new();
    for tx in txs.into_iter().take(5) {
        let hash = match tx.get("hash").and_then(|h| h.as_str()) {
            Some(hash) => hash.to_string(),
            None => continue,
        };
        if let Ok(receipt) = rpc_call(
            client,
            url,
            "eth_getTransactionReceipt",
            vec![Value::String(hash.clone())],
        )
        .await
        {
            let status = receipt
                .get("status")
                .and_then(|s| s.as_str())
                .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
                .map(|v| v == 1);
            let gas_used = receipt
                .get("gasUsed")
                .and_then(|g| g.as_str())
                .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());
            let from = receipt
                .get("from")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let to = receipt
                .get("to")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let block_num = receipt
                .get("blockNumber")
                .and_then(|n| n.as_str())
                .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
                .or(block_number);

            receipts.push(TxReceiptSummary {
                tx_hash: hash,
                block_number: block_num,
                from,
                to,
                status,
                gas_used,
            });
        }
    }
    receipts
}

pub fn parse_targets(cli: &Cli) -> Result<Vec<ValidatorTarget>> {
    let entries = cli.rpc_entries.clone();

    let mut targets = Vec::with_capacity(entries.len());
    for (idx, entry) in entries.into_iter().enumerate() {
        let (label, url_str) = match entry.split_once('=') {
            Some((label, url)) if !label.trim().is_empty() => {
                (label.trim().to_string(), url.trim().to_string())
            }
            _ => (format!("v{idx}"), entry.trim().to_string()),
        };
        let url =
            Url::parse(&url_str).with_context(|| format!("invalid URL for validator {label}"))?;
        let port = url.port_or_known_default();
        let key = crate::config::load_validator_key(&label);
        targets.push(ValidatorTarget {
            label,
            url,
            port,
            key,
            log_path: None,
        });
    }

    targets.sort_by(|a, b| match a.label.cmp(&b.label) {
        Ordering::Equal => a.url.as_str().cmp(b.url.as_str()),
        other => other,
    });

    let max = cli.max_validators.max(1);
    targets.truncate(max);
    Ok(targets)
}

pub async fn spawn_fetcher(
    tx: mpsc::Sender<AppEvent>,
    targets: Arc<RwLock<Vec<ValidatorTarget>>>,
    refresh: Duration,
) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build HTTP client");

    let mut ticker = interval(refresh);
    loop {
        let current_targets = targets.read().await.clone();
        if current_targets.is_empty() {
            if tx.send(AppEvent::Data(Vec::new())).await.is_err() {
                break;
            }
            ticker.tick().await;
            continue;
        }

        let reports = fetch_all(&client, &current_targets).await;
        if tx.send(AppEvent::Data(reports)).await.is_err() {
            break;
        }
        ticker.tick().await;
    }
}
