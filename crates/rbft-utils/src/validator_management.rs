// SPDX-License-Identifier: Apache-2.0
use alloy_primitives::{Address, Bytes};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use eyre::{eyre, Result};
use reth_network_peers::NodeRecord;
use serde_json::json;
use std::net::SocketAddr;

/// Default validator contract address (proxy contract)
const DEFAULT_VALIDATOR_CONTRACT: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x10, 0x01,
]);

/// Generate a fresh validator key-pair + P2P secret key and print as JSON.
///
/// Output fields:
///   validator_address      – Ethereum address derived from the validator key
///   validator_private_key  – hex-encoded validator private key (for --validator-key)
///   p2p_secret_key         – hex-encoded P2P secret key (for --p2p-secret-key)
///   enode                  – enode URL derived from the P2P key
pub fn keygen_validator(ip: &str, port: u16) -> Result<()> {
    // Generate validator key
    let validator_signer = PrivateKeySigner::random();
    let validator_address = validator_signer.address();
    let validator_private_key = format!(
        "0x{}",
        alloy_primitives::hex::encode(validator_signer.to_bytes())
    );

    // Generate P2P secret key
    let p2p_secret_key = reth_ethereum::network::config::rng_secret_key();
    let p2p_key_hex = alloy_primitives::hex::encode(p2p_secret_key.as_ref());

    // Derive enode from P2P key
    let socket_addr: SocketAddr = format!("{ip}:{port}")
        .parse()
        .map_err(|e| eyre!("Invalid IP/port '{}:{}': {}", ip, port, e))?;
    let enode = NodeRecord::from_secret_key(socket_addr, &p2p_secret_key);

    let output = json!({
        "validator_address": format!("{:?}", validator_address),
        "validator_private_key": validator_private_key,
        "p2p_secret_key": p2p_key_hex,
        "enode": enode.to_string(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Add a validator to the QBFTValidatorSet contract
pub async fn add_validator(
    private_key: &str,
    validator_address: Address,
    enode: &str,
    rpc_url: &str,
) -> Result<()> {
    let contract_address = DEFAULT_VALIDATOR_CONTRACT;

    // Validate enode is a valid NodeRecord
    let _node_record: NodeRecord = enode
        .parse()
        .map_err(|e| eyre!("Invalid enode format: {}", e))?;

    // Parse private key
    let private_key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let from_address = signer.address();

    // Create provider with wallet
    let url = rpc_url.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect_http(url);

    // Encode the function call: addValidator(address,string)
    // Function selector: keccak256("addValidator(address,string)")[:4] = 0x63e2a232
    let mut data = vec![0x63, 0xe2, 0xa2, 0x32];

    // Encode parameters (address validator, string enode)
    // Parameter 1: validator address at offset 0x00 (padded to 32 bytes)
    let mut addr_bytes = [0u8; 32];
    addr_bytes[12..].copy_from_slice(validator_address.as_slice());
    data.extend_from_slice(&addr_bytes);

    // Parameter 2: offset to string data (0x40 = 64 bytes from start of parameters)
    let string_offset = [0u8; 31]
        .iter()
        .chain(&[0x40])
        .copied()
        .collect::<Vec<u8>>();
    data.extend_from_slice(&string_offset);

    // String data: length + padded content
    let enode_bytes = enode.as_bytes();
    let enode_len = enode_bytes.len();
    let mut len_bytes = [0u8; 32];
    len_bytes[28..].copy_from_slice(&(enode_len as u32).to_be_bytes());
    data.extend_from_slice(&len_bytes);

    // String content (padded to 32-byte boundary)
    data.extend_from_slice(enode_bytes);
    let padding = (32 - (enode_len % 32)) % 32;
    data.extend_from_slice(&vec![0u8; padding]);

    // Create transaction
    let tx = TransactionRequest::default()
        .to(contract_address)
        .from(from_address)
        .input(Bytes::from(data).into());

    // Send transaction
    let pending_tx = provider.send_transaction(tx).await?;
    let tx_hash = *pending_tx.tx_hash();

    // Wait for confirmation
    let receipt = pending_tx.get_receipt().await?;

    if receipt.status() {
        println!(
            "✓ Added validator {} (tx: {:#x}, block: {})",
            validator_address,
            tx_hash,
            receipt.block_number.unwrap_or_default()
        );
    } else {
        return Err(eyre!(
            "Transaction failed with status: {:?}",
            receipt.status()
        ));
    }

    Ok(())
}

/// Remove a validator from the QBFTValidatorSet contract
pub async fn remove_validator(
    private_key: &str,
    validator_address: Address,
    rpc_url: &str,
) -> Result<()> {
    let contract_address = DEFAULT_VALIDATOR_CONTRACT;
    println!(
        "Removing validator {} from contract at {}",
        validator_address, contract_address
    );
    println!("Using RPC URL: {}", rpc_url);

    // Parse private key
    let private_key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let from_address = signer.address();
    println!("Sending transaction from: {}", from_address);

    // Create provider with wallet
    let url = rpc_url.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect_http(url);

    // Encode the function call: removeValidator(address)
    // Function selector: keccak256("removeValidator(address)")[:4] = 0x40a141ff
    let mut data = vec![0x40, 0xa1, 0x41, 0xff];
    // Encode the validator address (padded to 32 bytes)
    let mut addr_bytes = [0u8; 32];
    addr_bytes[12..].copy_from_slice(validator_address.as_slice());
    data.extend_from_slice(&addr_bytes);

    // Create transaction
    let tx = TransactionRequest::default()
        .to(contract_address)
        .from(from_address)
        .input(Bytes::from(data).into());

    // Send transaction
    println!("Sending removeValidator transaction...");
    let pending_tx = provider.send_transaction(tx).await?;
    println!("Transaction sent successfully!");

    // Try to watch for receipt, but don't fail if it errors
    match pending_tx.watch().await {
        Ok(receipt) => {
            println!("Transaction hash: {:?}", receipt);
            println!("Receipt: {:?}", receipt);
            println!("✓ Transaction confirmed!");
        }
        Err(e) => {
            println!("⚠️  Could not watch transaction: {}", e);
            println!("Transaction was sent but confirmation status unknown");
        }
    }

    Ok(())
}

/// Set the maximum active validators value
pub async fn set_max_active_validators(private_key: &str, value: u64, rpc_url: &str) -> Result<()> {
    let contract_address = DEFAULT_VALIDATOR_CONTRACT;
    println!(
        "Setting maximum active validators to {} in contract at {}",
        value, contract_address
    );

    // Parse private key
    let private_key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let from_address = signer.address();

    // Create provider
    let url = rpc_url.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect_http(url);

    // Encode the function call: setMaxActiveValidators(uint256)
    // Function selector: keccak256("setMaxActiveValidators(uint256)")[:4] = 0x1741c372
    let mut data = vec![0x17, 0x41, 0xc3, 0x72];
    // Encode the value (padded to 32 bytes)
    let mut value_bytes = [0u8; 32];
    value_bytes[24..32].copy_from_slice(&value.to_be_bytes());
    data.extend_from_slice(&value_bytes);

    // Create transaction
    let tx = TransactionRequest::default()
        .to(contract_address)
        .from(from_address)
        .input(Bytes::from(data).into());

    // Send transaction
    println!("Sending setMaxActiveValidators transaction...");
    let tx_hash = provider.send_transaction(tx).await?.watch().await?;
    println!("Transaction hash: {:?}", tx_hash);

    println!("✓ Maximum active validators set successfully!");
    Ok(())
}

/// Set the base fee value
pub async fn set_base_fee(private_key: &str, value: u64, rpc_url: &str) -> Result<()> {
    let contract_address = DEFAULT_VALIDATOR_CONTRACT;
    println!(
        "Setting base fee to {} in contract at {}",
        value, contract_address
    );

    // Parse private key
    let private_key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let from_address = signer.address();

    // Create provider
    let url = rpc_url.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect_http(url);

    // Encode the function call: setBaseFee(uint256)
    // Function selector: keccak256("setBaseFee(uint256)")[:4] = 0x46860698
    let mut data = vec![0x46, 0x86, 0x06, 0x98];
    // Encode the value (padded to 32 bytes)
    let mut value_bytes = [0u8; 32];
    value_bytes[24..32].copy_from_slice(&value.to_be_bytes());
    data.extend_from_slice(&value_bytes);

    // Create transaction
    let tx = TransactionRequest::default()
        .to(contract_address)
        .from(from_address)
        .input(Bytes::from(data).into());

    // Send transaction
    println!("Sending setBaseFee transaction...");
    let tx_hash = provider.send_transaction(tx).await?.watch().await?;
    println!("Transaction hash: {:?}", tx_hash);

    println!("✓ Base fee set successfully!");
    Ok(())
}

/// Set the block interval in milliseconds
pub async fn set_block_interval_ms(private_key: &str, value: u64, rpc_url: &str) -> Result<()> {
    let contract_address = DEFAULT_VALIDATOR_CONTRACT;
    println!(
        "Setting block interval to {} ms in contract at {}",
        value, contract_address
    );

    // Parse private key
    let private_key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let from_address = signer.address();

    // Create provider
    let url = rpc_url.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect_http(url);

    // Encode the function call: setBlockIntervalMs(uint256)
    // Function selector: keccak256("setBlockIntervalMs(uint256)")[:4] = 0x051c36c0
    let mut data = vec![0x05, 0x1c, 0x36, 0xc0];
    // Encode the value (padded to 32 bytes)
    let mut value_bytes = [0u8; 32];
    value_bytes[24..32].copy_from_slice(&value.to_be_bytes());
    data.extend_from_slice(&value_bytes);

    // Create transaction
    let tx = TransactionRequest::default()
        .to(contract_address)
        .from(from_address)
        .input(Bytes::from(data).into());

    // Send transaction
    println!("Sending setBlockIntervalMs transaction...");
    let tx_hash = provider.send_transaction(tx).await?.watch().await?;
    println!("Transaction hash: {:?}", tx_hash);

    println!("✓ Block interval set successfully!");
    Ok(())
}

/// Set the epoch length value
pub async fn set_epoch_length(private_key: &str, value: u64, rpc_url: &str) -> Result<()> {
    let contract_address = DEFAULT_VALIDATOR_CONTRACT;
    println!(
        "Setting epoch length to {} blocks in contract at {}",
        value, contract_address
    );

    // Parse private key
    let private_key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

    let from_address = signer.address();

    // Create provider
    let url = rpc_url.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect_http(url);

    // Encode the function call: setEpochLength(uint256)
    // Function selector: keccak256("setEpochLength(uint256)")[:4] = 0x54eea796
    let mut data = vec![0x54, 0xee, 0xa7, 0x96];
    // Encode the value (padded to 32 bytes)
    let mut value_bytes = [0u8; 32];
    value_bytes[24..32].copy_from_slice(&value.to_be_bytes());
    data.extend_from_slice(&value_bytes);

    // Create transaction
    let tx = TransactionRequest::default()
        .to(contract_address)
        .from(from_address)
        .input(Bytes::from(data).into());

    // Send transaction
    println!("Sending setEpochLength transaction...");
    let tx_hash = provider.send_transaction(tx).await?.watch().await?;
    println!("Transaction hash: {:?}", tx_hash);

    println!("✓ Epoch length set successfully!");
    Ok(())
}

/// Parse an ABI-encoded address array from bytes
fn parse_address_array(data: &[u8]) -> Result<Vec<Address>> {
    if data.len() < 64 {
        return Err(eyre!("Response too short"));
    }

    // Skip the first 32 bytes (offset) and read the length
    let length = u64::from_be_bytes([
        data[56], data[57], data[58], data[59], data[60], data[61], data[62], data[63],
    ]);

    let mut addresses = Vec::new();
    let start_offset = 64;

    for i in 0..length {
        let addr_start = start_offset + (i as usize * 32) + 12; // Skip 12 bytes of padding
        if addr_start + 20 > data.len() {
            break;
        }

        let mut addr_bytes = [0u8; 20];
        addr_bytes.copy_from_slice(&data[addr_start..addr_start + 20]);
        addresses.push(Address::from(addr_bytes));
    }

    Ok(addresses)
}

/// Get comprehensive validator set status (combines all getter functions)
pub async fn get_validator_status(rpc_url: &str) -> Result<()> {
    let contract_address = DEFAULT_VALIDATOR_CONTRACT;

    // Create provider
    let url = rpc_url.parse()?;
    let provider = ProviderBuilder::new().connect_http(url);

    // Get all values in parallel for better performance
    let (
        max_active_result,
        base_fee_result,
        block_interval_result,
        epoch_length_result,
        validators_result,
    ) = tokio::join!(
        get_max_active_validators_internal(&provider, &contract_address),
        get_base_fee_internal(&provider, &contract_address),
        get_block_interval_ms_internal(&provider, &contract_address),
        get_epoch_length_internal(&provider, &contract_address),
        get_validators_internal(&provider, &contract_address)
    );

    let max_active_validators = max_active_result.unwrap_or(0);
    let base_fee = base_fee_result.unwrap_or(0);
    let block_interval_ms = block_interval_result.unwrap_or(0);
    let epoch_length = epoch_length_result.unwrap_or(0);
    let validators = validators_result.unwrap_or_default();

    // Output combined JSON result
    let json_result = json!({
        "maxActiveValidators": max_active_validators,
        "baseFee": base_fee,
        "blockIntervalMs": block_interval_ms,
        "epochLength": epoch_length,
        "validators": validators,
        "count": validators.len()
    });
    println!("{}", json_result);

    Ok(())
}

// Internal helper functions for parallel execution
async fn get_max_active_validators_internal(
    provider: &impl Provider,
    contract_address: &Address,
) -> Result<u64> {
    let data = vec![0x18, 0x45, 0xef, 0xf0];
    let tx = TransactionRequest::default()
        .to(*contract_address)
        .input(Bytes::from(data).into());

    let result = provider.call(tx).await?;
    if result.len() != 32 {
        return Err(eyre!("Invalid response length: {}", result.len()));
    }

    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&result[24..32]);
    Ok(u64::from_be_bytes(bytes))
}

async fn get_base_fee_internal(
    provider: &impl Provider,
    contract_address: &Address,
) -> Result<u64> {
    let data = vec![0x15, 0xe8, 0x12, 0xad];
    let tx = TransactionRequest::default()
        .to(*contract_address)
        .input(Bytes::from(data).into());

    let result = provider.call(tx).await?;
    if result.len() != 32 {
        return Err(eyre!("Invalid response length: {}", result.len()));
    }

    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&result[24..32]);
    Ok(u64::from_be_bytes(bytes))
}

async fn get_block_interval_ms_internal(
    provider: &impl Provider,
    contract_address: &Address,
) -> Result<u64> {
    // Function selector for getBlockIntervalMs()
    let data = vec![0x8c, 0x52, 0xdc, 0x81];
    let tx = TransactionRequest::default()
        .to(*contract_address)
        .input(Bytes::from(data).into());

    let result = provider.call(tx).await?;
    if result.len() != 32 {
        return Err(eyre!("Invalid response length: {}", result.len()));
    }

    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&result[24..32]);
    Ok(u64::from_be_bytes(bytes))
}

async fn get_epoch_length_internal(
    provider: &impl Provider,
    contract_address: &Address,
) -> Result<u64> {
    // Function selector for getEpochLength()
    let data = vec![0xc3, 0x1f, 0x3e, 0x82];
    let tx = TransactionRequest::default()
        .to(*contract_address)
        .input(Bytes::from(data).into());

    let result = provider.call(tx).await?;
    if result.len() != 32 {
        return Err(eyre!("Invalid response length: {}", result.len()));
    }

    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&result[24..32]);
    Ok(u64::from_be_bytes(bytes))
}

async fn get_validators_internal(
    provider: &impl Provider,
    contract_address: &Address,
) -> Result<Vec<Address>> {
    let data = vec![0xb7, 0xab, 0x4d, 0xb5];
    let tx = TransactionRequest::default()
        .to(*contract_address)
        .input(Bytes::from(data).into());

    let result = provider.call(tx).await?;
    parse_address_array(&result)
}
