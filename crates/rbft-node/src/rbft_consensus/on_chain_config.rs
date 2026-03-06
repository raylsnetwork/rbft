// SPDX-License-Identifier: Apache-2.0
use alloy_primitives::{keccak256, Address as AlloyAddress, B256, U256};
use reth_ethereum::storage::{HeaderProvider, StateProviderFactory};
use reth_network_peers::NodeRecord;
use std::net::{IpAddr, ToSocketAddrs};
use tracing::{debug, warn};

/// Configuration read from the on-chain validator contract.
///
/// Reads from the validators array storage starting at keccak256(0) for validators.
/// Reads configuration values from storage slots 1-4.
/// Returns the full validator list and the selected validator subset along with configuration.
/// If block_number is 0, uses the latest state provider.
/// If max_validators is set and the number of validators exceeds it, uses the block_hash
/// as a random seed to deterministically select a subset.
#[derive(Debug, Clone)]
pub struct OnChainConfig {
    pub all_validators: Vec<AlloyAddress>,
    pub selected_validators: Vec<AlloyAddress>,
    pub _max_validators: usize,
    pub _base_fee: u64,
    pub block_interval_ms: u64,
    pub _epoch_length: u64,
    #[allow(dead_code)]
    pub all_validator_enodes: Vec<NodeRecord>,
    pub selected_validator_enodes: Vec<NodeRecord>,
}

fn parse_enode_with_dns(enode_str: &str) -> Result<NodeRecord, String> {
    match enode_str.parse::<NodeRecord>() {
        Ok(record) => Ok(record),
        Err(e) => {
            let initial = format!("{e:?}");
            match resolve_enode_hostname(enode_str) {
                Ok(rewritten) => rewritten
                    .parse::<NodeRecord>()
                    .map_err(|e| format!("{initial}; after dns: {e:?}")),
                Err(resolve_err) => Err(format!("{initial}; dns: {resolve_err}")),
            }
        }
    }
}

fn resolve_enode_hostname(enode_str: &str) -> Result<String, String> {
    let without_scheme = enode_str
        .strip_prefix("enode://")
        .ok_or_else(|| "missing enode:// scheme".to_string())?;
    let (id, host_part) = without_scheme
        .split_once('@')
        .ok_or_else(|| "missing @ separator".to_string())?;
    let (host_port, query) = host_part.split_once('?').unwrap_or((host_part, ""));
    let (host, port_str) = host_port
        .rsplit_once(':')
        .ok_or_else(|| "missing port".to_string())?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port: u16 = port_str.parse().map_err(|e| format!("invalid port: {e}"))?;

    if host.parse::<IpAddr>().is_ok() {
        return Err("host is already an IP address".to_string());
    }

    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("dns lookup failed for {host}: {e}"))?
        .collect();
    let addr = addrs
        .iter()
        .find(|addr| matches!(addr.ip(), IpAddr::V4(_)))
        .or_else(|| addrs.first())
        .ok_or_else(|| "dns lookup returned no addresses".to_string())?;

    let host = match addr.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };

    let mut resolved = format!("enode://{}@{}:{}", id, host, port);
    if !query.is_empty() {
        resolved.push('?');
        resolved.push_str(query);
    }
    Ok(resolved)
}

