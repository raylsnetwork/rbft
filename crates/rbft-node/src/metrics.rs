// SPDX-License-Identifier: Apache-2.0
use prometheus::{register_int_gauge_with_registry, Encoder, IntGauge, Registry, TextEncoder};
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use warp::{http::StatusCode, Filter};

const DEFAULT_BLOCK_INTERVAL_MS: u64 = 1_000;
const STALL_MULTIPLIER: u64 = 5;
const MIN_STALL_THRESHOLD_MS: u64 = 30_000;

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub status: &'static str,
    pub block_height: u64,
    pub round: u64,
    pub is_proposer: bool,
    pub last_block_timestamp_ms: u64,
    pub millis_since_last_block: u64,
    pub expected_block_interval_ms: u64,
    pub stall_threshold_ms: u64,
}

pub struct QbftMetrics {
    pub block_height: IntGauge,
    pub round: IntGauge,
    pub is_proposer: IntGauge,
    pub registry: Registry,
    last_block_height: AtomicU64,
    last_block_at_ms: AtomicU64,
    expected_block_interval_ms: AtomicU64,
}

impl QbftMetrics {
    pub fn new() -> Arc<Self> {
        let registry = Registry::new();

        let block_height = register_int_gauge_with_registry!(
            "qbft_block_height",
            "Current block height for this node",
            &registry
        )
        .expect("failed to register qbft_block_height gauge");

        let round =
            register_int_gauge_with_registry!("qbft_round", "Current QBFT round number", &registry)
                .expect("failed to register qbft_round gauge");

        let is_proposer = register_int_gauge_with_registry!(
            "qbft_is_proposer",
            "Whether this node is the proposer (1=yes, 0=no)",
            &registry
        )
        .expect("failed to register qbft_is_proposer gauge");

        Arc::new(Self {
            block_height,
            round,
            is_proposer,
            registry,
            last_block_height: AtomicU64::new(0),
            last_block_at_ms: AtomicU64::new(now_ms()),
            expected_block_interval_ms: AtomicU64::new(DEFAULT_BLOCK_INTERVAL_MS),
        })
    }

    pub fn record_consensus_state(&self, block_height: u64, round: u64, is_proposer: bool) {
        let previous_height = self.last_block_height.swap(block_height, Ordering::Relaxed);
        if block_height > previous_height {
            self.last_block_at_ms.store(now_ms(), Ordering::Relaxed);
        }

        self.block_height.set(block_height as i64);
        self.round.set(round as i64);
        self.is_proposer.set(if is_proposer { 1 } else { 0 });
    }

    fn stall_threshold_ms(&self) -> u64 {
        let interval = self.expected_block_interval_ms.load(Ordering::Relaxed);
        let expected = interval.max(1);
        let threshold = expected.saturating_mul(STALL_MULTIPLIER);
        threshold.max(MIN_STALL_THRESHOLD_MS)
    }

    pub fn health_status(&self) -> HealthStatus {
        let now = now_ms();
        let last_block_ms = self.last_block_at_ms.load(Ordering::Relaxed);
        let millis_since_last_block = now.saturating_sub(last_block_ms);
        let expected_block_interval_ms = self.expected_block_interval_ms.load(Ordering::Relaxed);
        let stall_threshold_ms = self.stall_threshold_ms();
        let status = if millis_since_last_block > stall_threshold_ms {
            "stalled"
        } else {
            "ok"
        };

        HealthStatus {
            status,
            block_height: self.block_height.get() as u64,
            round: self.round.get() as u64,
            is_proposer: self.is_proposer.get() == 1,
            last_block_timestamp_ms: last_block_ms,
            millis_since_last_block,
            expected_block_interval_ms,
            stall_threshold_ms,
        }
    }

    pub fn serve(self: Arc<Self>, port: u16) {
        tokio::spawn(async move {
            let metrics = self.clone();
            let metrics_route = warp::path("metrics").and(warp::get()).map(move || {
                let mut buffer = Vec::new();
                let encoder = TextEncoder::new();
                let mf = metrics.registry.gather();
                encoder
                    .encode(&mf, &mut buffer)
                    .expect("failed to encode Prometheus metrics");
                warp::reply::with_header(buffer, "Content-Type", encoder.format_type())
            });

            let health = self.clone();
            let health_route = warp::path("health").and(warp::get()).map(move || {
                let status = health.health_status();
                let code = if status.status == "ok" {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                warp::reply::with_status(warp::reply::json(&status), code)
            });

            println!(
                "→ Metrics available at http://localhost:{}/metrics (health at /health)",
                port
            );

            let routes = metrics_route.or(health_route);
            warp::serve(routes).run(([0, 0, 0, 0], port)).await;
        });
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("cannot be earlier than UNIX_EPOCH")
        .as_millis() as u64
}
