// SPDX-License-Identifier: Apache-2.0
use std::sync::Arc;
use std::time::Duration;

use alloy_network::{eip2718::Encodable2718, EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use clap::Parser;
use futures::stream::{self, StreamExt};
use rand::SeedableRng;
use rbft_utils::constants::DEFAULT_ADMIN_KEY;
use tokio::sync::RwLock;

/// Ethereum transaction testing tool
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Spam transactions to an RPC endpoint
    Spam(SpamArgs),
}

/// Arguments for spamming transactions
#[derive(Parser, Debug)]
struct SpamArgs {
    /// Number of transactions to send
    #[arg(short = 'n', long, default_value = "20000")]
    num_txs: u64,

    /// Private key for the funding account (hex string with or without 0x prefix)
    /// This account must have sufficient balance to fund generated accounts.
    #[arg(
        short = 'k',
        long,
        env = "RBFT_ADMIN_KEY",
        default_value_t = DEFAULT_ADMIN_KEY.to_string()
    )]
    admin_key: String,

    /// Recipient address for spam transactions
    #[arg(
        short = 't',
        long,
        default_value = "0xE4924AE4907A883EE6C3295b1340929983DBd8E5"
    )]
    to_address: String,

    /// RPC endpoint URLs (comma-separated for multiple nodes)
    #[arg(short = 'u', long, default_value = "http://localhost:8545")]
    urls: String,

    /// Batch size (number of transactions per request)
    #[arg(short = 'b', long, default_value = "1000")]
    batch_size: usize,

    /// Max transactions per account (auto-calculates number of accounts).
    /// Default 16 matches Reth's default max-account-slots.
    #[arg(long, default_value = "16")]
    max_txs_per_account: u64,

    /// Amount of ETH to fund each generated account (in wei).
    /// Default: 1 ETH = 1000000000000000000 wei
    #[arg(long, default_value = "1000000000000000000")]
    funding_amount: String,

    /// Target transactions per second (0 = unlimited).
    /// Limits sending rate to avoid overwhelming node pools.
    #[arg(long, default_value = "0")]
    target_tps: u64,

    /// Max seconds to wait for all transactions to appear in blocks (0 = wait forever).
    #[arg(long, default_value = "0")]
    max_wait_seconds: u64,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    match args.command {
        Commands::Spam(spam_args) => spam(spam_args).await?,
    }

    Ok(())
}

/// Parse comma-separated URLs into a vector
fn parse_urls(urls: &str) -> Vec<String> {
    urls.split(',').map(|s| s.trim().to_string()).collect()
}

/// Generate N random accounts using a seeded RNG for reproducibility
fn generate_accounts(count: usize) -> Vec<PrivateKeySigner> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    (0..count)
        .map(|_| PrivateKeySigner::random_with(&mut rng))
        .collect()
}

