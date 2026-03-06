// SPDX-License-Identifier: Apache-2.0
use crate::types::{RbftConfig, ValidatorInfo};
use alloy_genesis::{ChainConfig, Genesis, GenesisAccount};
use alloy_primitives::{hex, keccak256, Address, B256, U256};
use alloy_signer_local::PrivateKeySigner;
use eyre::{eyre, Result as EyreResult};
use foundry_compilers::{Project, ProjectPathsConfig};
use rbft_utils::constants::DEFAULT_ADMIN_KEY;
use reth_network_peers::NodeRecord;
use revm::{
    context::{Context as EvmContext, TxEnv},
    database::{CacheDB, EmptyDB},
    primitives::{
        hardfork::SpecId, hash_map::HashMap as RevmHashMap, Address as RevmAddress,
        Bytes as RevmBytes, TxKind, U256 as RevmU256,
    },
    state::{AccountInfo, Bytecode},
    ExecuteEvm, MainBuilder, MainContext,
};
use soldeer_core::{
    config::{Dependency, HttpDependency},
    install::{ensure_dependencies_dir, install_dependency, InstallProgress},
};
use std::collections::BTreeMap;
use std::fs;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

#[derive(Debug, serde::Deserialize)]
struct FoundryConfig {
    profile: ProfileConfig,
    #[serde(default)]
    dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum DependencySpec {
    Version(String),
    Detailed {
        version: String,
        #[serde(default)]
        #[allow(dead_code)]
        url: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        git: Option<String>,
    },
}

#[derive(Debug, serde::Deserialize)]
struct ProfileConfig {
    default: DefaultProfile,
}

#[derive(Debug, serde::Deserialize)]
struct DefaultProfile {
    src: String,
    test: String,
    libs: Vec<String>,
    out: String,
    #[serde(default)]
    #[allow(dead_code)]
    cache_path: String,
    #[serde(default)]
    remappings: Vec<String>,
}

#[derive(Debug)]
struct NodeData {
    validator_address: Address,
    validator_private_key: String,
    p2p_secret_key: String,
    enode: String,
}

/// ERC1967 implementation slot: keccak256("eip1967.proxy.implementation") - 1
const ERC1967_IMPLEMENTATION_SLOT: &str =
    "360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc";

/// Predefined addresses for genesis contracts
const IMPLEMENTATION_ADDRESS: &str = "0x0000000000000000000000000000000000001000";

/// Ensure Soldeer dependencies are installed before compilation
async fn ensure_soldeer_dependencies(
    workspace_root: &Path,
    dependencies: &BTreeMap<String, DependencySpec>,
) -> Result<(), Box<dyn std::error::Error>> {
    let dependencies_dir = workspace_root.join("target").join("forge_dependencies");

    // Create dependencies directory if it doesn't exist
    ensure_dependencies_dir(&dependencies_dir)?;

    eprintln!("Checking Soldeer dependencies...");

    // Create progress monitor (we ignore the monitoring channels for simplicity)
    let (progress, _monitor) = InstallProgress::new();

    for (name, spec) in dependencies {
        let version = match spec {
            DependencySpec::Version(v) => v.clone(),
            DependencySpec::Detailed { version, .. } => version.clone(),
        };

        // Check if dependency already exists
        let dep_path = dependencies_dir.join(format!("{}-{}", name, version));
        if dep_path.exists() {
            eprintln!("  ✓ {} already installed", name);
            continue;
        }

        eprintln!("  → Installing {}@{}...", name, version);

        // Create dependency
        let dep: Dependency = HttpDependency::builder()
            .name(name)
            .version_req(version)
            .build()
            .into();

        // Install dependency
        install_dependency(&dep, None, &dependencies_dir, None, false, progress.clone())
            .await
            .map_err(|e| format!("Failed to install {}: {}", name, e))?;

        eprintln!("  ✓ {} installed successfully", name);
    }

    eprintln!("All Soldeer dependencies ready");
    Ok(())
}

/// Compile all Foundry contracts using foundry_compilers.
/// Returns (workspace_root, artifacts_directory)
fn compile_foundry_project() -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    // Get the workspace root (project directory with foundry.toml)
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .ok_or("Failed to get workspace root directory")?;