/// Reads validator configuration from on-chain contract storage.
///
/// This function reads the validator set and configuration parameters from the
/// on-chain validator contract at a specific block number. It handles epoch-based
/// validator selection and deterministic subset selection when there are more
/// validators than the configured maximum.
///
/// This function is infallible - it will return default/empty values and log warnings
/// if any errors occur during reading.
pub fn get_on_chain_config<P>(provider: &P, block_number: u64) -> OnChainConfig
where
    P: StateProviderFactory + HeaderProvider,
{
    /// Maximum number of validators to read from contract storage
    const MAX_VALIDATORS: usize = 256;
    // Proxy address from genesis.rs
    const PROXY_ADDRESS: &str = "0x0000000000000000000000000000000000001001";
    let proxy_address: AlloyAddress = match PROXY_ADDRESS.parse() {
        Ok(addr) => addr,
        Err(e) => {
            warn!(
                target: "rbft",
                "Failed to parse proxy address: {} - using default empty config",
                e
            );
            return OnChainConfig {
                all_validators: Vec::new(),
                selected_validators: Vec::new(),
                _max_validators: 4,
                _base_fee: 4761904761905,
                block_interval_ms: 1000,
                _epoch_length: 32,
                all_validator_enodes: Vec::new(),
                selected_validator_enodes: Vec::new(),
            };
        }
    };

    // First, read epoch_length from the current block_number state
    let current_state = if block_number <= 1 {
        match provider.latest() {
            Ok(state) => state,
            Err(e) => {
                warn!(
                    target: "rbft",
                    "Failed to get latest state provider: {} - using default empty config",
                    e
                );
                return OnChainConfig {
                    all_validators: Vec::new(),
                    selected_validators: Vec::new(),
                    _max_validators: 4,
                    _base_fee: 4761904761905,
                    block_interval_ms: 1000,
                    _epoch_length: 32,
                    all_validator_enodes: Vec::new(),
                    selected_validator_enodes: Vec::new(),
                };
            }
        }
    } else {
        match provider.state_by_block_number_or_tag(block_number.into()) {
            Ok(state) => state,
            Err(e) => {
                warn!(
                    target: "rbft",
                    "Failed to get state provider for block {}: {} - using default empty config",
                    block_number, e
                );
                return OnChainConfig {
                    all_validators: Vec::new(),
                    selected_validators: Vec::new(),
                    _max_validators: 4,
                    _base_fee: 4761904761905,
                    block_interval_ms: 1000,
                    _epoch_length: 32,
                    all_validator_enodes: Vec::new(),
                    selected_validator_enodes: Vec::new(),
                };
            }
        }
    };

    let epoch_length_value = current_state
        .storage(proxy_address, U256::from(4).into())
        .ok()
        .flatten()
        .unwrap_or(U256::from(32)); // Default to 32 if not set
    let epoch_length: u64 = epoch_length_value.try_into().unwrap_or(32);

    // Calculate the first block of this epoch (we've already validated epoch_length > 0)
    let epoch_first_block = (block_number / epoch_length) * epoch_length;

    // Now read other configuration values from the last block of the previous epoch.
    let epoch_config_state = if epoch_first_block <= 1 {
        match provider.latest() {
            Ok(state) => state,
            Err(e) => {
                warn!(
                    target: "rbft",
                    "Failed to get latest state provider for epoch config: {} - using defaults",
                    e
                );
                return OnChainConfig {
                    all_validators: Vec::new(),
                    selected_validators: Vec::new(),
                    _max_validators: 4,
                    _base_fee: 4761904761905,
                    block_interval_ms: 1000,
                    _epoch_length: epoch_length,
                    all_validator_enodes: Vec::new(),
                    selected_validator_enodes: Vec::new(),
                };
            }
        }
    } else {
        match provider.state_by_block_number_or_tag(epoch_first_block.saturating_sub(1).into()) {
            Ok(state) => state,
            Err(e) => {
                warn!(
                    target: "rbft",
                    "Failed to get state provider for epoch block {}: {} - using defaults",
                    epoch_first_block.saturating_sub(1), e
                );
                return OnChainConfig {
                    all_validators: Vec::new(),
                    selected_validators: Vec::new(),
                    _max_validators: 4,
                    _base_fee: 4761904761905,
                    block_interval_ms: 1000,
                    _epoch_length: epoch_length,
                    all_validator_enodes: Vec::new(),
                    selected_validator_enodes: Vec::new(),
                };
            }
        }
    };

    // Read configuration values from contract storage slots at epoch_first_block
    let max_validators_value = epoch_config_state
        .storage(proxy_address, U256::from(1).into())
        .ok()
        .flatten()
        .unwrap_or(U256::from(4)); // Default to 4 if not set
    let max_validators: usize = max_validators_value.try_into().unwrap_or(4);

    let base_fee_value = epoch_config_state
        .storage(proxy_address, U256::from(2).into())
        .ok()
        .flatten()
        .unwrap_or(U256::from(4761904761905u64)); // Default base fee
    let base_fee: u64 = base_fee_value.try_into().unwrap_or(4761904761905u64);

    let block_interval_ms_value = epoch_config_state
        .storage(proxy_address, U256::from(3).into())
        .ok()
        .flatten()
        .unwrap_or(U256::from(1000)); // Default to 1000ms
    let block_interval_ms: u64 = block_interval_ms_value.try_into().unwrap_or(1000);

    debug!(
        target: "rbft",
        "Configuration from contract storage: epoch_length={} (from block {}), \
         max_validators={}, base_fee={}, block_time_ms={} (from epoch block {})",
        epoch_length, block_number, max_validators, base_fee, block_interval_ms, epoch_first_block
    );

    // Get the block hash from the last block of the previous epoch
    // For the first epoch (genesis), use the genesis block hash or a default
    let epoch_hash = if epoch_first_block <= 1 {
        // For genesis epoch, try to get block 0 or 1, or use default hash
        match provider.sealed_header(1) {
            Ok(Some(header)) => header.hash(),
            _ => match provider.sealed_header(0) {
                Ok(Some(header)) => header.hash(),
                _ => {
                    // Use a predictable hash for genesis if no block exists yet
                    alloy_primitives::keccak256(b"genesis_epoch_hash")
                }
            },
        }
    } else {
        match provider.sealed_header(epoch_first_block.saturating_sub(1)) {
            Ok(Some(header)) => header.hash(),
            Ok(None) => {
                warn!(
                    target: "rbft",
                    "Failed to get sealed header for epoch block {} - using default hash",
                    epoch_first_block.saturating_sub(1)
                );
                alloy_primitives::keccak256(b"genesis_epoch_hash")
            }
            Err(e) => {
                warn!(
                    target: "rbft",
                    "Error getting sealed header for epoch block {}: {} - using default hash",
                    epoch_first_block.saturating_sub(1), e
                );
                alloy_primitives::keccak256(b"genesis_epoch_hash")
            }
        }
    };

    debug!(
        target: "rbft",
        "Using epoch-based validator selection: block {}, epoch starts at \
         block {}, reading validators from epoch block {}, epoch_length {}, hash {}",
        block_number,
        epoch_first_block,
        epoch_first_block,
        epoch_length,
        epoch_hash
    );

    // Read slot 0 to get the array length from the epoch_first_block state
    let slot_0_value = epoch_config_state
        .storage(proxy_address, U256::ZERO.into())
        .ok()
        .flatten()
        .unwrap_or(U256::ZERO);

    let num_validators: usize = slot_0_value.try_into().unwrap_or(0);

    // Limit to MAX_VALIDATORS
    let num_validators = num_validators.min(MAX_VALIDATORS);

    debug!(target: "rbft", "block: {} num_validators: {}", block_number, num_validators);

    if num_validators == 0 {
        return OnChainConfig {
            all_validators: Vec::new(),
            selected_validators: Vec::new(),
            _max_validators: max_validators,
            _base_fee: base_fee,
            block_interval_ms,
            _epoch_length: epoch_length,
            all_validator_enodes: Vec::new(),
            selected_validator_enodes: Vec::new(),
        };
    }

    // Calculate the starting storage slot for the array elements: keccak256(0)
    let array_start_slot = keccak256(U256::ZERO.to_be_bytes::<32>());

    let mut validators = Vec::new();

    // Read up to num_validators addresses from sequential storage slots
    for i in 0..num_validators {
        // Calculate the slot for this validator: keccak256(0) + i
        let slot_bytes: [u8; 32] = array_start_slot.into();
        let mut slot = U256::from_be_bytes(slot_bytes);
        slot = slot.saturating_add(U256::from(i));

        // Read the validator address from storage
        if let Some(value) = epoch_config_state
            .storage(proxy_address, slot.into())
            .ok()
            .flatten()
        {
            // Address is the last 20 bytes of the U256 value
            let address_bytes: [u8; 32] = value.to_be_bytes();
            let address = AlloyAddress::from_slice(&address_bytes[12..32]);
            if !address.is_zero() {
                validators.push(address);
            }
        }
    }

    debug!(
        target: "rbft",
        "Successfully read {} validators from contract storage",
        validators.len()
    );

    let all_validators = validators;
    let mut selected_validators = all_validators.clone();
    let mut seed = epoch_hash;

    // If we have more validators than the limit,
    // deterministically select a subset using the epoch block hash as a random seed
    if selected_validators.len() > max_validators {
        debug!(
            target: "rbft",
            "Selecting {} validators from {} using deterministic seed from epoch block",
            max_validators,
            selected_validators.len()
        );

        // Use the block hash as a seed to deterministically select validators
        // Fisher-Yates shuffle with deterministic randomness
        let mut selected_indices = Vec::new();
        let mut available: Vec<usize> = (0..selected_validators.len()).collect();

        for i in 0..max_validators {
            // Use bytes from the seed to generate deterministic "random" indices
            let seed_bytes: [u8; 32] = seed.into();
            let index_seed = u64::from_be_bytes(
                seed_bytes[0..8]
                    .try_into()
                    .expect("slice of length 8 always converts to [u8; 8]"),
            );
            let idx = (index_seed as usize) % available.len();
            selected_indices.push(available.remove(idx));

            // Re-hash the seed for the next iteration to get different randomness
            if i < max_validators - 1 {
                // For next iteration, hash the current seed with the index
                let mut next_seed_data = Vec::new();
                next_seed_data.extend_from_slice(seed.as_slice());
                next_seed_data.extend_from_slice(&(i as u64).to_be_bytes());
                seed = keccak256(&next_seed_data);
            }
        }

        // Sort indices to maintain some ordering consistency
        selected_indices.sort_unstable();

        // Extract the selected validators
        selected_validators = selected_indices
            .into_iter()
            .map(|i| all_validators[i])
            .collect();

        debug!(
            target: "rbft",
            "Selected validators (first 3): {:?}",
            selected_validators.iter().take(3).collect::<Vec<_>>()
        );
    }

    // Read enodes from slot 5 (string[] storage)
    let slot_5_key = B256::from(U256::from(5).to_be_bytes::<32>());
    let slot_5_value = current_state
        .storage(proxy_address, slot_5_key)
        .unwrap_or_default()
        .unwrap_or(U256::ZERO);

    let num_enodes: usize = slot_5_value.try_into().unwrap_or(0);
    let mut validator_enodes = Vec::new();

    if num_enodes > 0 {
        // Calculate starting slot for enodes array: keccak256(5)
        let enodes_array_start = keccak256(U256::from(5).to_be_bytes::<32>());

        for i in 0..num_enodes.min(MAX_VALIDATORS) {
            // Each string in the array is stored at keccak256(enodes_array_start) + i
            let string_slot_bytes = U256::from_be_slice(enodes_array_start.as_slice())
                .wrapping_add(U256::from(i))
                .to_be_bytes::<32>();
            let string_slot = B256::from(string_slot_bytes);

            // Read the string length and data from the slot
            let string_data = current_state
                .storage(proxy_address, string_slot)
                .unwrap_or_default()
                .unwrap_or(U256::ZERO);

            // For strings <= 31 bytes, data is stored inline with length in LSB
            // For longer strings, slot contains (length * 2 + 1) and data is at keccak256(slot)
            let bytes = string_data.to_be_bytes::<32>();
            let last_byte = bytes[31];

            if last_byte & 1 == 0 {
                // Short string: length in last byte (divided by 2), data inline
                let length = (last_byte / 2) as usize;
                if length > 0 && length <= 31 {
                    let enode_str = String::from_utf8_lossy(&bytes[..length]).to_string();
                    match parse_enode_with_dns(&enode_str) {
                        Ok(node_record) => validator_enodes.push(node_record),
                        Err(e) => warn!(
                            target: "rbft",
                            "Failed to parse enode from contract: {} - error: {}",
                            enode_str,
                            e
                        ),
                    }
                }
            } else {
                // Long string: read from keccak256(string_slot)
                let length = string_data.wrapping_sub(U256::from(1)) / U256::from(2);
                let length_usize: usize = length.try_into().unwrap_or(0);

                if length_usize > 0 && length_usize <= 1024 {
                    // Cap at reasonable size
                    let data_start_slot = keccak256(string_slot.as_slice());
                    let num_slots = length_usize.div_ceil(32);

                    let mut enode_bytes = Vec::new();
                    for j in 0..num_slots {
                        let data_slot_bytes = U256::from_be_slice(data_start_slot.as_slice())
                            .wrapping_add(U256::from(j))
                            .to_be_bytes::<32>();
                        let data_slot = B256::from(data_slot_bytes);

                        let chunk = current_state
                            .storage(proxy_address, data_slot)
                            .unwrap_or_default()
                            .unwrap_or(U256::ZERO);
                        enode_bytes.extend_from_slice(&chunk.to_be_bytes::<32>());
                    }

                    enode_bytes.truncate(length_usize);
                    if let Ok(enode_str) = String::from_utf8(enode_bytes) {
                        match parse_enode_with_dns(&enode_str) {
                            Ok(node_record) => validator_enodes.push(node_record),
                            Err(e) => warn!(
                                target: "rbft",
                                "Failed to parse enode from contract: {} - error: {}",
                                enode_str,
                                e
                            ),
                        }
                    }
                }
            }
        }

        debug!(
            target: "rbft",
            "Read enodes from contract storage {validator_enodes:?}"
        );
    }

    // Create selected_validator_enodes by filtering to only enodes of selected validators
    let selected_validator_enodes: Vec<NodeRecord> = selected_validators
        .iter()
        .filter_map(|selected_addr| {
            // Find the index of this selected validator in all_validators
            all_validators
                .iter()
                .position(|addr| addr == selected_addr)
                .and_then(|idx| validator_enodes.get(idx).cloned())
        })
        .collect();

    debug!(
        target: "rbft",
        "Selected {} enodes out of {} total validator enodes",
        selected_validator_enodes.len(),
        validator_enodes.len()
    );

    OnChainConfig {
        all_validators,
        selected_validators,
        _max_validators: max_validators,
        _base_fee: base_fee,
        block_interval_ms,
        _epoch_length: epoch_length,
        all_validator_enodes: validator_enodes,
        selected_validator_enodes,
    }
}