/// Fund generated accounts from the funder account.
/// Funds accounts incrementally in chunks to avoid hitting pool limits.
async fn fund_accounts(
    provider: &impl Provider,
    client: &reqwest::Client,
    url: &str,
    funder: &PrivateKeySigner,
    accounts: &[PrivateKeySigner],
    amount_per_account: U256,
) -> eyre::Result<()> {
    let funder_address = funder.address();
    let chain_id = 123123u64;
    let gas_limit = 21000u64;
    let gas_price = U256::from(1_000_000_000u64); // 1 gwei

    // Get starting nonce for funder
    let mut current_nonce = provider.get_transaction_count(funder_address).await?;
    eprintln!(
        "Funding {} accounts from {} (nonce: {})",
        accounts.len(),
        funder_address,
        current_nonce
    );

    // Fund accounts in chunks to avoid hitting pool limits.
    // We send a chunk, wait for it to be mostly confirmed, then send the next chunk.
    // This ensures we never have more pending than the pool can handle.
    let chunk_size = 5000usize; // Send 5000 funding txs at a time
    let send_batch_size = 100usize; // RPC batch size for sending
    let mut funded_count = 0usize;

    for chunk in accounts.chunks(chunk_size) {
        let chunk_start_nonce = current_nonce;
        eprintln!(
            "Funding chunk: accounts {}-{} (nonces {}-{})",
            funded_count,
            funded_count + chunk.len() - 1,
            chunk_start_nonce,
            chunk_start_nonce + chunk.len() as u64 - 1
        );

        // Build funding transactions for this chunk
        let funding_txs: Vec<Vec<u8>> = stream::iter(chunk.iter().enumerate())
            .map(|(i, account)| {
                let nonce = chunk_start_nonce + i as u64;
                let to_address = account.address();
                let funder = funder.clone();

                async move {
                    let tx = TransactionRequest::default()
                        .with_from(funder_address)
                        .with_to(to_address)
                        .with_nonce(nonce)
                        .with_chain_id(chain_id)
                        .with_gas_limit(gas_limit)
                        .with_gas_price(gas_price.to::<u128>())
                        .with_value(amount_per_account);

                    let wallet = EthereumWallet::from(funder);
                    let tx_envelope = tx.build(&wallet).await.expect("Failed to build funding tx");
                    tx_envelope.encoded_2718().to_vec()
                }
            })
            .buffered(16)
            .collect()
            .await;

        // Send funding transactions in batches
        for (batch_idx, batch) in funding_txs.chunks(send_batch_size).enumerate() {
            let batch_request =
                create_batch_request(batch, funded_count + batch_idx * send_batch_size);

            let mut retries = 0;
            loop {
                match client.post(url).json(&batch_request).send().await {
                    Ok(response) if response.status().is_success() => {
                        break;
                    }
                    Ok(response) => {
                        eprintln!(
                            "Funding batch {}: HTTP error {}, retrying...",
                            batch_idx,
                            response.status()
                        );
                        retries += 1;
                        if retries >= 5 {
                            return Err(eyre::eyre!(
                                "Failed to send funding batch after 5 retries"
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Err(e) => {
                        eprintln!(
                            "Funding batch {}: request error {}, retrying...",
                            batch_idx, e
                        );
                        retries += 1;
                        if retries >= 5 {
                            return Err(eyre::eyre!(
                                "Failed to send funding batch after 5 retries"
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }

        // Wait for this chunk to be confirmed before sending next chunk
        let expected_nonce = chunk_start_nonce + chunk.len() as u64;
        let timeout = std::time::Instant::now() + Duration::from_secs(120);

        while std::time::Instant::now() < timeout {
            let confirmed_nonce = provider.get_transaction_count(funder_address).await?;
            if confirmed_nonce >= expected_nonce {
                break;
            }
            let confirmed_in_chunk = confirmed_nonce.saturating_sub(chunk_start_nonce);
            eprintln!(
                "Funding progress: {}/{} total ({}/{} in current chunk)",
                funded_count as u64 + confirmed_in_chunk,
                accounts.len(),
                confirmed_in_chunk,
                chunk.len()
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // Verify chunk was confirmed
        let final_nonce = provider.get_transaction_count(funder_address).await?;
        if final_nonce < expected_nonce {
            return Err(eyre::eyre!(
                "Timeout waiting for funding chunk to confirm. Expected nonce {}, got {}",
                expected_nonce,
                final_nonce
            ));
        }

        funded_count += chunk.len();
        current_nonce = final_nonce;
        eprintln!(
            "Chunk complete: {}/{} accounts funded",
            funded_count,
            accounts.len()
        );
    }

    eprintln!("All {} funding transactions confirmed", accounts.len());
    Ok(())
}

/// Validate that the node's txpool can hold max_txs_per_account transactions per account.
/// This probes the node by sending test transactions and checking if they all fit in the pool.
async fn validate_pool_capacity(
    provider: &impl Provider,
    client: &reqwest::Client,
    url: &str,
    funder: &PrivateKeySigner,
    max_txs_per_account: u64,
) -> eyre::Result<()> {
    eprintln!(
        "Validating node pool capacity for {} txs per account...",
        max_txs_per_account
    );

    // Generate a unique probe account (different seed from main accounts)
    let mut rng = rand::rngs::StdRng::seed_from_u64(999999);
    let probe_account = PrivateKeySigner::random_with(&mut rng);
    let probe_address = probe_account.address();

    // Fund the probe account
    let funder_address = funder.address();
    let chain_id = 123123u64;
    let gas_limit = 21000u64;
    let gas_price = U256::from(1_000_000_000u64);
    let funding_amount = U256::from(1_000_000_000_000_000_000u128); // 1 ETH

    let funder_nonce = provider.get_transaction_count(funder_address).await?;

    let funding_tx = TransactionRequest::default()
        .with_from(funder_address)
        .with_to(probe_address)
        .with_nonce(funder_nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(gas_limit)
        .with_gas_price(gas_price.to::<u128>())
        .with_value(funding_amount);

    let wallet = EthereumWallet::from(funder.clone());
    let tx_envelope = funding_tx.build(&wallet).await?;
    let encoded = tx_envelope.encoded_2718();

    // Send funding transaction
    let batch_request = vec![serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "eth_sendRawTransaction",
        "params": [format!("0x{}", hex::encode(&encoded))]
    })];

    client.post(url).json(&batch_request).send().await?;

    // Wait for funding to confirm
    let timeout = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < timeout {
        let balance = provider.get_balance(probe_address).await?;
        if balance >= funding_amount {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Build probe transactions
    let probe_wallet = EthereumWallet::from(probe_account.clone());
    let mut probe_txs: Vec<Vec<u8>> = Vec::with_capacity(max_txs_per_account as usize);

    for nonce in 0..max_txs_per_account {
        let tx = TransactionRequest::default()
            .with_from(probe_address)
            .with_to(funder_address) // Send back to funder
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_gas_limit(gas_limit)
            .with_gas_price(gas_price.to::<u128>())
            .with_value(U256::from(1u64));

        let tx_envelope = tx.build(&probe_wallet).await?;
        probe_txs.push(tx_envelope.encoded_2718().to_vec());
    }

    // Send all probe transactions at once
    let batch_request: Vec<serde_json::Value> = probe_txs
        .iter()
        .enumerate()
        .map(|(i, tx)| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "eth_sendRawTransaction",
                "params": [format!("0x{}", hex::encode(tx))]
            })
        })
        .collect();

    let response = client.post(url).json(&batch_request).send().await?;

    // Check the actual RPC responses for pool-related errors
    let mut accepted = 0u64;
    let mut pool_full_errors = 0u64;
    let mut other_errors = 0u64;

    if response.status().is_success() {
        if let Ok(body) = response.text().await {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(&body) {
                for result in &results {
                    if let Some(error) = result.get("error") {
                        let msg = error.get("message").and_then(|m| m.as_str()).unwrap_or("");
                        if msg.contains("txpool is full")
                            || msg.contains("exceeds txpool max")
                            || msg.contains("account slots full")
                        {
                            pool_full_errors += 1;
                        } else if !msg.contains("already known") && !msg.contains("nonce too low") {
                            other_errors += 1;
                        } else {
                            // "already known" or "nonce too low" means tx was accepted before
                            accepted += 1;
                        }
                    } else {
                        accepted += 1;
                    }
                }
            }
        }
    }

    // If there were pool-related errors, the node's limit is too low
    if pool_full_errors > 0 {
        return Err(eyre::eyre!(
            "Node's txpool.max-account-slots is less than --max-txs-per-account ({}).\nThe node \
             rejected {} out of {} probe transactions with pool-related errors.\nEither:\n- \
             Reduce --max-txs-per-account to {}, or\n- Increase node's --txpool.max-account-slots \
             to at least {}",
            max_txs_per_account,
            pool_full_errors,
            max_txs_per_account,
            accepted,
            max_txs_per_account
        ));
    }

    if other_errors > 0 {
        eprintln!(
            "  Warning: {} probe transactions had unexpected errors",
            other_errors
        );
    }

    eprintln!(
        "  Pool capacity validated: {} txs accepted per account",
        accepted
    );

    // Wait for probe transactions to be mined before continuing
    eprintln!("  Waiting for probe transactions to clear...");
    let timeout = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < timeout {
        let confirmed_nonce = provider.get_transaction_count(probe_address).await?;
        if confirmed_nonce >= max_txs_per_account {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("  Probe transactions cleared");

    Ok(())
}

/// Build transactions distributed across multiple accounts
async fn build_transactions_multi_account(
    provider: &impl Provider,
    num_txs: u64,
    accounts: &[PrivateKeySigner],
    to_address: &str,
    max_txs_per_account: u64,
) -> eyre::Result<Vec<(usize, Vec<u8>)>> {
    let to_address: Address = to_address.parse().expect("to_address");
    let chain_id = 123123u64;
    let gas_limit = 21000u64;
    let gas_price = U256::from(1_000_000_000u64); // 1 gwei
    let value = U256::from(1u64);

    // Fetch current on-chain nonces for each account
    eprintln!(
        "  Fetching on-chain nonces for {} accounts...",
        accounts.len()
    );
    let mut account_nonces: Vec<u64> = Vec::with_capacity(accounts.len());
    for account in accounts {
        let nonce = provider.get_transaction_count(account.address()).await?;
        account_nonces.push(nonce);
    }
    eprintln!(
        "  Nonces fetched (first: {}, last: {})",
        account_nonces.first().unwrap_or(&0),
        account_nonces.last().unwrap_or(&0)
    );

    // Track how many txs we've assigned to each account (separate from on-chain nonces)
    let mut txs_per_account: Vec<u64> = vec![0; accounts.len()];

    // Build transactions, distributing across accounts
    let mut tx_assignments: Vec<(usize, u64)> = Vec::with_capacity(num_txs as usize);

    for _ in 0..num_txs {
        // Find account with fewest assigned txs that hasn't hit the limit
        let account_idx = txs_per_account
            .iter()
            .enumerate()
            .filter(|(_, &count)| count < max_txs_per_account)
            .min_by_key(|(_, &count)| count)
            .map(|(idx, _)| idx)
            .expect("Not enough accounts for transactions");

        // Use on-chain nonce + number of txs already assigned
        let nonce = account_nonces[account_idx] + txs_per_account[account_idx];
        tx_assignments.push((account_idx, nonce));
        txs_per_account[account_idx] += 1;
    }

    // Build transactions concurrently
    let raw_transactions: Vec<(usize, Vec<u8>)> = stream::iter(tx_assignments.into_iter())
        .enumerate()
        .map(|(i, (account_idx, nonce))| {
            let signer = accounts[account_idx].clone();
            let from_address = signer.address();

            async move {
                let tx = TransactionRequest::default()
                    .with_from(from_address)
                    .with_to(to_address)
                    .with_nonce(nonce)
                    .with_chain_id(chain_id)
                    .with_gas_limit(gas_limit)
                    .with_gas_price(gas_price.to::<u128>())
                    .with_value(value);

                let wallet = EthereumWallet::from(signer);
                let tx_envelope = tx.build(&wallet).await.expect("Failed to build tx");
                let encoded = tx_envelope.encoded_2718().to_vec();

                if i % 10000 == 0 && i > 0 {
                    eprintln!("  Built {} transactions...", i);
                }

                (account_idx, encoded)
            }
        })
        .buffered(16)
        .collect()
        .await;

    Ok(raw_transactions)
}

fn create_batch_request(raw_transactions: &[Vec<u8>], id_offset: usize) -> Vec<serde_json::Value> {
    raw_transactions
        .iter()
        .enumerate()
        .map(|(id, raw_tx)| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id_offset + id,
                "method": "eth_sendRawTransaction",
                "params": [format!("0x{}", hex::encode(raw_tx))]
            })
        })
        .collect()
}

async fn monitor_blocks(
    provider: impl Provider,
    expected_tx_count: u64,
    max_wait_seconds: u64,
    monitoring_active: Arc<RwLock<bool>>,
) {
    let mut last_block_number: Option<u64> = None;
    let mut last_block_time: Option<std::time::Instant> = None;
    let mut cumulative_tx_count = 0u64;
    let mut peak_tps = 0.0f64;
    let start_time = std::time::Instant::now();
    let mut stop_deadline: Option<std::time::Instant> = None;

    // Get initial block number
    if let Ok(block_num) = provider.get_block_number().await {
        last_block_number = Some(block_num);
        eprintln!("Starting block number: {}", block_num);
    }

    loop {
        // Wait 100ms before checking for new blocks
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Check for new block
        if let Ok(current_block_number) = provider.get_block_number().await {
            if let Some(last_num) = last_block_number {
                if current_block_number > last_num {
                    // New block(s) detected
                    for block_num in (last_num + 1)..=current_block_number {
                        // Fetch the block header and transaction count
                        if let Ok(Some(block)) = provider.get_block(block_num.into()).await {
                            let tx_count = block.transactions.len();
                            cumulative_tx_count += tx_count as u64;

                            let block_time_ms = if let Some(last_time) = last_block_time {
                                last_time.elapsed().as_millis()
                            } else {
                                0
                            };

                            // Calculate TPS for this block
                            let tps = if block_time_ms > 0 {
                                (tx_count as f64 * 1000.0) / block_time_ms as f64
                            } else {
                                0.0
                            };

                            // Update peak TPS
                            if tps > peak_tps {
                                peak_tps = tps;
                            }

                            eprintln!(
                                "Block #{}: {} transactions, {}ms since last block, timestamp: \
                                 {}, TPS: {:.2} (total seen: {}/{})",
                                block_num,
                                tx_count,
                                block_time_ms,
                                block.header.timestamp,
                                tps,
                                cumulative_tx_count,
                                expected_tx_count
                            );

                            last_block_time = Some(std::time::Instant::now());
                        }
                    }
                    last_block_number = Some(current_block_number);
                }
            } else {
                last_block_number = Some(current_block_number);
            }
        }

        // Stop once we've seen all expected transactions after sending completes.
        if !*monitoring_active.read().await {
            if stop_deadline.is_none() && max_wait_seconds > 0 {
                stop_deadline =
                    Some(std::time::Instant::now() + Duration::from_secs(max_wait_seconds));
            }

            if cumulative_tx_count >= expected_tx_count {
                eprintln!(
                    "All {} transactions seen in blocks, stopping",
                    expected_tx_count
                );
                break;
            }

            if let Some(deadline) = stop_deadline {
                if std::time::Instant::now() >= deadline {
                    eprintln!(
                        "Max wait {}s reached (seen {}/{}), stopping",
                        max_wait_seconds, cumulative_tx_count, expected_tx_count
                    );
                    break;
                }
            }
        }
    }

    let total_elapsed = start_time.elapsed();
    let avg_tps = if total_elapsed.as_secs() > 0 {
        cumulative_tx_count as f64 / total_elapsed.as_secs_f64()
    } else {
        0.0
    };

    if let Some(final_block) = last_block_number {
        eprintln!(
            "Stopped at block #{} with {} transactions seen",
            final_block, cumulative_tx_count
        );
    }

    eprintln!("\n=== Performance Summary ===");
    eprintln!("Total transactions: {}", cumulative_tx_count);
    eprintln!("Total time: {:.2}s", total_elapsed.as_secs_f64());
    eprintln!("Average TPS: {:.2}", avg_tps);
    eprintln!("Peak TPS: {:.2}", peak_tps);

    // Output JSON to stdout
    let result = serde_json::json!({
        "total_transactions": cumulative_tx_count,
        "total_time_seconds": total_elapsed.as_secs_f64(),
        "average_tps": avg_tps,
        "peak_tps": peak_tps
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

async fn spam(args: SpamArgs) -> eyre::Result<()> {
    // Parse URLs for multi-node distribution
    let urls = parse_urls(&args.urls);
    let num_nodes = urls.len();
    eprintln!("Using {} RPC endpoint(s):", num_nodes);
    for (i, url) in urls.iter().enumerate() {
        eprintln!("  Node {}: {}", i, url);
    }

    // Parse the funder private key
    let funder: PrivateKeySigner = args.admin_key.parse().expect("admin_key");
    let funder_address = funder.address();
    eprintln!("Funder address: {}", funder_address);

    // Create providers for each URL
    let mut providers = Vec::with_capacity(num_nodes);
    for url in &urls {
        let provider = ProviderBuilder::new().connect(url).await?;
        providers.push(provider);
    }

    // Create HTTP clients for each URL
    let clients: Vec<reqwest::Client> = (0..num_nodes)
        .map(|_| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client")
        })
        .collect();

    // Validate that the node's pool can handle max_txs_per_account
    validate_pool_capacity(
        &providers[0],
        &clients[0],
        &urls[0],
        &funder,
        args.max_txs_per_account,
    )
    .await?;

    // Calculate number of accounts needed
    let num_accounts = args.num_txs.div_ceil(args.max_txs_per_account) as usize;
    eprintln!(
        "Auto-calculated: {} accounts needed for {} txs ({} txs/account max)",
        num_accounts, args.num_txs, args.max_txs_per_account
    );

    // Generate accounts
    eprintln!("Generating {} accounts...", num_accounts);
    let accounts = generate_accounts(num_accounts);
    eprintln!("Generated {} accounts", accounts.len());

    // Parse funding amount
    let funding_amount: U256 = args.funding_amount.parse().expect("Invalid funding_amount");
    eprintln!("Funding each account with {} wei", funding_amount);

    // Fund accounts (use first provider/client)
    fund_accounts(
        &providers[0],
        &clients[0],
        &urls[0],
        &funder,
        &accounts,
        funding_amount,
    )
    .await?;

    // Build ALL transactions upfront, distributed across accounts
    eprintln!(
        "Building {} transactions across {} accounts...",
        args.num_txs, num_accounts
    );
    let raw_transactions = build_transactions_multi_account(
        &providers[0],
        args.num_txs,
        &accounts,
        &args.to_address,
        args.max_txs_per_account,
    )
    .await?;
    eprintln!("All transactions built successfully");

    // Start block monitoring (use first provider)
    let monitoring_active = Arc::new(RwLock::new(true));
    let monitoring_active_clone = monitoring_active.clone();
    let expected_tx_count = args.num_txs;

    let monitor_url = urls[0].clone();
    let monitor_handle = tokio::spawn(async move {
        let monitor_provider = ProviderBuilder::new()
            .connect(&monitor_url)
            .await
            .expect("Failed to connect monitor provider");
        monitor_blocks(
            monitor_provider,
            expected_tx_count,
            args.max_wait_seconds,
            monitoring_active_clone,
        )
        .await;
    });

    let mut consecutive_failures = 0;
    const MAX_CONSECUTIVE_FAILURES: u32 = 10;
    let mut last_progress_report = std::time::Instant::now();
    let mut sent_count = 0usize;
    let mut accepted_count = 0usize;
    let mut rejected_count = 0usize;
    let mut batch_idx = 0usize;

    // Rate limiting: calculate delay between batches
    let batch_delay = if args.target_tps > 0 {
        let txs_per_batch = args.batch_size as f64;
        let seconds_per_batch = txs_per_batch / args.target_tps as f64;
        Duration::from_secs_f64(seconds_per_batch)
    } else {
        Duration::ZERO
    };

    if args.target_tps > 0 {
        eprintln!(
            "Rate limiting: target {} TPS, {}ms between batches",
            args.target_tps,
            batch_delay.as_millis()
        );
    }

    let send_start = std::time::Instant::now();

    // Send transactions in batches, distributing across nodes round-robin
    while sent_count < raw_transactions.len() {
        let batch_start = std::time::Instant::now();

        let batch_end = (sent_count + args.batch_size).min(raw_transactions.len());
        let batch: Vec<Vec<u8>> = raw_transactions[sent_count..batch_end]
            .iter()
            .map(|(_, tx)| tx.clone())
            .collect();

        // Select node for this batch (round-robin)
        let node_idx = batch_idx % num_nodes;
        let url = &urls[node_idx];
        let client = &clients[node_idx];

        let batch_request = create_batch_request(&batch, sent_count);

        // Send the batch
        match client.post(url).json(&batch_request).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    if let Ok(body) = response.text().await {
                        let results: Result<Vec<serde_json::Value>, _> =
                            serde_json::from_str(&body);
                        if let Ok(results) = results {
                            // Count successes and categorize errors
                            let mut batch_accepted = 0usize;
                            let mut batch_pool_full = 0usize;
                            let mut batch_already_known = 0usize;
                            let mut batch_other_errors = 0usize;

                            for result in &results {
                                if let Some(error) = result.get("error") {
                                    let msg =
                                        error.get("message").and_then(|m| m.as_str()).unwrap_or("");
                                    if msg.contains("txpool is full") {
                                        batch_pool_full += 1;
                                    } else if msg.contains("already known")
                                        || msg.contains("nonce too low")
                                    {
                                        batch_already_known += 1;
                                    } else {
                                        batch_other_errors += 1;
                                    }
                                } else {
                                    batch_accepted += 1;
                                }
                            }

                            // If pool is full, wait and retry entire batch
                            if batch_pool_full > 0 {
                                eprintln!(
                                    "Batch {} (node {}): {} pool full, {} accepted, waiting...",
                                    batch_idx, node_idx, batch_pool_full, batch_accepted
                                );
                                rejected_count += batch_pool_full;
                                tokio::time::sleep(Duration::from_secs(2)).await;
                                // Don't advance - retry the batch
                                continue;
                            }

                            // Count accepted (success + already known count as "in pool")
                            accepted_count += batch_accepted + batch_already_known;
                            rejected_count += batch_other_errors;

                            if batch_other_errors > 0 && batch_accepted == 0 {
                                consecutive_failures += 1;
                                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                    eprintln!("Too many failures, aborting.");
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                continue;
                            }

                            consecutive_failures = 0;
                        }
                    }
                    // Batch processed - advance
                    sent_count = batch_end;
                    batch_idx += 1;
                } else {
                    eprintln!(
                        "Batch {} (node {}): HTTP error {}, retrying...",
                        batch_idx,
                        node_idx,
                        response.status()
                    );
                    consecutive_failures += 1;
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        eprintln!("Too many consecutive failures, aborting.");
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            }
            Err(e) => {
                eprintln!(
                    "Batch {} (node {}): request failed: {}, retrying...",
                    batch_idx, node_idx, e
                );
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    eprintln!("Too many consecutive failures, aborting.");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        }

        // Rate limiting: wait if needed
        if !batch_delay.is_zero() {
            let elapsed = batch_start.elapsed();
            if elapsed < batch_delay {
                tokio::time::sleep(batch_delay - elapsed).await;
            }
        }

        // Progress report every 5 seconds
        if last_progress_report.elapsed() > Duration::from_secs(5) {
            let elapsed = send_start.elapsed().as_secs_f64();
            let current_tps = if elapsed > 0.0 {
                accepted_count as f64 / elapsed
            } else {
                0.0
            };
            eprintln!(
                "Progress: {}/{} sent, {} accepted, {} rejected, {:.0} TPS",
                sent_count,
                raw_transactions.len(),
                accepted_count,
                rejected_count,
                current_tps
            );
            last_progress_report = std::time::Instant::now();
        }
    }

    let send_elapsed = send_start.elapsed();
    let send_tps = if send_elapsed.as_secs_f64() > 0.0 {
        accepted_count as f64 / send_elapsed.as_secs_f64()
    } else {
        0.0
    };

    eprintln!(
        "\nSending complete! {} sent, {} accepted, {} rejected in {:.1}s ({:.0} TPS)",
        sent_count,
        accepted_count,
        rejected_count,
        send_elapsed.as_secs_f64(),
        send_tps
    );

    // Signal that sending is complete, but keep monitoring until all transactions are seen
    eprintln!("Waiting for all transactions to appear in blocks...");
    *monitoring_active.write().await = false;

    // Wait for monitor to finish (it will stop when all transactions are counted)
    let _ = monitor_handle.await;

    Ok(())
}