    let foundry_toml_path = workspace_root.join("foundry.toml");
    eprintln!("Loading Foundry project from: {:?}", foundry_toml_path);

    if !foundry_toml_path.exists() {
        return Err(format!("foundry.toml not found at: {:?}", foundry_toml_path).into());
    }

    // Parse foundry.toml
    let foundry_toml_content = fs::read_to_string(&foundry_toml_path)
        .map_err(|e| format!("Failed to read foundry.toml: {}", e))?;

    let foundry_config: FoundryConfig = toml::from_str(&foundry_toml_content)
        .map_err(|e| format!("Failed to parse foundry.toml: {}", e))?;

    // Ensure soldeer dependencies are installed (blocking call)
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(ensure_soldeer_dependencies(
        workspace_root,
        &foundry_config.dependencies,
    ))?;

    let profile = &foundry_config.profile.default;

    // Build ProjectPathsConfig from foundry.toml
    let mut paths = ProjectPathsConfig::builder()
        .root(workspace_root)
        .sources(workspace_root.join(&profile.src))
        .tests(workspace_root.join(&profile.test))
        .artifacts(workspace_root.join(&profile.out))
        .build()
        .map_err(|e| format!("Failed to build ProjectPathsConfig: {}", e))?;

    // Add library paths (only if not already in the default "lib" path)
    for lib in &profile.libs {
        let lib_path = workspace_root.join(lib);
        if !paths.libraries.contains(&lib_path) {
            paths.libraries.push(lib_path);
        }
    }

    // Also add forge_dependencies directory for soldeer packages
    let dependencies_path = workspace_root.join("target").join("forge_dependencies");
    if dependencies_path.exists() && !paths.libraries.contains(&dependencies_path) {
        paths.libraries.push(dependencies_path);
    }

    // Add remappings
    for remapping in &profile.remappings {
        paths.remappings.push(
            remapping
                .parse()
                .map_err(|e| format!("Failed to parse remapping '{}': {:?}", remapping, e))?,
        );
    }

    eprintln!("Project paths configured:");
    eprintln!("  Root: {:?}", paths.root);
    eprintln!("  Sources: {:?}", paths.sources);
    eprintln!("  Artifacts: {:?}", paths.artifacts);
    eprintln!("  Cache: {:?}", paths.cache);
    eprintln!("  Libraries: {:?}", paths.libraries);
    eprintln!("  Remappings: {:?}", paths.remappings);

    // Store artifacts directory before building project
    let artifacts_dir = paths.artifacts.clone();

    // Build the project with default solc settings
    let project = Project::builder()
        .paths(paths)
        .build(Default::default())
        .map_err(|e| format!("Failed to build project: {}", e))?;

    eprintln!("Compiling Foundry project...");

    let output = project
        .compile()
        .map_err(|e| format!("Failed to compile project: {}", e))?;

    if output.has_compiler_errors() {
        return Err(format!(
            "Compilation failed with errors: {:?}",
            output.output().errors
        )
        .into());
    }

    eprintln!("Compilation completed successfully");

    Ok((workspace_root.to_path_buf(), artifacts_dir))
}

fn find_contract_artifact(artifacts_dir: &Path, contract_name: &str) -> Option<PathBuf> {
    let mut stack = vec![artifacts_dir.to_path_buf()];
    let filename = format!("{}.json", contract_name);

    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) == Some(filename.as_str()) {
                return Some(path);
            }
        }
    }

    None
}

