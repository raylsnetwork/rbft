// SPDX-License-Identifier: Apache-2.0
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use url::Url;

use super::events::{LifecycleState, StatusClass};

#[derive(Clone, Debug)]
pub struct ValidatorTarget {
    pub label: String,
    pub url: Url,
    pub port: Option<u16>,
    pub key: Option<String>,
    pub log_path: Option<PathBuf>,
}

impl ValidatorTarget {
    pub fn http_port(&self) -> Option<u16> {
        self.port
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub id: String,
    pub name: Option<String>,
    pub caps: Vec<String>,
    pub enode: Option<String>,
    pub remote_addr: Option<String>,
    pub remote_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct TxReceiptSummary {
    pub tx_hash: String,
    pub block_number: Option<u64>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub status: Option<bool>,
    pub gas_used: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ValidatorReport {
    pub label: String,
    pub height: Option<u64>,
    pub block_txs: Option<u64>,
    pub timestamp: Instant,
    pub ok: bool,
    pub error: Option<String>,
    pub peer_count: Option<usize>,
    pub peers: Vec<PeerInfo>,
    pub latency: Option<Duration>,
    pub latest_receipts: Vec<TxReceiptSummary>,
}

#[derive(Debug, Clone)]
pub struct ValidatorEntry {
    pub target: ValidatorTarget,
    pub height: Option<u64>,
    pub total_txs: u64,
    pub last_ok: Option<Instant>,
    pub last_update: Option<Instant>,
    pub last_error: Option<String>,
    pub lifecycle: Option<LifecycleState>,
    pub peer_count: Option<usize>,
    pub peers: Vec<PeerInfo>,
    pub latency: Option<Duration>,
    pub receipts: VecDeque<TxReceiptSummary>,
}

impl ValidatorEntry {
    pub fn new(target: ValidatorTarget) -> Self {
        Self {
            target,
            height: None,
            total_txs: 0,
            last_ok: None,
            last_update: None,
            last_error: None,
            lifecycle: None,
            peer_count: None,
            peers: Vec::new(),
            latency: None,
            receipts: VecDeque::new(),
        }
    }

    pub fn apply_report(&mut self, report: ValidatorReport) {
        self.last_update = Some(report.timestamp);
        if report.ok {
            self.last_ok = Some(report.timestamp);
            if let Some(new_height) = report.height {
                if let Some(prev_height) = self.height {
                    if let Some(block_txs) = report.block_txs {
                        if new_height > prev_height {
                            self.total_txs = self.total_txs.saturating_add(block_txs);
                        }
                    }
                } else if let Some(block_txs) = report.block_txs {
                    self.total_txs = block_txs;
                }
            }
            self.height = report.height;
            self.last_error = None;
        } else {
            self.last_ok = None;
            self.last_error = report.error;
        }
        self.peer_count = report.peer_count;
        self.peers = report.peers;
        self.latency = report.latency;
        const MAX_RECEIPTS: usize = 512;
        for receipt in report.latest_receipts {
            if self.receipts.iter().any(|r| r.tx_hash == receipt.tx_hash) {
                continue;
            }
            if self.receipts.len() >= MAX_RECEIPTS {
                self.receipts.pop_front();
            }
            self.receipts.push_back(receipt);
        }
    }

    pub fn status_class(&self, now: Instant) -> StatusClass {
        const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);
        const DOWN_AFTER: std::time::Duration = std::time::Duration::from_secs(600);

        match self.last_ok {
            Some(last) => {
                let age = now.saturating_duration_since(last);
                if age <= STALE_AFTER {
                    StatusClass::Up
                } else if age <= DOWN_AFTER {
                    StatusClass::Stale
                } else {
                    StatusClass::Down
                }
            }
            None => StatusClass::Down,
        }
    }

    pub fn height_str(&self) -> String {
        self.height
            .map(|h| h.to_string())
            .unwrap_or_else(|| "-".to_string())
    }

    pub fn txs_str(&self) -> String {
        if self.total_txs == 0 {
            "-".to_string()
        } else {
            self.total_txs.to_string()
        }
    }

    pub fn set_lifecycle(&mut self, state: Option<LifecycleState>) {
        self.lifecycle = state;
    }
}
