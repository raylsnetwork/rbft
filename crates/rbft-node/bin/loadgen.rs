// SPDX-License-Identifier: Apache-2.0
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

use alloy_primitives::{Address, U256};
use alloy_provider::network::TransactionBuilder;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use rbft_utils::constants::DEFAULT_ADMIN_KEY;
use clap::Parser;
use eyre::Result;
use tokio::sync::Semaphore;
use tokio::time::{sleep, Duration, MissedTickBehavior};

/// Blast transactions at one or more RBFT RPC endpoints and report observed TPS.
#[derive(Debug, Parser, Clone)]
struct Args {
    /// Comma-separated RPC URLs to load-balance across.
    #[arg(
        long = "rpc-urls",
        value_delimiter = ',',
        num_args = 1..,
        default_value = "http://localhost:8545"
    )]
    rpc_urls: Vec<String>,

    /// Duration to send transactions (seconds).
    #[arg(long, default_value = "30")]
    duration_secs: u64,

    /// Target transactions per second to submit.
    #[arg(long, default_value = "1000")]
    target_tps: u64,

    /// Maximum in-flight submissions to avoid unbounded memory/queue growth.
    #[arg(long, default_value = "2000")]
    max_in_flight: usize,

    /// Gas limit per transaction.
    #[arg(long, default_value = "21000")]
    gas_limit: u64,

    /// Max priority fee per gas (wei).
    #[arg(long, default_value = "1000000000")]
    max_priority_fee: u128,

    /// Max fee per gas (wei).
    #[arg(long, default_value = "20000000000")]
    max_fee: u128,

    /// Transfer value in wei.
    #[arg(long, default_value = "0")]
    value_wei: u64,

    /// Expected block interval (seconds) for throughput calculation.
    #[arg(long, default_value = "2.0")]
    block_interval: f64,

    /// Time to wait after sending to let blocks include the tail of the batch.
    #[arg(long, default_value = "5")]
    settle_secs: u64,

    /// Maximum retries when the node signals backpressure (txpool full / too many connections).
    #[arg(long, default_value = "5")]
    max_backpressure_retries: u32,

    /// Initial backoff in milliseconds when retrying on backpressure.
    #[arg(long, default_value = "50")]
    backpressure_backoff_ms: u64,
}

fn make_target_address(i: u64) -> Address {
    // Encode the counter in the low bytes to create distinct recipients.
    let mut bytes = [0u8; 20];
    bytes[12..].copy_from_slice(&i.to_be_bytes());
    Address::from(bytes)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.target_tps == 0 {
        eyre::bail!("target_tps must be > 0");
    }

    println!(
        "Starting loadgen: urls={:?}, duration={}s, target_tps={}, max_in_flight={}, gas_limit={}",
        args.rpc_urls, args.duration_secs, args.target_tps, args.max_in_flight, args.gas_limit
    );

    // Use the pre-funded test key baked into genesis.
    let spam_key: PrivateKeySigner = DEFAULT_ADMIN_KEY
        .parse()
        .expect("invalid spammer private key");
    let spam_from = spam_key.address();

    // Build providers with the wallet attached for signing.
    let mut providers = Vec::with_capacity(args.rpc_urls.len());
    for url in &args.rpc_urls {
        let provider = ProviderBuilder::new()
            .wallet(spam_key.clone())
            .connect_http(url.parse()?);
        providers.push(provider);
    }

    let primary = providers[0].clone();
    let chain_id = primary.get_chain_id().await?;
    let start_block = primary.get_block_number().await?;
    let start_nonce = primary.get_transaction_count(spam_from).await?;
    let start_wall = Instant::now();

    println!(
        "chain_id={}, start_block={}, starting nonce={} for {}",
        chain_id, start_block, start_nonce, spam_from
    );

    let total_to_send = args.target_tps * args.duration_secs;
    let interval_micros = ((1_000_000f64 / args.target_tps as f64).max(1.0)).round() as u64;
    let mut ticker = tokio::time::interval(Duration::from_micros(interval_micros));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let next_nonce = Arc::new(AtomicU64::new(start_nonce));
    let success = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let semaphore = Arc::new(Semaphore::new(args.max_in_flight));

    let mut handles = Vec::with_capacity(total_to_send as usize);
    for i in 0..total_to_send {
        ticker.tick().await;

        let permit = semaphore.clone().acquire_owned().await?;
        let provider = providers[(i as usize) % providers.len()].clone();
        let nonce = next_nonce.fetch_add(1, Ordering::Relaxed);
        let to = make_target_address(i + 1);
        let success = success.clone();
        let failed = failed.clone();

        let gas_limit = args.gas_limit;
        let max_priority_fee = args.max_priority_fee;
        let max_fee = args.max_fee;
        let value = args.value_wei;
        let max_retries = args.max_backpressure_retries;
        let mut backoff_ms = args.backpressure_backoff_ms;
        let h = tokio::spawn(async move {
            let mut attempts = 0;
            loop {
                let tx = TransactionRequest::default()
                    .with_to(to)
                    .with_from(spam_from)
                    .with_nonce(nonce)
                    .with_chain_id(chain_id)
                    .with_value(U256::from(value))
                    .with_gas_limit(gas_limit)
                    .with_max_priority_fee_per_gas(max_priority_fee)
                    .with_max_fee_per_gas(max_fee);

                let res = provider.send_transaction(tx).await;
                match res {
                    Ok(_) => {
                        success.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        let is_backpressure = msg.contains("txpool is full")
                            || msg.contains("Too many connections")
                            || msg.contains("connection refused");
                        if is_backpressure && attempts < max_retries {
                            attempts += 1;
                            eprintln!(
                                "send backpressure (attempt {attempts}/{max_retries}): {msg}, \
                                 backing off {backoff_ms}ms"
                            );
                            sleep(Duration::from_millis(backoff_ms)).await;
                            backoff_ms = (backoff_ms.saturating_mul(2)).min(5_000);
                            continue;
                        }
                        failed.fetch_add(1, Ordering::Relaxed);
                        eprintln!("send error: {msg}");
                        break;
                    }
                }
            }
            drop(permit);
        });
        handles.push(h);
    }

    for h in handles {
        let _ = h.await;
    }

    println!(
        "Submitted {} txs (ok: {}, failed: {}), waiting {}s for inclusion...",
        total_to_send,
        success.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        args.settle_secs
    );

    sleep(Duration::from_secs(args.settle_secs)).await;

    let end_block = primary.get_block_number().await?;
    let mut total_included: usize = 0;
    let mut max_block_txs = 0usize;
    for n in (start_block + 1)..=end_block {
        if let Some(block) = primary.get_block(n.into()).await? {
            let txs = block.transactions.len();
            total_included += txs;
            if txs > max_block_txs {
                max_block_txs = txs;
            }
        }
    }

    let blocks_elapsed = end_block.saturating_sub(start_block).max(1);
    let chain_window_secs = blocks_elapsed as f64 * args.block_interval;
    let observed_tps = total_included as f64 / chain_window_secs;
    let wall_tps = total_included as f64 / start_wall.elapsed().as_secs_f64().max(0.001);

    println!("--- loadgen report ---");
    println!(
        "blocks_seen: {} -> {} ({} blocks)",
        start_block, end_block, blocks_elapsed
    );
    println!("included txs: {}", total_included);
    println!("max txs in any block: {}", max_block_txs);
    println!(
        "tps (chain window, {}s per block): {:.2}",
        args.block_interval, observed_tps
    );
    println!("tps (wall clock): {:.2}", wall_tps);

    Ok(())
}