/// Read runtime bytecode from a compiled contract artifact.
fn read_contract_bytecode(
    artifacts_dir: &Path,
    contract_name: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Read the compiled artifact from the configured artifacts directory
    let default_path = artifacts_dir
        .join(format!("{}.sol", contract_name))
        .join(format!("{}.json", contract_name));

    let artifact_path = if default_path.exists() {
        default_path
    } else {
        find_contract_artifact(artifacts_dir, contract_name)
            .ok_or_else(|| format!("Compiled artifact not found under: {:?}", artifacts_dir))?
    };

    eprintln!("Reading artifact from: {:?}", artifact_path);

    let artifact_content = std::fs::read_to_string(&artifact_path)
        .map_err(|e| format!("Failed to read artifact: {}", e))?;

    let artifact: serde_json::Value = serde_json::from_str(&artifact_content)
        .map_err(|e| format!("Failed to parse artifact JSON: {}", e))?;

    // Extract deployed bytecode
    let bytecode_hex = artifact
        .get("deployedBytecode")
        .and_then(|db| db.get("object"))
        .and_then(|obj| obj.as_str())
        .ok_or("deployedBytecode.object not found in artifact")?;

    // Remove 0x prefix if present
    let bytecode_hex = bytecode_hex.strip_prefix("0x").unwrap_or(bytecode_hex);

    // Convert hex to bytes
    let bytecode =
        hex::decode(bytecode_hex).map_err(|e| format!("Failed to decode bytecode hex: {}", e))?;

    eprintln!(
        "Successfully loaded {} bytecode, length: {} bytes",
        contract_name,
        bytecode.len()
    );

    Ok(bytecode)
}

fn address_to_b256(address: Address) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[12..].copy_from_slice(address.as_slice());
    B256::from(bytes)
}

fn alloy_u256_to_revm(value: U256) -> RevmU256 {
    RevmU256::from_be_slice(&value.to_be_bytes::<32>())
}

/// Encode the initialize() function call
fn encode_initialize(
    validators: &[ValidatorInfo],
    admin: Address,
    max_active_validators: u64,
    base_fee: u64,
    block_interval_ms: u64,
    epoch_length: u64,
    enodes: &[String],
) -> Vec<u8> {
    // Function selector:
    // keccak256("initialize(address[],address,uint256,uint256,uint256,uint256,string[])")[:4]
    let selector =
        &keccak256(b"initialize(address[],address,uint256,uint256,uint256,uint256,string[])")[..4];

    let mut data = Vec::new();
    data.extend_from_slice(selector);

    // Encode parameters (ABI encoding)
    // Offset to first dynamic array (validators) - 7 params before enodes array
    // = 7 * 32 = 224 bytes = 0xE0
    data.extend_from_slice(&U256::from(224u64).to_be_bytes::<32>());

    // Admin address (padded to 32 bytes)
    data.extend_from_slice(&address_to_b256(admin).0);

    // Max active validators (uint256)
    data.extend_from_slice(&U256::from(max_active_validators).to_be_bytes::<32>());

    // Base fee (uint256)
    data.extend_from_slice(&U256::from(base_fee).to_be_bytes::<32>());

    // Block interval ms (uint256)
    data.extend_from_slice(&U256::from(block_interval_ms).to_be_bytes::<32>());

    // Epoch length (uint256)
    data.extend_from_slice(&U256::from(epoch_length).to_be_bytes::<32>());

    // Calculate offset to enodes array (comes after validators array)
    // validators array takes: 32 (length) + 32 * num_validators
    let validators_size = 32 + (32 * validators.len());
    data.extend_from_slice(&U256::from(224u64 + validators_size as u64).to_be_bytes::<32>());

    // Validators array length
    data.extend_from_slice(&U256::from(validators.len() as u64).to_be_bytes::<32>());

    // Validators array elements (each address padded to 32 bytes)
    for validator in validators {
        data.extend_from_slice(&address_to_b256(validator.address).0);
    }

    // Enodes array length
    data.extend_from_slice(&U256::from(enodes.len() as u64).to_be_bytes::<32>());

    // Enodes array elements (dynamic strings)
    // First, write all the offsets
    let mut current_offset = 32 * enodes.len() as u64; // Start after all offset slots
    for enode in enodes {
        data.extend_from_slice(&U256::from(current_offset).to_be_bytes::<32>());
        // Each string takes: 32 (length) + padded bytes
        let string_bytes = enode.as_bytes();
        let padded_len = string_bytes.len().div_ceil(32) * 32;
        current_offset += 32 + padded_len as u64;
    }

    // Then write all the string data
    for enode in enodes {
        let string_bytes = enode.as_bytes();
        // String length
        data.extend_from_slice(&U256::from(string_bytes.len() as u64).to_be_bytes::<32>());
        // String data (padded to 32-byte boundary)
        data.extend_from_slice(string_bytes);
        let padded_len = string_bytes.len().div_ceil(32) * 32;
        let padding = padded_len - string_bytes.len();
        data.extend_from_slice(&vec![0u8; padding]);
    }

    data
}

#[derive(Clone, Copy, Debug)]
struct ValidatorSetConfig {
    max_active_validators: u64,
    base_fee: u64,
    block_interval_ms: u64,
    epoch_length: u64,
}

/// Initialize the validator contract storage by simulating the initialize() call
/// through the proxy, which delegates to the implementation.
#[allow(clippy::too_many_arguments)]
fn initialize_proxy_storage(
    implementation_bytecode: &[u8],
    proxy_bytecode: &[u8],
    implementation_address: Address,
    proxy_address: Address,
    admin_address: Address,
    validators: &[ValidatorInfo],
    config: ValidatorSetConfig,
    enodes: &[String],
) -> EyreResult<BTreeMap<B256, B256>> {
    if validators.is_empty() {
        return Err(eyre!("validator list cannot be empty"));
    }

    let mut db = CacheDB::new(EmptyDB::default());

    let revm_admin = RevmAddress::from_slice(admin_address.as_slice());
    let revm_impl = RevmAddress::from_slice(implementation_address.as_slice());
    let revm_proxy = RevmAddress::from_slice(proxy_address.as_slice());

    // Set up admin account with balance
    let admin_info = AccountInfo {
        balance: alloy_u256_to_revm(U256::MAX),
        ..AccountInfo::default()
    };
    db.insert_account_info(revm_admin, admin_info);

    // Set up implementation contract (code only, no storage)
    let impl_info = AccountInfo {
        code: Some(Bytecode::new_raw(RevmBytes::from(
            implementation_bytecode.to_vec(),
        ))),
        ..AccountInfo::default()
    };
    db.insert_account_info(revm_impl, impl_info);

    // Set up proxy contract with code and ERC1967 implementation slot
    let proxy_info = AccountInfo {
        code: Some(Bytecode::new_raw(RevmBytes::from(proxy_bytecode.to_vec()))),
        ..AccountInfo::default()
    };
    db.insert_account_info(revm_proxy, proxy_info);

    // Set the ERC1967 implementation slot in proxy storage
    let impl_slot = RevmU256::from_be_slice(
        &hex::decode(ERC1967_IMPLEMENTATION_SLOT)
            .expect("ERC1967_IMPLEMENTATION_SLOT is not valid hex"),
    );
    let impl_value = RevmU256::from_be_slice(&address_to_b256(implementation_address).0);

    let mut proxy_storage = RevmHashMap::default();
    proxy_storage.insert(impl_slot, impl_value);
    db.replace_account_storage(revm_proxy, proxy_storage)
        .expect("failed to set proxy implementation slot");

    // Build EVM context and execute initialize() call through the proxy
    let ctx = EvmContext::mainnet()
        .modify_cfg_chained(|cfg| cfg.spec = SpecId::CANCUN)
        .with_db(db);
    let mut evm = ctx.build_mainnet();

    let init_data = encode_initialize(
        validators,
        admin_address,
        config.max_active_validators,
        config.base_fee,
        config.block_interval_ms,
        config.epoch_length,
        enodes,
    );

    let tx = TxEnv::builder()
        .caller(revm_admin)
        .gas_limit(10_000_000)
        .kind(TxKind::Call(revm_proxy)) // Call the proxy, which delegates to impl
        .data(RevmBytes::from(init_data))
        .value(RevmU256::ZERO)
        .build()
        .map_err(|err| eyre!("failed to build initialization tx: {err:?}"))?;

    let exec = evm
        .transact(tx)
        .map_err(|err| eyre!("failed to execute initialization: {err:?}"))?;

    if !exec.result.is_success() {
        return Err(eyre!(
            "validator contract initialization transaction reverted"
        ));
    }

    // Extract storage from the proxy (where delegatecall writes)
    let proxy_state = exec
        .state
        .get(&revm_proxy)
        .ok_or_else(|| eyre!("missing proxy state after initialization"))?;

    let mut storage = BTreeMap::new();

    // Always include the ERC1967 implementation slot
    storage.insert(
        B256::from_slice(
            &hex::decode(ERC1967_IMPLEMENTATION_SLOT)
                .expect("ERC1967_IMPLEMENTATION_SLOT is not valid hex"),
        ),
        address_to_b256(implementation_address),
    );

    // Add all storage slots written during initialization
    for (slot, value) in &proxy_state.storage {
        let key = B256::from_slice(&slot.to_be_bytes::<32>());
        let val = B256::from_slice(&value.present_value.to_be_bytes::<32>());
        storage.insert(key, val);
    }

    println!(
        "Initialized proxy storage with {} slots (including {} validators)",
        storage.len(),
        validators.len()
    );
    println!("Configuration values stored in contract slots 1-4:");
    println!(
        "  Slot 1 (maxActiveValidators): {}",
        config.max_active_validators
    );
    println!("  Slot 2 (baseFee): base_fee: {}", config.base_fee);
    println!("  Slot 3 (blockIntervalMs): {}", config.block_interval_ms);
    println!("  Slot 4 (epochLength): {}", config.epoch_length);

    Ok(storage)
}

/// Read node data from CSV file
fn read_nodes_csv(csv_path: &Path) -> EyreResult<Vec<NodeData>> {
    use std::io::{BufRead, BufReader};

    let file = fs::File::open(csv_path)?;
    let reader = BufReader::new(file);
    let mut nodes = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;

        // Skip header line
        if line_num == 0 {
            continue;
        }

        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() != 5 {
            return Err(eyre!(
                "Invalid CSV format at line {}: expected 5 fields, got {}",
                line_num + 1,
                fields.len()
            ));
        }

        let validator_address: Address = fields[1].parse()?;
        let validator_private_key = fields[2].to_string();
        let p2p_secret_key = fields[3].to_string();
        let enode = fields[4].to_string();

        nodes.push(NodeData {
            validator_address,
            validator_private_key,
            p2p_secret_key,
            enode,
        });
    }

    Ok(nodes)
}

/// Create RBFT genesis configuration with configurable number of validators
#[allow(clippy::too_many_arguments)]
pub fn create_rbft_genesis(
    assets_dir: Option<PathBuf>,
    nodes_csv: Option<PathBuf>,
    initial_nodes: Option<usize>,
    validator_contract_address: Option<Address>,
    gas_limit: Option<u64>,
    block_interval: Option<f64>,
    epoch_length: Option<u64>,
    base_fee: Option<u64>,
    max_active_validators: Option<u64>,
    admin_key: Option<String>,
    _docker: bool, /* Reserved for Docker Compose with bridge networking (not used with
                    * --network host) */
    kube: bool,
    use_trusted_peers: bool,
) {
    let write_enodes = use_trusted_peers;

    let assets_dir = assets_dir.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".rbft/testnet/assets")
    });
    fs::create_dir_all(&assets_dir).expect("failed to create assets directory");

    // Determine CSV path: use provided path or default to <assets_dir>/nodes.csv
    let csv_path = nodes_csv.unwrap_or_else(|| assets_dir.join("nodes.csv"));

    // Always read from CSV - fail if it doesn't exist
    if !csv_path.exists() {
        panic!(
            "Node CSV file not found: {:?}\nPlease run 'node-gen' first to generate the nodes.csv \
             file.",
            csv_path
        );
    }

    println!("Reading node data from CSV: {:?}", csv_path);
    let nodes_from_csv = read_nodes_csv(&csv_path).expect("Failed to read nodes CSV");

    // Derive num_nodes from CSV file
    let num_nodes = nodes_from_csv.len() as u32;

    if num_nodes < 4 {
        panic!(
            "RBFT consensus requires at least 4 validators, but CSV contains only {} nodes",
            num_nodes
        );
    }

    // Determine number of initial validators for genesis
    let num_initial_validators = initial_nodes.unwrap_or(num_nodes as usize);

    if num_initial_validators > num_nodes as usize {
        panic!(
            "initial_nodes ({}) cannot exceed total nodes in CSV ({})",
            num_initial_validators, num_nodes
        );
    }

    if num_initial_validators < 4 {
        panic!(
            "RBFT consensus requires at least 4 initial validators, but initial_nodes is {}",
            num_initial_validators
        );
    }

    println!(
        "Generating RBFT genesis assets for {} node(s) in {:?}",
        num_nodes, assets_dir
    );

    if num_initial_validators < num_nodes as usize {
        println!(
            "Note: Only {} of {} nodes will be initial validators in genesis contract",
            num_initial_validators, num_nodes
        );
        println!(
            "The remaining {} node(s) can be added as validators later",
            num_nodes as usize - num_initial_validators
        );
    }

    // Only compile contracts if validator_contract_address is provided
    let (implementation_bytecode, proxy_bytecode, implementation_address, proxy_address) =
        if let Some(proxy_addr) = validator_contract_address {
            // Compile Foundry project once
            let (_workspace_root, artifacts_dir) =
                compile_foundry_project().expect("Failed to compile Foundry project");

            // Get contract bytecodes from compiled artifacts
            let implementation_bytecode =
                read_contract_bytecode(&artifacts_dir, "QBFTValidatorSet")
                    .expect("Failed to read validator set contract bytecode");
            let proxy_bytecode = read_contract_bytecode(&artifacts_dir, "ERC1967Proxy")
                .expect("Failed to read ERC1967Proxy bytecode");

            // Parse addresses
            let implementation_address: Address = IMPLEMENTATION_ADDRESS
                .parse()
                .expect("IMPLEMENTATION_ADDRESS is not a valid address");

            (
                Some(implementation_bytecode),
                Some(proxy_bytecode),
                Some(implementation_address),
                Some(proxy_addr),
            )
        } else {
            println!("Skipping validator contract deployment (no address provided)");
            (None, None, None, None)
        };

    let mut enodes = Vec::new();
    let mut validators = Vec::new();

    let kube_namespace = if kube {
        crate::kube_namespace::default_kube_namespace()
    } else {
        String::new()
    };
    if kube {
        println!("Using kube namespace: {}", kube_namespace);
    }

    let resolve_enodes = kube
        && match std::env::var("RBFT_KUBE_RESOLVE_ENODES") {
            Ok(value) => value != "0",
            Err(_) => false,
        };

    for i in 0..num_nodes {
        // Use data from CSV
        let node_data = &nodes_from_csv[i as usize];
        println!(
            "Using node {} from CSV: address={}, enode={}",
            i, node_data.validator_address, node_data.enode
        );

        let mut enode_str = node_data.enode.clone();
        if resolve_enodes {
            match resolve_enode_hostname(&enode_str) {
                Ok(resolved) => {
                    if resolved != enode_str {
                        println!("Resolved enode host to {}", resolved);
                    }
                    enode_str = resolved;
                }
                Err(err) => {
                    eprintln!(
                        "Warning: failed to resolve enode host for {}: {}",
                        enode_str, err
                    );
                }
            }
        }

        let (ethereum_address, ethereum_private_key, p2p_secret_key_hex, enode_str) = (
            node_data.validator_address,
            node_data.validator_private_key.clone(),
            node_data.p2p_secret_key.clone(),
            enode_str,
        );

        // Create validator info
        let validator = ValidatorInfo {
            address: ethereum_address,
        };

        // Only add to initial validators if within initial_nodes limit
        if (i as usize) < num_initial_validators {
            validators.push(validator);
            enodes.push(enode_str.clone());
        }

        // Write P2P secret key to file (for all nodes)
        let p2p_filename = assets_dir.join(format!("p2p-secret-key{}.txt", i));
        std::fs::write(&p2p_filename, &p2p_secret_key_hex)
            .expect("failed to write P2P secret key file");

        // Write validator address to id file (for all nodes)
        let id_filename = assets_dir.join(format!("id{}.txt", i));
        std::fs::write(&id_filename, ethereum_address.to_string())
            .expect("failed to write validator id file");

        // Write validator private key to separate file (for all nodes)
        let validator_key_filename = assets_dir.join(format!("validator-key{}.txt", i));
        std::fs::write(&validator_key_filename, ethereum_private_key)
            .expect("failed to write validator key file");
    }

    // Write all enodes (not just validators) to file if requested
    let all_enodes: Vec<String> = nodes_from_csv.iter().map(|n| n.enode.clone()).collect();
    if write_enodes {
        // Write enodes to file
        std::fs::write(assets_dir.join("enodes.txt"), all_enodes.join(","))
            .expect("failed to write enodes.txt");
    } else {
        // Delete the enodes file if it exists
        std::fs::remove_file(assets_dir.join("enodes.txt")).unwrap_or_default();
    }

    // Create allocation for accounts
    let mut alloc = BTreeMap::new();

    // Add pre-funded account (also serves as admin for the validator contract)
    // Derive address from private key
    let admin_key_str = admin_key.as_deref().unwrap_or(DEFAULT_ADMIN_KEY);

    // Parse the private key and derive the address
    let admin_key_trimmed = admin_key_str.strip_prefix("0x").unwrap_or(admin_key_str);
    let signer: PrivateKeySigner = admin_key_trimmed
        .parse()
        .expect("Failed to parse admin private key");
    let prefunded_address: Address = signer.address();

    println!("Using admin address: {}", prefunded_address);

    let account = GenesisAccount {
        balance: U256::MAX,
        ..Default::default()
    };
    alloc.insert(prefunded_address, account);

    // Only deploy contracts if addresses are provided
    if let (Some(impl_bytecode), Some(prx_bytecode), Some(impl_addr), Some(prx_addr)) = (
        implementation_bytecode,
        proxy_bytecode,
        implementation_address,
        proxy_address,
    ) {
        let validator_config = ValidatorSetConfig {
            max_active_validators: max_active_validators.unwrap_or(num_initial_validators as u64),
            base_fee: base_fee.unwrap_or(4_761_904_761_905),
            block_interval_ms: (block_interval.unwrap_or(1.0) * 1000.0) as u64,
            epoch_length: epoch_length.unwrap_or(32),
        };

        // Initialize proxy storage by simulating initialize() call
        let proxy_storage = initialize_proxy_storage(
            &impl_bytecode,
            &prx_bytecode,
            impl_addr,
            prx_addr,
            prefunded_address, // admin
            &validators,
            validator_config,
            &enodes,
        )
        .expect("failed to initialize validator contract storage");

        // Add implementation contract (code only, no storage)
        let implementation_account = GenesisAccount {
            balance: U256::ZERO,
            code: Some(impl_bytecode.into()),
            storage: None,
            ..Default::default()
        };
        alloc.insert(impl_addr, implementation_account);

        // Add proxy contract (code + initialized storage)
        let proxy_account = GenesisAccount {
            balance: U256::ZERO,
            code: Some(prx_bytecode.into()),
            storage: Some(proxy_storage),
            ..Default::default()
        };
        alloc.insert(prx_addr, proxy_account);

        println!("Implementation contract: {}", impl_addr);
        println!("Proxy contract: {}", prx_addr);
        println!(
            "Initialized genesis with {} initial validators (out of {} total nodes)",
            num_initial_validators, num_nodes
        );
    }

    // Create RBFT config
    let rbft_config = RbftConfig {
        validator_set_contract: proxy_address,
        ..Default::default()
    };

    // Create chain config with RBFT configuration in extra_fields
    let mut extra_fields_map = BTreeMap::new();
    let rbft_value = serde_json::to_value(&rbft_config).expect("failed to serialise RbftConfig");
    extra_fields_map.insert("rbft".to_string(), rbft_value);

    let chain_config = ChainConfig {
        chain_id: 123123,
        homestead_block: Some(0),
        eip150_block: Some(0),
        eip155_block: Some(0),
        eip158_block: Some(0),
        byzantium_block: Some(0),
        constantinople_block: Some(0),
        petersburg_block: Some(0),
        istanbul_block: Some(0),
        berlin_block: Some(0),
        london_block: Some(0),
        // Merge fork conditions - grouped together for alloy-evm compatibility
        merge_netsplit_block: Some(0),
        terminal_total_difficulty: Some(U256::ZERO),
        terminal_total_difficulty_passed: true,
        shanghai_time: Some(0),
        cancun_time: Some(0),
        prague_time: Some(0),
        osaka_time: Some(0),
        extra_fields: serde_json::from_value(
            serde_json::to_value(&extra_fields_map).expect("failed to serialise extra fields map"),
        )
        .unwrap_or_default(),
        ..Default::default()
    };

    // Create the genesis block
    let genesis = Genesis {
        config: chain_config,
        nonce: 0u64,
        timestamp: 0u64,
        extra_data: vec![].into(),
        gas_limit: gas_limit.unwrap_or(600_000_000u64),
        difficulty: U256::ZERO, // Post-merge difficulty should be 0
        mix_hash: keccak256(b"rbft-genesis-prevrandao"), /* Deterministic prevrandao for
                                 * genesis */
        coinbase: Address::ZERO,
        alloc,
        ..Default::default()
    };
    let s = serde_json::to_string_pretty(&genesis).expect("failed to serialise genesis");
    std::fs::write(assets_dir.join("genesis.json"), s).expect("failed to write genesis.json");

    println!(
        "Genesis file written to {:?}",
        assets_dir.join("genesis.json")
    );
}

/// Generate node enodes and secret keys in CSV format
pub fn generate_nodes_csv(
    num_nodes: u32,
    assets_dir: Option<PathBuf>,
    output_path: Option<&str>,
    docker: bool,
    kube: bool,
) -> EyreResult<()> {
    use std::io::Write;

    // Determine assets directory
    let assets_dir = if let Some(dir) = assets_dir {
        dir
    } else {
        // Default to ~/.rbft/testnet
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".rbft").join("testnet")
    };

    // Determine output path: use provided path or default to <assets_dir>/nodes.csv
    let output_path = if let Some(path) = output_path {
        PathBuf::from(path)
    } else {
        // Create directory if it doesn't exist
        std::fs::create_dir_all(&assets_dir)?;
        assets_dir.join("nodes.csv")
    };

    let kube_namespace = if kube {
        crate::kube_namespace::default_kube_namespace()
    } else {
        String::new()
    };
    if kube {
        println!("Using kube namespace: {}", kube_namespace);
    }

    let mut csv_data = Vec::new();

    // Write CSV header
    writeln!(
        csv_data,
        "node_id,validator_address,validator_private_key,p2p_secret_key,enode"
    )?;

    for i in 0..num_nodes {
        // Generate random Ethereum private key and recover address
        let ethereum_signer = PrivateKeySigner::random();
        let ethereum_address = ethereum_signer.address();
        let ethereum_private_key = format!(
            "0x{}",
            alloy_primitives::hex::encode(ethereum_signer.to_bytes())
        );

        // Generate P2P network secret key (separate from validator key)
        let p2p_secret_key = reth_ethereum::network::config::rng_secret_key();
        let p2p_key_hex = alloy_primitives::hex::encode(p2p_secret_key.as_ref());

        // Create enode for P2P networking
        let socket_addr = if docker || kube {
            // Use 0.0.0.0 for Docker/Kubernetes (will be replaced with hostname)
            // Port is always 30303 inside the container (mapped differently on host)
            SocketAddr::from(([0, 0, 0, 0], 30303))
        } else {
            // Use localhost with different ports for local testing
            SocketAddr::from(([127, 0, 0, 1], 30303 + i as u16))
        };
        let enode = NodeRecord::from_secret_key(socket_addr, &p2p_secret_key);

        // For Docker/Kubernetes, replace the IP with the appropriate hostname
        let enode_str = if docker || kube {
            let enode_str = enode.to_string();
            let hostname = if kube {
                // Use Kubernetes StatefulSet DNS: pod-name.service-name.namespace.svc.cluster.local
                format!(
                    "rbft-node-{}.rbft-node.{}.svc.cluster.local",
                    i,
                    kube_namespace.as_str()
                )
            } else {
                // Use Docker compose service name: node0, node1, etc.
                format!("node{}", i)
            };
            // Replace 0.0.0.0 with the hostname
            enode_str.replace("0.0.0.0", &hostname)
        } else {
            enode.to_string()
        };

        // Write CSV row
        writeln!(
            csv_data,
            "{},{},{},{},{}",
            i, ethereum_address, ethereum_private_key, p2p_key_hex, enode_str
        )?;
    }

    // Write to file
    std::fs::write(&output_path, csv_data)?;

    println!(
        "Generated {} nodes and saved to {}",
        num_nodes,
        output_path.display()
    );
    println!("CSV format: node_id,validator_address,validator_private_key,p2p_secret_key,enode");

    Ok(())
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
        return Ok(enode_str.to_string());
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
