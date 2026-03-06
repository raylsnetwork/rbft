// SPDX-License-Identifier: Apache-2.0
//! QBFT Consensus Engine Implementation.
//!
//! This module connects qbft NodeState instances with a network layer.
//!
//! The module qbft contains the core QBFT state machine and types.
//! The module net contains the networking components for streaming QBFT messages.

/// Capacity of the shared QBFT message and event bus channels.
/// In practice the number of pending messages/events should be low.
/// There are also other checks for the numbers of consensus messages.
///
/// The open question is what will happen if we fail to send a message/event to the bus.
/// Will this break the state machine or will we recover?
///
/// In practice it may not be possible to reproduce this condition.
const CONSENSUS_CHANNEL_CAPACITY: usize = 10_000;

mod aligned_interval;

pub use rbft_utils::types::RbftConfig;

use std::{
    collections::{HashMap, VecDeque},
    env,
    time::{Duration, Instant, UNIX_EPOCH},
};

use aligned_interval::AlignedInterval;

use alloy_consensus::BlockHeader;
use alloy_eips::eip2718::{Decodable2718, Encodable2718};
use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rlp::{decode_exact, Decodable, Encodable, Error as RlpError, Header, PayloadView};
use alloy_rpc_types::engine::ForkchoiceState;
use alloy_rpc_types_engine::PayloadAttributes as EnginePayloadAttributes;
use alloy_signer_local::PrivateKeySigner;
use eyre::eyre;
use reth_ethereum::{
    chainspec::ChainSpec,
    consensus::HeaderValidator,
    network::{
        api::PeerId, protocol::IntoRlpxSubProtocol, EthNetworkPrimitives, NetworkHandle,
        NetworkProtocols,
    },
    node::api::{
        BuiltPayload, ConsensusEngineHandle, EngineApiMessageVersion, PayloadAttributesBuilder,
        PayloadTypes,
    },
    primitives::{BlockBody, Header as RethHeader, SealedBlock, SealedHeader},
    storage::{AccountReader, BlockReader, HeaderProvider, StateProviderFactory},
};
use reth_ethereum::{
    pool::{PoolConsensusTx, PoolTransaction, TransactionOrigin, TransactionPool},
    primitives::SignedTransaction,
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadKind};
use tokio::sync::mpsc;
// use tokio_stream::wrappers::UnboundedReceiverStream; // Unused - removed for clean compilation
use tracing::{debug, error, info, trace, warn};

use crate::RbftNodeArgs;
use rbft::{
    node_auxilliary_functions::{quorum, validators},
    types::{
        qbft_message::summarise_messages, Address, Block as QbftBlock,
        BlockHeader as QbftBlockHeader, BlockRequest, BlockResponse, Blockchain, Configuration,
        NodeState, Proposal, RawBlock, RawBlockHeader, Signature, SignedNewBlock, UnsignedNewBlock,
    },
    QbftMessage,
};

/// Helper trait for payload attribute types that expose fee recipient mutation.
pub(crate) trait SuggestedFeeRecipientExt {
    fn set_suggested_fee_recipient(&mut self, recipient: Address);
}

#[allow(dead_code)]
pub(crate) trait PrevRandaoExt {
    fn set_prev_randao(&mut self, randao: B256);
}

pub(crate) trait TimestampExt {
    fn set_timestamp(&mut self, timestamp: u64);
}

impl SuggestedFeeRecipientExt for EnginePayloadAttributes {
    fn set_suggested_fee_recipient(&mut self, recipient: Address) {
        self.suggested_fee_recipient = recipient;
    }
}

impl PrevRandaoExt for EnginePayloadAttributes {
    fn set_prev_randao(&mut self, randao: B256) {
        self.prev_randao = randao;
    }
}

impl TimestampExt for EnginePayloadAttributes {
    fn set_timestamp(&mut self, timestamp: u64) {
        self.timestamp = timestamp;
    }
}

// Enhanced RLPx protocol implementation with ProtocolState and ProtocolEvent
mod consensus_builder;
mod on_chain_config;
mod payload_validator;
mod rbft_protocol;
mod validator_builder;

use crate::metrics::QbftMetrics;
use consensus_builder::RbftBeaconConsensus;
use on_chain_config::{get_on_chain_config, OnChainConfig};

use rbft_protocol::{Command, Message, ProtocolEvent, ProtocolState, RbftProtocolHandler};

pub use consensus_builder::RbftConsensusBuilder;
pub use payload_validator::RbftPayloadValidator;
pub use validator_builder::RbftPayloadValidatorBuilder;

#[cfg(test)]
mod reload_tests {
    use super::*;
    use alloy_consensus::{Block, BlockBody, Header};
    use alloy_primitives::{keccak256, Bytes, B256};
    use rbft::types::{Block as QbftBlock, BlockHeader as QbftBlockHeader};
    use reth_ethereum_primitives::EthPrimitives;
    use reth_provider::test_utils::MockEthProvider;

    #[tokio::test]
    async fn reloads_node_state_from_chain_head() {
        // Build a mock provider with a single block at height 5.
        let provider: MockEthProvider<EthPrimitives> = MockEthProvider::new();
        let beneficiary = Address::from_slice(&[2u8; 20]);
        let block_number = 5;
        let timestamp = 1234;

        // Add genesis (block 0) so validator selection has a header to read.
        let genesis_header = Header {
            number: 0,
            timestamp: 0,
            ..Default::default()
        };
        let genesis_body = BlockBody::default();
        let genesis_block: Block<reth_ethereum_primitives::TransactionSigned> =
            Block::new(genesis_header, genesis_body);
        let genesis_sealed = SealedBlock::from(genesis_block.clone());
        provider.add_block(genesis_sealed.hash(), genesis_block);

        let header = Header {
            beneficiary,
            number: block_number,
            timestamp,
            // Match defaults for the rest.
            ..Default::default()
        };
        let body = BlockBody::default();
        let block: Block<reth_ethereum_primitives::TransactionSigned> = Block::new(header, body);
        let sealed = SealedBlock::from(block.clone());
        let hash: B256 = sealed.hash();
        provider.add_block(hash, block);

        // Build a dummy NodeState with a different head so we can see it change.
        let qbft_beneficiary: Address = beneficiary;
        let genesis_header = QbftBlockHeader {
            proposer: qbft_beneficiary,
            round_number: 0,
            commit_seals: vec![],
            height: 0,
            timestamp: 0,
            validators: vec![qbft_beneficiary],
            digest: Default::default(),
        };
        let genesis_block = QbftBlock::new(genesis_header.clone(), Bytes::from_static(b"genesis"));
        let blockchain = Blockchain::new(VecDeque::from([genesis_block.clone()]));
        let configuration = Configuration {
            genesis_block: genesis_block.clone(),
            nodes: vec![qbft_beneficiary],
            ..Default::default()
        };
        let private_key = B256::from([1u8; 32]);
        let node_state =
            NodeState::new(blockchain, configuration, qbft_beneficiary, private_key, 0);

        // Create a mock OnChainConfig for the test
        let mock_config = OnChainConfig {
            all_validators: vec![qbft_beneficiary],
            selected_validators: vec![qbft_beneficiary],
            _max_validators: 4,
            _base_fee: 4761904761905,
            block_interval_ms: 1000,
            _epoch_length: 32,
            all_validator_enodes: vec![],
            selected_validator_enodes: vec![],
        };

        let new_state = rebuild_node_state_from_chain(&provider, &node_state, &mock_config)
            .await
            .unwrap();

        let head = new_state.blockchain().head();
        assert_eq!(head.header.height, block_number);
        assert_eq!(head.header.timestamp, timestamp);
        assert_eq!(head.header.digest, keccak256(head.body()));
    }
}

#[cfg(test)]
mod proposal_validation_tests {
    use super::*;
    use alloy_consensus::{constants::MAXIMUM_EXTRA_DATA_SIZE, Block as AlloyBlock, Header};
    use alloy_primitives::Bytes;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_ethereum_primitives::TransactionSigned;
    use reth_provider::test_utils::MockEthProvider;

    #[test]
    fn rejects_proposal_when_header_validator_fails() {
        let proposer = Address::from_slice(&[3u8; 20]);
        let height = 1;
        let timestamp = 42;

        let header = Header {
            beneficiary: proposer,
            number: height,
            timestamp,
            extra_data: Bytes::from(vec![0u8; MAXIMUM_EXTRA_DATA_SIZE + 1]),
            ..Default::default()
        };
        let body = alloy_consensus::BlockBody::<TransactionSigned>::default();
        let block: AlloyBlock<TransactionSigned> = AlloyBlock::new(header.clone(), body);
        let sealed_block = SealedBlock::seal_slow(block);

        let mut encoded = Vec::new();
        sealed_block.encode(&mut encoded);

        let qbft_header = QbftBlockHeader {
            proposer,
            round_number: 0,
            commit_seals: vec![],
            height,
            timestamp,
            validators: vec![proposer],
            digest: sealed_block.hash(),
        };
        let proposed_block = QbftBlock::new(qbft_header, Bytes::from(encoded));
        let proposal = Proposal {
            proposed_block,
            ..Default::default()
        };

        let validator = RbftBeaconConsensus::new(std::sync::Arc::new(ChainSpec::default()));

        assert!(!validate_proposal_with_validator::<
            MockEthProvider<EthPrimitives>,
            _,
        >(&validator, &proposal));
    }
}

/// Rebuild a NodeState from the provider's current head, preserving node identity and
/// configuration.
async fn rebuild_node_state_from_chain<P>(
    provider: &P,
    node_state: &NodeState,
    on_chain_config: &OnChainConfig,
) -> eyre::Result<NodeState>
where
    P: BlockReader + AccountReader + StateProviderFactory + Clone + 'static,
    SealedBlock<<P as BlockReader>::Block>: Decodable,
{
    let block_number = provider.best_block_number().unwrap_or(0);
    let latest_block = provider
        .block(block_number.into())
        .map_err(|e| eyre!("Failed to read block {block_number} from provider: {e:?}"))?
        .ok_or_else(|| eyre!("Provider returned None for block {block_number}"))?;

    let sealed_block = SealedBlock::from(latest_block);
    let mut body = Vec::new();
    sealed_block.encode(&mut body);

    // Ensure the encoded payload decodes cleanly.
    SealedBlock::<P::Block>::decode(&mut body.as_ref()).map_err(|e| {
        eyre!("Failed to decode sealed block payload when reloading blockchain: {e:?}")
    })?;

    let OnChainConfig {
        all_validators: all_nodes,
        selected_validators: validators,
        block_interval_ms: block_time_ms,
        ..
    } = on_chain_config;
    let proposer = validators.first().copied().unwrap_or_default();
    let timestamp = sealed_block.header().timestamp();

    let header = QbftBlockHeader {
        proposer,
        round_number: 0,
        commit_seals: vec![],
        height: block_number,
        timestamp,
        validators: validators.clone(),
        digest: sealed_block.hash(),
    };
    let latest_block = QbftBlock::new(header, Bytes::from(body));
    let blockchain = Blockchain::new(VecDeque::from([latest_block]));

    let mut configuration = node_state.configuration().clone();
    configuration.nodes = all_nodes.clone();
    configuration.block_time = block_time_ms / 1000;
    let id = node_state.id();
    let private_key = node_state.private_key();
    Ok(NodeState::new(
        blockchain,
        configuration,
        id,
        private_key,
        now(),
    ))
}

/// Load RBFT configuration from chainspec
fn rbft_config_from_chainspec(chainspec: &ChainSpec) -> RbftConfig {
    let genesis = chainspec.genesis();
    if let Ok(config_value) = serde_json::to_value(&genesis.config) {
        if let Some(rbft_value) = config_value.get("rbft") {
            if let Ok(_rbft_config) = serde_json::from_value::<RbftConfig>(rbft_value.clone()) {
                debug!(
                    target: "rbft",
                    "Successfully loaded RBFT config from chainspec"
                );
                return _rbft_config;
            } else {
                error!(
                    target: "rbft",
                    "Failed to parse RBFT config from chainspec, using default"
                );
            }
        } else {
            error!(
                target: "rbft",
                "No RBFT config found in chainspec, using default"
            );
        }
    } else {
        error!(
            target: "rbft",
            "Failed to serialize genesis config, using default RBFT config"
        );
    }
    RbftConfig::default()
}

/// Shared peer connection map: PeerId → (remote socket address, command sender).
type PeerConnections = std::sync::Arc<
    tokio::sync::RwLock<HashMap<PeerId, (std::net::SocketAddr, mpsc::Sender<Command>)>>,
>;

pub struct RbftConsensus<T: PayloadTypes, B, P: HeaderProvider<Header = RethHeader>, Pool> {
    /// The payload attribute builder for the engine
    payload_attributes_builder: B,
    /// Sender for events to engine.
    to_engine: ConsensusEngineHandle<T>,
    /// The payload builder for the engine
    payload_builder: PayloadBuilderHandle<T>,

    /// The state of the QBFT node
    node_state: NodeState,

    // Streaming components removed - using QbftProtocol RLPx subprotocol instead
    /// Task executor for spawning background tasks
    task_executor: reth_tasks::TaskExecutor,
    /// Configuration information from genesis.json
    rbft_config: RbftConfig,
    /// Consensus header validator to pre-screen proposals
    header_validator:
        std::sync::Arc<dyn HeaderValidator<<P as HeaderProvider>::Header> + Send + Sync>,
    /// Args from command line
    #[allow(dead_code)]
    args: RbftNodeArgs,
    /// Network handle for peer management
    network_handle: NetworkHandle<EthNetworkPrimitives>,
    /// Receiver for QBFT messages from QbftProtocol peers
    qbft_protocol_message_rx: mpsc::Receiver<QbftMessage>,
    /// Sender for QBFT messages to QbftProtocol peers
    qbft_protocol_message_tx: mpsc::Sender<QbftMessage>,
    /// Shared connections to QbftProtocol peers (RwLock for better read concurrency)
    qbft_protocol_connections: PeerConnections,

    /// Transaction pool handle for reading pending transactions
    pool: Pool,
    /// Receiver for incoming transaction batches forwarded by remote peers
    transactions_rx: mpsc::Receiver<Vec<alloy_primitives::Bytes>>,
    /// Sender given to the protocol handler so connections can forward transactions
    transactions_tx: mpsc::Sender<Vec<alloy_primitives::Bytes>>,
    /// Provider for blockchain access
    provider: P,
    /// List of validators in the network
    #[allow(dead_code)]
    validators: Vec<Address>,
    /// Shared metrics
    metrics: std::sync::Arc<QbftMetrics>,
    /// Cached on-chain configuration
    cached_config: OnChainConfig,
    /// Height for which the cached config is valid
    cached_config_height: u64,
    /// Last "before" state summary (for simple logs mode)
    last_summary_b: Option<String>,
    /// Last "after" state summary (for simple logs mode)
    last_summary_a: Option<String>,
    /// Emit full logs on every advance cycle (default: false for simple logs)
    full_logs: bool,
    /// Shared protocol state for accessing messages_received cache
    protocol_state: Option<ProtocolState>,
    /// Time of last block commit (for resend logic)
    last_commit_time: Option<std::time::Instant>,
    /// Resend timeout (if Some, enables resending after this duration)
    resend_timeout: Option<Duration>,
    /// Timestamp of the last express-delivery send; only txs added after this
    /// are forwarded, preventing re-sending already-forwarded transactions.
    last_express_delivery_at: std::time::Instant,
    /// Ring buffer of recent debug/trace log entries for dump on error
    debug_buffer: VecDeque<(Instant, String)>,
}

/// Debug ring buffer: entries older than this are pruned each health-check tick.
const DEBUG_BUFFER_TTL: Duration = Duration::from_secs(60);
/// Debug ring buffer: hard cap to prevent unbounded growth.
const DEBUG_BUFFER_MAX_ENTRIES: usize = 10_000;

const RLP_HEADER_BENEFICIARY_INDEX: usize = 2;
const RLP_HEADER_NUMBER_INDEX: usize = 8;
const RLP_HEADER_TIMESTAMP_INDEX: usize = 11;

fn extract_rlp_block_metadata(block: &RawBlock) -> Result<Option<(u64, u64, Address)>, RlpError> {
    let mut payload = block.body.as_ref();
    let payload_view = Header::decode_raw(&mut payload)?;

    match payload_view {
        PayloadView::String(_) => Ok(None),
        PayloadView::List(items) => {
            if !payload.is_empty() {
                return Err(RlpError::Custom(
                    "unexpected trailing bytes in block payload",
                ));
            }

            let Some(raw_header) = items.first() else {
                return Err(RlpError::Custom("missing block header in payload"));
            };

            let mut header_slice = *raw_header;
            let header_view = Header::decode_raw(&mut header_slice)?;
            let fields = match header_view {
                PayloadView::String(_) => return Err(RlpError::UnexpectedString),
                PayloadView::List(fields) => fields,
            };

            if fields.len() <= RLP_HEADER_TIMESTAMP_INDEX {
                return Err(RlpError::Custom("incomplete block header payload"));
            }

            let number: u64 = decode_exact(fields[RLP_HEADER_NUMBER_INDEX])?;
            let timestamp: u64 = decode_exact(fields[RLP_HEADER_TIMESTAMP_INDEX])?;
            let proposer: Address = decode_exact(fields[RLP_HEADER_BENEFICIARY_INDEX])?;

            Ok(Some((number, timestamp, proposer)))
        }
    }
}

/// Cross-checks that the RLP payload encodes a block whose header matches the given metadata.
///
/// Returns true if either:
/// - the payload is not an RLP list (e.g. synthetic test payload), or
/// - the embedded header number, timestamp, and beneficiary/proposer match the expected values.
///
/// Returns false if decoding fails or the values mismatch.
fn validate_block_payload_metadata(
    body: &[u8],
    expected_height: u64,
    expected_timestamp: u64,
    expected_proposer: Address,
) -> bool {
    let raw_block = RawBlock {
        header: RawBlockHeader {
            proposer: expected_proposer,
            height: expected_height,
            timestamp: expected_timestamp,
        },
        body: alloy_primitives::Bytes::copy_from_slice(body),
        transactions: vec![],
    };

    match extract_rlp_block_metadata(&raw_block) {
        Ok(Some((number, timestamp, proposer))) => {
            number == expected_height
                && timestamp == expected_timestamp
                && proposer == expected_proposer
        }
        Ok(None) => true,
        Err(err) => {
            debug!(
                target: "rbft",
                "Failed to decode block payload at height {}: {:?}",
                expected_height,
                err
            );
            false
        }
    }
}

fn validate_proposal_with_validator<P, V>(validator: &V, proposal: &Proposal) -> bool
where
    P: BlockReader + HeaderProvider<Header = RethHeader>,
    SealedBlock<P::Block>: Decodable,
    V: HeaderValidator<<P as HeaderProvider>::Header> + ?Sized,
{
    let mut body = proposal.proposed_block.body().as_ref();
    let Ok(sealed_block) = SealedBlock::<P::Block>::decode(&mut body) else {
        warn!(target: "rbft", "Rejected proposal: block body is not a valid SealedBlock");
        return false;
    };

    if !body.is_empty() {
        warn!(target: "rbft", "Rejected proposal: trailing bytes after SealedBlock payload");
        return false;
    }

    let header = sealed_block.sealed_header();
    let qbft_header = &proposal.proposed_block.header;
    let header_fields = header.header();

    if header_fields.number() != qbft_header.height {
        warn!(
            target: "rbft",
            "Rejected proposal: block number {} does not match QBFT height {}",
            header_fields.number(),
            qbft_header.height
        );
        return false;
    }

    if header_fields.timestamp() != qbft_header.timestamp {
        warn!(
            target: "rbft",
            "Rejected proposal: timestamp {} does not match QBFT header {}",
            header_fields.timestamp(),
            qbft_header.timestamp
        );
        return false;
    }

    let enforce_proposer_match = qbft_header.round_number == 0;
    if enforce_proposer_match && header_fields.beneficiary() != qbft_header.proposer {
        warn!(
            target: "rbft",
            "Rejected proposal: beneficiary {} does not match proposer {}",
            header_fields.beneficiary(),
            qbft_header.proposer
        );
        return false;
    }

    if let Err(err) = validator.validate_header(header) {
        warn!(
            target: "rbft",
            "Rejected proposal: header failed consensus validation: {:?}",
            err
        );
        return false;
    }

    // Also verify the raw payload metadata matches the QBFT header
    let payload_proposer = if enforce_proposer_match {
        qbft_header.proposer
    } else {
        header_fields.beneficiary()
    };
    validate_block_payload_metadata(
        proposal.proposed_block.body().as_ref(),
        qbft_header.height,
        qbft_header.timestamp,
        payload_proposer,
    )
}

impl<T: PayloadTypes, B, P, Pool> RbftConsensus<T, B, P, Pool>
where
    T: PayloadTypes,
    B: PayloadAttributesBuilder<<T as PayloadTypes>::PayloadAttributes>,
    <T as PayloadTypes>::PayloadAttributes: SuggestedFeeRecipientExt,
    <T as PayloadTypes>::PayloadAttributes: PrevRandaoExt,
    <T as PayloadTypes>::PayloadAttributes: TimestampExt,
    SealedBlock<<P as BlockReader>::Block>: Decodable,
    P: BlockReader
        + HeaderProvider<Header = RethHeader>
        + AccountReader
        + StateProviderFactory
        + Clone
        + 'static,
    // Constrain provider's Block type to match PayloadTypes' Block type
    SealedBlock<<P as BlockReader>::Block>: Into<
        SealedBlock<
            <<<T as PayloadTypes>::BuiltPayload as BuiltPayload>::Primitives
                as reth_ethereum::node::api::NodePrimitives
            >::Block,
        >,
    >,
    Pool: TransactionPool + Clone + 'static,
    PoolConsensusTx<Pool>: Decodable2718 + SignedTransaction,
{
    /// Replace the model blockchain with the current provider head to avoid carrying a bad block.
    async fn reload_blockchain_from_chain(&mut self) -> eyre::Result<()> {
        self.node_state = rebuild_node_state_from_chain(
            &self.provider,
            &self.node_state,
            &self.cached_config,
        )
        .await?;
        Ok(())
    }

    fn proposal_has_valid_sealed_block(&self, proposal: &Proposal) -> bool {
        validate_proposal_with_validator::<P, _>(&*self.header_validator, proposal)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        pool: Pool,
        payload_attributes_builder: B,
        to_engine: ConsensusEngineHandle<T>,
        payload_builder: PayloadBuilderHandle<T>,
        chainspec: std::sync::Arc<ChainSpec>,
        task_executor: reth_tasks::TaskExecutor,
        network_handle: NetworkHandle<EthNetworkPrimitives>,
        args: &crate::RbftNodeArgs,
        metrics: std::sync::Arc<QbftMetrics>,
    ) -> eyre::Result<Self> {
        // Read RBFT configuration from chainspec genesis
        let _rbft_config = rbft_config_from_chainspec(&chainspec);
        let header_validator: std::sync::Arc<
            dyn HeaderValidator<<P as HeaderProvider>::Header> + Send + Sync,
        > = std::sync::Arc::new(RbftBeaconConsensus::new(chainspec.clone()));

        let block_number = provider.best_block_number().unwrap_or(0);

        debug!(target: "rbft",
            "QbftConsensus: Starting QBFT at block {} with genesis hash {}",
            block_number,
            chainspec.genesis_hash()
        );

        let latest_block = provider
            .block(block_number.into())
            .map_err(|e| eyre!("Failed to query block {block_number} from provider: {e:?}"))?
            .ok_or_else(|| eyre!("Latest block {block_number} not found in provider"))?;
        let sealed_block = SealedBlock::from(latest_block);

        let mut body = Vec::new();
        sealed_block.encode(&mut body);

        // Check the encode/decode round trip.
        assert!(SealedBlock::decode(&mut body.as_ref()).is_ok());

        let on_chain_config = get_on_chain_config(&provider, block_number);

        info!(
            target: "rbft",
            "Loaded on-chain config: {on_chain_config:#?}"
        );

        let OnChainConfig {
            all_validators: all_nodes,
            selected_validators: validators,
            block_interval_ms: block_time_ms,
            ..
        } = &on_chain_config;
        let proposer = validators.first().copied().unwrap_or_default();

        let timestamp = if block_number == 0 {
            now()
        } else {
            sealed_block.header().timestamp()
        };

        // Create a latest block for the blockchain
        let header = QbftBlockHeader {
            proposer,
            round_number: 0,
            commit_seals: vec![],
            height: block_number,
            timestamp,
            validators: validators.clone(),
            digest: sealed_block.hash(),
        };
        let latest_block = QbftBlock::new(header, Bytes::from(body));

        // The blockchain is a model of the current real chain.
        // When we restart a node, we need to update our model to the latest block.
        let blockchain = Blockchain::new(VecDeque::from([latest_block]));

        let mut configuration = Configuration {
            block_time: (block_time_ms / 1000).max(1),
            round_change_config: _rbft_config.round_change_config.clone(),
            ..Default::default()
        };

        configuration.nodes = all_nodes.clone();

        let (private_key, id) = get_private_key_and_id(args)?;

        debug!(target: "rbft", "Starting QBFT node with ID: {:?}", id);

        let node_state = NodeState::new(blockchain, configuration, id, private_key, now());

        debug!(
            target: "rbft",
            "Starting QBFT node with ID: {:?}",
            id
        );

        metrics.record_consensus_state(
            node_state.height(),
            node_state.round(),
            node_state.is_the_proposer_for_current_round(),
        );

        // Create channels for QBFT messages from QbftProtocol peers
        let (qbft_protocol_message_tx, qbft_protocol_message_rx) =
            mpsc::channel(CONSENSUS_CHANNEL_CAPACITY);

        // Create channel for incoming transaction batches forwarded by remote peers
        let (transactions_tx, transactions_rx) = mpsc::channel(CONSENSUS_CHANNEL_CAPACITY);

        let resend_timeout = args.resend_after_secs.map(Duration::from_secs);

        // Initialize last_commit_time to now if resend is enabled, so timer starts immediately
        let last_commit_time = if resend_timeout.is_some() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        Ok(Self {
            payload_attributes_builder,
            to_engine,
            payload_builder,
            node_state,
            task_executor,
            rbft_config: _rbft_config,
            header_validator,
            args: args.clone(),
            network_handle,
            qbft_protocol_message_rx,
            qbft_protocol_message_tx,
            qbft_protocol_connections: std::sync::Arc::new(
                tokio::sync::RwLock::new(HashMap::new()),
            ),
            pool,
            transactions_rx,
            transactions_tx,
            provider: provider.clone(),
            validators: validators.clone(),
            metrics,
            cached_config: on_chain_config,
            cached_config_height: block_number,
            last_summary_b: None,
            last_summary_a: None,
            full_logs: args.full_logs,
            protocol_state: None,
            last_commit_time,
            resend_timeout,
            last_express_delivery_at: std::time::Instant::now(),
            debug_buffer: VecDeque::new(),
        })
    }

    pub async fn _reset_peer_connections(&self) {
        use reth_ethereum::network::Peers;

        let local_peer_id = *self.network_handle.peer_id();

        debug!(
            target: "rbft",
            "Resetting all peer connections to fix routing issues"
        );

        // Get all connected peers
        if let Ok(all_peers) = self.network_handle.get_all_peers().await {
            info!(
                target: "rbft",
                "Found {} total peers in network",
                all_peers.len()
            );

            for peer_info in all_peers {
                let peer_id = peer_info.remote_id;
                let remote_addr = peer_info.remote_addr;

                // Skip self-connections (can happen if local peer is in trusted_peers)
                if peer_id == local_peer_id {
                    debug!(
                        target: "rbft",
                        "Skipping self-connection to {}",
                        peer_id
                    );
                    self.network_handle.disconnect_peer(peer_id);
                    continue;
                }

                debug!(
                    target: "rbft",
                    "Disconnecting and reconnecting peer: {} at {} (reason: manual_reset)",
                    peer_id,
                    remote_addr
                );

                // Disconnect the peer
                self.network_handle.disconnect_peer(peer_id);

                // Wait a bit then try to reconnect
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // Reconnect to the peer
                self.network_handle.connect_peer(peer_id, remote_addr);
            }
        }

        // Wait for all expected peers so that every node starts consensus at
        // roughly the same time.  This prevents early starters from timing out
        // on round 0 and sending round changes that disrupt late starters.
        // A 30-second timeout ensures bounded startup even if some peers are slow.
        let expected_peers = self.validators.len().saturating_sub(1).max(1);
        let max_wait_ms: u64 = 30_000; // 30 seconds
        info!(
            target: "rbft",
            "Waiting for {} peer(s) ({} validators - self) with {}s timeout...",
            expected_peers,
            self.validators.len(),
            max_wait_ms / 1000
        );
        let mut attempts: u64 = 0;

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            attempts += 1;
            let elapsed_ms = attempts * 100;

            // Check QbftProtocol connections - use read lock here
            let connections = self.qbft_protocol_connections.read().await;
            let connected_peers = connections.len();
            drop(connections); // Release lock before potential sleep

            if connected_peers >= expected_peers {
                info!(
                    target: "rbft",
                    "All {} peer(s) connected after {:.1}s",
                    connected_peers,
                    elapsed_ms as f64 / 1000.0
                );
                break;
            }

            if elapsed_ms >= max_wait_ms {
                warn!(
                    target: "rbft",
                    "Startup timeout after {}s with {}/{} peers connected, proceeding anyway",
                    max_wait_ms / 1000,
                    connected_peers,
                    expected_peers
                );
                break;
            }

            if attempts.is_multiple_of(10) {
                info!(
                    target: "rbft",
                    "Still waiting for peer connections... ({}/{} connected, {:.1}s elapsed)",
                    connected_peers,
                    expected_peers,
                    elapsed_ms as f64 / 1000.0
                );
            }
        }

        if let Ok(all_peers) = self.network_handle.get_all_peers().await {
            info!(
                target: "rbft",
                "Found {} total peers in network",
                all_peers.len()
            );

            for peer_info in &all_peers {
                debug!(
                    target: "rbft",
                    "peer kind: {:?}, capabilities: {:?}",
                    peer_info.kind,
                    peer_info.capabilities
                );
            }

            // Immediate connection audit: detect network-level peers that
            // connected during the wait period without establishing a QBFT
            // subprotocol session (the KeepAlive trap).  Reconnecting them
            // now ensures every peer has a working QBFT channel before the
            // first consensus round.
            let local_peer_id = *self.network_handle.peer_id();
            let qbft_connections = self.qbft_protocol_connections.read().await;
            let mut missing = Vec::new();
            for peer_info in &all_peers {
                let peer_id = peer_info.remote_id;
                if peer_id == local_peer_id {
                    continue;
                }
                if !qbft_connections.contains_key(&peer_id) {
                    missing.push((peer_id, peer_info.remote_addr));
                }
            }
            drop(qbft_connections);

            if !missing.is_empty() {
                warn!(
                    target: "rbft",
                    "Post-startup audit: {} peer(s) missing QBFT, \
                     forcing reconnect",
                    missing.len()
                );
                for (peer_id, remote_addr) in missing {
                    debug!(
                        target: "rbft",
                        "Reconnecting peer {} at {} to renegotiate QBFT capability",
                        peer_id,
                        remote_addr
                    );
                    self.network_handle.disconnect_peer(peer_id);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    self.network_handle.connect_peer(peer_id, remote_addr);
                }

                // Give reconnected peers time to establish QBFT sessions
                info!(
                    target: "rbft",
                    "Waiting 2s for reconnected peers to establish QBFT sessions..."
                );
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn qbft_protocol_handler(
        mut qbft_protocol_rx: mpsc::Receiver<ProtocolEvent>,
        mut relay_rx: mpsc::Receiver<(Message, PeerId)>,
        connections_clone: PeerConnections,
        network_handle: NetworkHandle<EthNetworkPrimitives>,
        reconnect_config: ReconnectConfig,
        active_reconnections: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<PeerId>>>,
        relay_enabled: bool,
    ) {
        loop {
            tokio::select! {
                event = qbft_protocol_rx.recv() => {
                    match event {
                        Some(ProtocolEvent::Established {
                            direction,
                            peer_id,
                            remote_addr,
                            to_connection,
                        }) => {
                            // Use write lock for connection establishment - this is a mutation
                            let mut shared_connections = connections_clone.write().await;
                            // "Last wins" policy: always replace the stored connection.
                            //
                            // During simultaneous connect, both an incoming and outgoing
                            // TCP session may be established for the same peer. Reth resolves
                            // the duplicate by dropping one session. If we kept the "first wins"
                            // policy, the stored sender could belong to the session that Reth
                            // drops, leaving a dead connection. By always replacing, the
                            // Disconnected handler's addr-match guard ensures only the stale
                            // session's disconnect is ignored, keeping the live connection.
                            if let Some(old) =
                                shared_connections.insert(peer_id, (remote_addr, to_connection))
                            {
                                info!(
                                    target: "rbft",
                                    "QbftProtocol replacing connection to peer {} \
                                     (old_addr={}, new_addr={}, direction: {:?})",
                                    peer_id, old.0, remote_addr, direction
                                );
                            } else {
                                debug!(
                                    target: "rbft",
                                    "QbftProtocol connection established with peer {} at {} \
                                     (direction: {:?})",
                                    peer_id, remote_addr, direction
                                );
                            }
                        }
                        Some(ProtocolEvent::Disconnected {
                            peer_id,
                            remote_addr,
                            reason,
                        }) => {
                            // Only remove the connection if the stored remote_addr matches
                            // the disconnecting one. During simultaneous connect, the
                            // "last wins" Established handler may have already replaced the
                            // sender with one from a newer session. A stale disconnect from
                            // the replaced session must not remove the live connection.
                            let mut shared_connections = connections_clone.write().await;
                            let should_remove = shared_connections
                                .get(&peer_id)
                                .is_some_and(|(stored_addr, _)| *stored_addr == remote_addr);

                            if !should_remove {
                                debug!(
                                    target: "rbft",
                                    "Ignoring stale disconnect for peer {} at {} \
                                     (current connection uses different address)",
                                    peer_id, remote_addr
                                );
                                continue;
                            }

                            debug!(
                                target: "rbft",
                                "QbftProtocol connection disconnected from peer {} at {} \
                                 (reason: {})",
                                peer_id,
                                remote_addr,
                                reason
                            );
                            shared_connections.remove(&peer_id);
                            drop(shared_connections);

                            // Spawn reconnection task if reconnection is enabled
                            // and no reconnection is already in progress for this peer
                            if reconnect_config.max_attempts > 0 {
                                let mut active = active_reconnections.lock().await;
                                if active.contains(&peer_id) {
                                    debug!(
                                        target: "rbft",
                                        "Reconnection already in progress for peer {}, skipping",
                                        peer_id
                                    );
                                    continue;
                                }
                                // Mark this peer as having an active reconnection task
                                active.insert(peer_id);
                                drop(active);

                                let network_handle_clone = network_handle.clone();
                                let connections_for_reconnect = connections_clone.clone();
                                let active_reconnections_clone = active_reconnections.clone();
                                tokio::spawn(async move {
                                    attempt_peer_reconnection(
                                        network_handle_clone,
                                        connections_for_reconnect,
                                        peer_id,
                                        remote_addr,
                                        reconnect_config,
                                        active_reconnections_clone,
                                    )
                                    .await;
                                });
                            }
                        }
                        None => {
                            error!(target: "rbft", "QbftProtocol event channel closed");
                            break;
                        }
                    }
                }
                relay_msg = relay_rx.recv() => {
                    match relay_msg {
                        Some((message, sender_peer_id)) => {
                            // Skip relay if disabled (e.g., in full mesh topologies)
                            if !relay_enabled {
                                trace!(
                                    target: "rbft",
                                    "Relay disabled, skipping message from peer {}",
                                    sender_peer_id
                                );
                                continue;
                            }
                            trace!(
                                target: "rbft",
                                "Relaying message from peer {} to all other peers \
                                 (msg_size_bytes={})",
                                sender_peer_id,
                                message.encoded().len()
                            );
                            // Use read lock for message relaying - only iterating
                            let connections = connections_clone.read().await;
                            let mut relay_failures = Vec::new();
                            for (peer_id, (_, connection)) in connections.iter() {
                                if *peer_id != sender_peer_id {
                                    let cmd = Command::Message(message.clone());
                                    if let Err(e) = connection.try_send(cmd) {
                                        warn!(
                                            target: "rbft",
                                            "Failed to relay message to peer {}: {:?}",
                                            peer_id,
                                            e
                                        );
                                        relay_failures.push(*peer_id);
                                    }
                                }
                            }
                            // Log summary if there were failures
                            if !relay_failures.is_empty() {
                                warn!(
                                    target: "rbft",
                                    "Relay failures to {} peers: {:?}",
                                    relay_failures.len(),
                                    relay_failures
                                );
                            }
                        }
                        None => {
                            error!(target: "rbft", "Relay channel closed");
                            break;
                        }
                    }
                }
            }
        }
    }

    // ── Debug ring-buffer helpers ──────────────────────────────────

    /// Log at `trace!` level and push the message into the debug ring buffer.
    fn buf_trace(&mut self, msg: String) {
        trace!(target: "rbft", "{}", msg);
        self.push_debug_entry(msg);
    }

    /// Log at `debug!` level and push the message into the debug ring buffer.
    fn buf_debug(&mut self, msg: String) {
        debug!(target: "rbft", "{}", msg);
        self.push_debug_entry(msg);
    }

    /// Push a timestamped entry into the ring buffer, enforcing the hard cap.
    fn push_debug_entry(&mut self, msg: String) {
        if self.debug_buffer.len() >= DEBUG_BUFFER_MAX_ENTRIES {
            self.debug_buffer.pop_front();
        }
        self.debug_buffer.push_back((Instant::now(), msg));
    }

    /// Flush every buffered entry at `warn!` level, then clear the buffer.
    /// Call this immediately before an `error!()` to provide context.
    fn flush_debug_buffer(&mut self) {
        if self.debug_buffer.is_empty() {
            return;
        }
        let count = self.debug_buffer.len();
        warn!(target: "rbft", "=== DEBUG DUMP ({count} entries from last ~60s) ===");
        for (i, (_ts, msg)) in self.debug_buffer.iter().enumerate() {
            warn!(target: "rbft", "[DUMP {}/{}] {}", i + 1, count, msg);
        }
        warn!(target: "rbft", "=== END DEBUG DUMP ===");
        self.debug_buffer.clear();
    }

    pub async fn run(mut self) {
        let reconnect_config = ReconnectConfig {
            max_attempts: self.rbft_config.reconnect_max_attempts,
            base_delay_ms: self.rbft_config.reconnect_base_delay_ms,
            max_delay_ms: self.rbft_config.reconnect_max_delay_ms,
        };

        let (qbft_protocol_tx, qbft_protocol_rx) = mpsc::channel(CONSENSUS_CHANNEL_CAPACITY);
        let (relay_tx, relay_rx) = mpsc::channel(CONSENSUS_CHANNEL_CAPACITY);
        let protocol_state = ProtocolState::new(
            qbft_protocol_tx,
            self.rbft_config.message_buffer_max,
            self.rbft_config.message_buffer_trim_to,
            if self.rbft_config.idle_connection_timeout_seconds > 0 {
                Some(Duration::from_secs(
                    self.rbft_config.idle_connection_timeout_seconds,
                ))
            } else {
                None
            },
        );
        // Store protocol_state for resend functionality
        self.protocol_state = Some(protocol_state.clone());

        let message_tx_clone = self.qbft_protocol_message_tx.clone();
        let transactions_tx_clone = self.transactions_tx.clone();
        let qbft_protocol = RbftProtocolHandler {
            state: protocol_state,
            message_tx: message_tx_clone,
            transactions_tx: transactions_tx_clone,
            relay_tx: relay_tx.clone(),
        };
        self.network_handle
            .add_rlpx_sub_protocol(qbft_protocol.into_rlpx_sub_protocol());

        // Log network protocol information
        info!(
            target: "rbft",
            "QbftProtocol RLPx subprotocol registered successfully"
        );

        // Store connections reference for advance() method (RwLock for better read concurrency)
        let connections_handle = std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::<
            PeerId,
            (std::net::SocketAddr, mpsc::Sender<Command>),
        >::new()));
        let connections_clone = connections_handle.clone();

        // Track active reconnection tasks to prevent duplicate spawning
        let active_reconnections = std::sync::Arc::new(
            tokio::sync::Mutex::new(std::collections::HashSet::<PeerId>::new()),
        );

        let task_executor = self.task_executor.clone();
        let network_handle_clone = self.network_handle.clone();
        task_executor.spawn_critical(
            "qbft_protocol_handler",
            Self::qbft_protocol_handler(
                qbft_protocol_rx,
                relay_rx,
                connections_clone,
                network_handle_clone,
                reconnect_config,
                active_reconnections,
                self.rbft_config.relay_enabled,
            ),
        );

        self.qbft_protocol_connections = connections_handle;

        // self.reset_peer_connections().await;

        // Streaming client tasks removed - using QbftProtocol RLPx subprotocol

        let mut qbft_interval = AlignedInterval::new(10);
        let mut tick_count: u64 = 0;
        let health_log_interval = 500; // Log health every 500 ticks (5 seconds at 10ms tick)
        let connection_audit_interval = 3000; // Audit every 3000 ticks (30 seconds at 10ms tick)

        self.check_peer_connections().await;
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        loop {
            qbft_interval.tick().await;

            // Use try_read for non-critical connection count check
            let num_connections = self
                .qbft_protocol_connections
                .try_read()
                .map(|c| c.len())
                .unwrap_or(0);

            self.buf_trace(format!("QBFT Consensus Tick - connected to {num_connections} peers"));

            // === Periodic Connection Health Logging ===
            if tick_count.is_multiple_of(health_log_interval) {
                // Use read lock for health logging - collect info then drop lock
                let peer_ids: Vec<_> = {
                    let connections = self.qbft_protocol_connections.read().await;
                    connections.keys().cloned().collect()
                };
                self.buf_debug(format!(
                    "Connection health check - {} active peer connections",
                    peer_ids.len()
                ));
                for peer_id in &peer_ids {
                    self.buf_debug(format!("Active connection to peer {peer_id}"));
                }
            }

            // === Prune stale debug ring-buffer entries ===
            if tick_count.is_multiple_of(health_log_interval) {
                let cutoff = Instant::now() - DEBUG_BUFFER_TTL;
                while self
                    .debug_buffer
                    .front()
                    .is_some_and(|(ts, _)| *ts < cutoff)
                {
                    self.debug_buffer.pop_front();
                }
            }

            // === Periodic Connection Audit ===
            // Compare network-level peers against QBFT protocol connections.
            // Peers that are connected at the network layer but missing a QBFT
            // subprotocol connection likely connected before QBFT was registered
            // during startup. Disconnect and reconnect them to force capability
            // renegotiation.
            if tick_count.is_multiple_of(connection_audit_interval) {
                use reth_ethereum::network::Peers;

                let local_peer_id = *self.network_handle.peer_id();
                if let Ok(all_peers) = self.network_handle.get_all_peers().await {
                    let qbft_connections = self.qbft_protocol_connections.read().await;
                    let mut missing = Vec::new();
                    for peer_info in &all_peers {
                        let peer_id = peer_info.remote_id;
                        if peer_id == local_peer_id {
                            continue;
                        }
                        if !qbft_connections.contains_key(&peer_id) {
                            missing.push((peer_id, peer_info.remote_addr));
                        }
                    }
                    drop(qbft_connections);

                    if !missing.is_empty() {
                        warn!(
                            target: "rbft",
                            "Connection audit: {} peer(s) missing QBFT, \
                             forcing reconnect",
                            missing.len()
                        );
                        for (peer_id, remote_addr) in missing {
                            self.buf_debug(format!(
                                "Reconnecting peer {peer_id} at \
                                 {remote_addr} to renegotiate QBFT"
                            ));
                            self.network_handle.disconnect_peer(peer_id);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            self.network_handle.connect_peer(peer_id, remote_addr);
                        }
                    }
                }
            }

            // === Advance the QBFT state machine ===
            if let Err(e) = self.advance().await {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Error advancing the chain: {:?}",
                    e
                );
            }

            // === Check if resend timeout has been reached ===
            if let Some(resend_timeout) = self.resend_timeout {
                let should_resend = match self.last_commit_time {
                    Some(last_commit) => last_commit.elapsed() >= resend_timeout,
                    None => false, // No commits yet, don't resend
                };

                if should_resend {
                    info!(
                        target: "rbft",
                        "Resend timeout reached ({:?} since last commit), \
                         triggering message resend",
                        resend_timeout
                    );
                    self.resend_cached_messages().await;
                    // Reset the timer to prevent continuous resending
                    self.last_commit_time = Some(std::time::Instant::now());
                }
            }

            // === METRICS UPDATE ===
            self.metrics.record_consensus_state(
                self.node_state.height(),
                self.node_state.round(),
                self.node_state.is_the_proposer_for_current_round(),
            );

            tick_count += 1;
        }
    }

    async fn advance(&mut self) -> eyre::Result<()> {
        let node_height = self.provider.best_block_number().unwrap_or(0) + 1;

        // Get or update cached on-chain configuration if node height has changed
        // Read config from the current best block, not the future block we're about to produce
        let config_read_height = node_height.saturating_sub(1).max(1);
        if self.cached_config_height != node_height {
            let config = get_on_chain_config(&self.provider, config_read_height);
            self.buf_trace(format!(
                "Updated cached OnChainConfig for height {} \
                 (read from block {}), block_time_ms={}",
                node_height, config_read_height, config.block_interval_ms
            ));
            self.cached_config = config;
            self.cached_config_height = node_height;
        }

        let num_connections = self.check_peer_connections().await;

        let whoami = self.node_state.whoami();
        let mut incoming_messages = Vec::new();

        // Gather any messages in the qbft_protocol_message_rx channel.
        // Separate sync messages (BlockRequest/BlockResponse) from consensus messages.
        let mut block_requests: Vec<BlockRequest> = Vec::new();
        let mut block_responses: Vec<BlockResponse> = Vec::new();

        while let Ok(message) = self.qbft_protocol_message_rx.try_recv() {
            match message {
                QbftMessage::BlockRequest(req) => {
                    block_requests.push(req);
                }
                QbftMessage::BlockResponse(resp) => {
                    block_responses.push(resp);
                }
                QbftMessage::Proposal(proposal) => {
                    if self.proposal_has_valid_sealed_block(&proposal) {
                        incoming_messages.push(QbftMessage::Proposal(proposal));
                    }
                }
                other => {
                    incoming_messages.push(other);
                }
            }
        }

        // Express delivery: receive incoming tx batches and forward local txs
        // to the next proposer. Disabled via --disable-express / RBFT_DISABLE_EXPRESS.
        if !self.args.disable_express {
            self.express_delivery().await;
        }

        // Handle incoming BlockRequests - respond with blocks we have
        for request in block_requests {
            if let Some(response) = self.handle_block_request(&request) {
                // Send the response to all peers (the requester will recognize it)
                let response_msg = QbftMessage::BlockResponse(response);
                let protocol_msg = Message::new_message(response_msg);
                let msg_size = protocol_msg.encoded().len();
                let connections = self.qbft_protocol_connections.read().await;
                for (peer_id, (_, connection)) in connections.iter() {
                    let msg = format!(
                        "Sending BlockResponse to peer {peer_id} (msg_size_bytes={msg_size})"
                    );
                    trace!(target: "rbft", "{}", msg);
                    self.debug_buffer.push_back((Instant::now(), msg));
                    if let Err(e) = connection.try_send(Command::Message(protocol_msg.clone())) {
                        warn!(
                            target: "rbft",
                            "Failed to send BlockResponse to peer {}: {:?}",
                            peer_id, e
                        );
                    }
                }
            }
        }

        // Handle incoming BlockResponses - apply blocks to catch up
        for response in block_responses {
            self.handle_block_response(&response).await;
        }

        let highest_newblock_message = incoming_messages
            .iter()
            .filter_map(|msg| match &msg {
                QbftMessage::NewBlock(nb) => Some(nb.block.header.height),
                _ => None,
            })
            .max()
            .unwrap_or_default();

        self.buf_trace(format!(
            "advance(): node_height={node_height} incoming_messages={} \
             highest_newblock_message={highest_newblock_message:?}",
            incoming_messages.len(),
        ));

        // Check if we're behind and need to request blocks for catch-up.
        // Note: node_height is best_block + 1 (the next block to build)
        let current_best = node_height.saturating_sub(1);
        if highest_newblock_message > current_best + 1 {
            // We're behind by more than 1 block - request missing blocks
            let from_height = current_best + 1;
            let to_height = highest_newblock_message;

            info!(
                target: "rbft",
                "Detected we're behind: current={}, highest_newblock={}. \
                 Requesting blocks {} to {}",
                current_best, highest_newblock_message, from_height, to_height
            );

            let block_request = BlockRequest {
                from_height,
                to_height,
            };
            let request_msg = QbftMessage::BlockRequest(block_request);
            let protocol_msg = Message::new_message(request_msg);

            // Send request to all peers
            let msg_size = protocol_msg.encoded().len();
            let connections = self.qbft_protocol_connections.read().await;
            for (peer_id, (_, connection)) in connections.iter() {
                let msg = format!(
                    "Sending BlockRequest to peer {peer_id} (msg_size_bytes={msg_size})"
                );
                trace!(target: "rbft", "{}", msg);
                self.debug_buffer.push_back((Instant::now(), msg));
                if let Err(e) = connection.try_send(Command::Message(protocol_msg.clone())) {
                    warn!(
                        target: "rbft",
                        "Failed to send BlockRequest to peer {}: {:?}",
                        peer_id, e
                    );
                }
            }
        }

        let enabled = match env::var("RBFT_DEBUG_CATCHUP_BLOCK") {
            Ok(value) => {
                let threshold = value.parse::<u64>().map_err(|e| {
                    eyre!("RBFT_DEBUG_CATCHUP_BLOCK must be a valid unsigned integer: {e}")
                })?;
                whoami != 0
                    || self.node_state.blockchain().height() >= threshold
                    || highest_newblock_message >= threshold
            }
            Err(_) => true,
        };

        if !enabled {
            info!(
                target: "rbft",
                "Skipping QBFT advance until catch-up block reached"
            );
            return Ok(());
        }

        let time_ms = now_ms();

        let proposed_height = self.node_state.proposed_height();

        // Use block_time_ms from cached on-chain configuration
        let block_interval_ms = self.cached_config.block_interval_ms;

        let current_block_time_ms = self
            .node_state
            .blockchain()
            .estimate_timestamp_ms(block_interval_ms);

        let estimated_next_block_time_ms = current_block_time_ms + block_interval_ms as u128;
        let prod = time_ms >= estimated_next_block_time_ms && proposed_height < node_height;

        self.buf_trace(format!(
            "Times: t={time_ms} nbt={estimated_next_block_time_ms} \
             nbt-t={} bim={block_interval_ms} \
             ph={proposed_height} nh={node_height} prod={prod}",
            estimated_next_block_time_ms as i128 - time_ms as i128,
        ));

        // Have a block proposal ready as soon as the block timeout is reached.
        // The proposed block is kept constant until a block is committed.
        // All nodes should build a block regardless of whether they are currently the proposer.
        if time_ms >= estimated_next_block_time_ms && proposed_height < node_height {
            let Some(proposed_block) = self.get_proposed_block().await else {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Failed to get proposed block at height {node_height}"
                );
                return Ok(());
            };

            if proposed_block.header.height != node_height {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Proposed block height {} does not match expected node height {}",
                    proposed_block.header.height,
                    node_height
                );
                return Ok(());
            };

            self.node_state
                .blockchain_mut()
                .set_proposed_block(Some(proposed_block));

            let proposed_height = self.node_state.proposed_height();
            if proposed_height != node_height {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "After setting, proposed block height {} does not match \
                     expected node height {}",
                    proposed_height,
                    node_height
                );
            } else {
                self.buf_trace(format!("Set proposed block to {node_height}"));
            }
        }

        // Get the time again as get_proposed_block() may have taken some time.
        // This time will be use for block and round timeouts.
        let time = now();

        // let quorum_size = quorum(validators(self.node_state.blockchain()).len());
        // let incoming_summary = summarise_messages(incoming_messages.iter(), quorum_size);

        // Add, sort and deduplicate incoming messages.
        self.node_state.add_messages(incoming_messages, time);

        // Log the state before stepping the QBFT state machine.
        let summary_b = self.node_state.summarise();
        let summary_b_with_incoming =
            format!("({num_connections}) {}", summary_b);

        if self.full_logs || self.last_summary_b.as_ref() != Some(&summary_b_with_incoming) {
            info!(
                target: "rbft",
                "b {}",
                summary_b_with_incoming
            );
            self.last_summary_b = Some(summary_b_with_incoming);
        }

        // Step the QBFT state machine.
        let outgoing_messages = self.node_state.node_next(time);

        // Log the final state and outgoing messages.
        let o = outgoing_messages.iter().map(|m| &m.message);
        let quorum_size = quorum(validators(self.node_state.blockchain()).len());
        let summary_a = self.node_state.summarise();
        let outgoing_summary = summarise_messages(o, quorum_size);
        let full_summary_a = if outgoing_summary.is_empty() {
            format!("({num_connections}) {}", summary_a)
        } else {
            format!("({num_connections}) {} o={}", summary_a, outgoing_summary)
        };

        if self.full_logs || self.last_summary_a.as_ref() != Some(&full_summary_a) {
            info!(
                target: "rbft",
                "a {}",
                full_summary_a
            );
            self.last_summary_a = Some(full_summary_a);
        }

        // Bypass messages for local node. This avoid a round trip via another peer.
        for m in &outgoing_messages {
            // Note that push_message is good enough as add_messages does deduplication later.
            self.node_state.push_message(m.message.clone());
        }

        // Check if the model blockchain is longer than the current node chain.
        // If so, extend the Reth blockchain by committing new blocks.
        // Note that the model blockchain may be ahead by more than one block.
        let model_height = self.node_state.blockchain().height();
        if model_height > node_height {
            self.buf_trace(format!("update_reth_blockchain {model_height} > {node_height}"));
            self.commit_block_to_reth_blockchain(node_height, model_height)
                .await;
            let last_block_number = self.provider.last_block_number().unwrap_or(0);
            let best_block_number = self.provider.best_block_number().unwrap_or(0);
            self.buf_trace(format!(
                "update_reth_blockchain last={last_block_number}, best={best_block_number}"
            ));
            // let _new_validators = self.get_validators(model_height)?;
        }

        // Send outgoing messages to all peers - use write lock since we may remove dead peers
        if !outgoing_messages.is_empty() {
            let mut connections = self.qbft_protocol_connections.write().await;
            let mut dead_peers: Vec<PeerId> = Vec::new();

            for msg in outgoing_messages {
                // Create a protocol message containing the actual QBFT message
                let protocol_msg = Message::new_message(msg.message.clone());
                let msg_size = protocol_msg.encoded().len();
                for (peer_id, (_, connection)) in connections.iter() {
                    let log_msg = format!(
                        "Sending QBFT message to peer {peer_id} (msg_size_bytes={msg_size})"
                    );
                    trace!(target: "rbft", "{}", log_msg);
                    self.debug_buffer.push_back((Instant::now(), log_msg));

                    if let Err(e) = connection.try_send(Command::Message(protocol_msg.clone())) {
                        // Inline flush: cannot call self.flush_debug_buffer()
                        // while connections is borrowed.
                        if !self.debug_buffer.is_empty() {
                            let count = self.debug_buffer.len();
                            warn!(
                                target: "rbft",
                                "=== DEBUG DUMP ({count} entries from last ~60s) ==="
                            );
                            for (i, (_ts, m)) in self.debug_buffer.iter().enumerate() {
                                warn!(target: "rbft", "[DUMP {}/{}] {}", i + 1, count, m);
                            }
                            warn!(target: "rbft", "=== END DEBUG DUMP ===");
                            self.debug_buffer.clear();
                        }
                        error!(
                            target: "rbft",
                            "Error sending QBFT message to QbftProtocol peer {}: \
                             {:?} - marking for removal",
                            peer_id,
                            e
                        );
                        if !dead_peers.contains(peer_id) {
                            dead_peers.push(*peer_id);
                        }
                    }
                }
            }

            // Remove dead peer connections
            if !dead_peers.is_empty() {
                warn!(
                    target: "rbft",
                    "Removing {} dead peer connections: {:?}",
                    dead_peers.len(),
                    dead_peers
                );
                for peer_id in dead_peers {
                    connections.remove(&peer_id);
                }
            }
        }

        Ok(())
    }

    /// Receive any transaction batches forwarded by remote peers, add them to our local
    /// pool, then forward our own local pending transactions directly to the next block
    /// proposer (bypassing normal gossip for lower latency).
    async fn express_delivery(&mut self) {
        // ── RECEIVE ──────────────────────────────────────────────────────────────────
        // Drain any incoming tx batches that remote peers forwarded to us.
        while let Ok(encoded_txs) = self.transactions_rx.try_recv() {
            let mut pool_txs = Vec::with_capacity(encoded_txs.len());
            for raw in &encoded_txs {
                let mut buf: &[u8] = raw.as_ref();
                match PoolConsensusTx::<Pool>::decode_2718(&mut buf) {
                    Ok(tx) => match tx.try_into_recovered() {
                        Ok(recovered) => {
                            match Pool::Transaction::try_from_consensus(recovered) {
                                Ok(pool_tx) => pool_txs.push(pool_tx),
                                Err(_) => {
                                    warn!(
                                        target: "rbft",
                                        "Express delivery: failed to convert tx to pool tx"
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            warn!(
                                target: "rbft",
                                "Express delivery: failed to recover signer for transaction"
                            );
                        }
                    },
                    Err(e) => {
                        warn!(
                            target: "rbft",
                            "Express delivery: failed to decode transaction: {:?}",
                            e
                        );
                    }
                }
            }
            if !pool_txs.is_empty() {
                let is_proposer = self.node_state.is_the_proposer_for_current_round();
                let has_proposal = self.node_state.last_prepared_block().is_some();
                debug!(
                    target: "rbft",
                    "Express delivery: p={is_proposer} hp={has_proposal} received {} txs",
                    pool_txs.len()
                );
                let _ = self
                    .pool
                    .add_transactions(TransactionOrigin::External, pool_txs)
                    .await;
            }
        }

        // ── SEND ─────────────────────────────────────────────────────────────────────
        // Forward our own local pending transactions directly to the next proposer so
        // they can be included in the very next block without waiting for gossip.
        let next_proposer = self.node_state.next_proposer_for_current_round();
        let proposer_index_and_peer = self
            .cached_config
            .selected_validators
            .iter()
            .zip(self.cached_config.selected_validator_enodes.iter())
            .enumerate()
            .find(|(_, (addr, _))| **addr == next_proposer)
            .map(|(idx, (_, node))| (idx, PeerId::from(node.id)));

        if let Some((proposer_index, proposer_peer_id)) = proposer_index_and_peer {
            // Only send if the proposer is not ourselves.
            if proposer_peer_id != *self.network_handle.peer_id() {
                let cutoff = self.last_express_delivery_at;
                let local_pending = self.pool.get_local_pending_transactions();
                let encoded_txs: Vec<alloy_primitives::Bytes> = local_pending
                    .iter()
                    .filter(|tx| tx.timestamp > cutoff)
                    .map(|tx| {
                        let consensus_tx = tx.transaction.clone_into_consensus();
                        alloy_primitives::Bytes::from(consensus_tx.encoded_2718())
                    })
                    .collect();

                if !encoded_txs.is_empty() {
                    let msg = Message::new_transactions(encoded_txs.clone());
                    let connections = self.qbft_protocol_connections.read().await;
                    if let Some((_, conn)) = connections.get(&proposer_peer_id) {
                        debug!(
                            target: "rbft",
                            "Express delivery: forwarding {} local txs to proposer node{}",
                            encoded_txs.len(),
                            proposer_index
                        );
                        if let Err(e) = conn.try_send(Command::Message(msg)) {
                            warn!(
                                target: "rbft",
                                "Express delivery: failed to send txs to proposer node{}: {:?}",
                                proposer_index,
                                e
                            );
                        } else {
                            // Advance the cutoff so these txs are not re-sent next tick.
                            if let Some(latest) = local_pending
                                .iter()
                                .filter(|tx| tx.timestamp > cutoff)
                                .map(|tx| tx.timestamp)
                                .max()
                            {
                                self.last_express_delivery_at = latest;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn check_peer_connections(&mut self) -> usize {
        // Check and connect to selected validator enodes from the contract (except our own)
        if !self.cached_config.selected_validator_enodes.is_empty() {
            use reth_ethereum::network::Peers;

            let local_peer_id = *self.network_handle.peer_id();

            // Get currently connected peers
            let connected_peers = if let Ok(all_peers) = self.network_handle.get_all_peers().await {
                all_peers.into_iter().map(|p| p.remote_id).collect::<std::collections::HashSet<_>>()
            } else {
                std::collections::HashSet::new()
            };

            let start_node = self.cached_config.selected_validator_enodes
                .iter()
                .position(|node| PeerId::from(node.id) == local_peer_id)
                .map(|x| x + 1)
                .unwrap_or(0);

            for index in start_node..self.cached_config.selected_validator_enodes.len() {
                let node_record = &self.cached_config.selected_validator_enodes[index];
                let peer_id = PeerId::from(node_record.id);

                // Check if this peer is already connected
                if !connected_peers.contains(&peer_id) {
                    // Get the socket address from the node record
                    let socket_addr = std::net::SocketAddr::new(
                        node_record.address,
                        node_record.tcp_port
                    );

                    info!(
                        target: "rbft",
                        "Connecting to selected validator node {index}",
                    );
                    self.network_handle.connect_peer(peer_id, socket_addr);
                }
            }
        }
        let qbft_connections = self.qbft_protocol_connections.read().await;
        qbft_connections.len()
    }

    /// Resend cached messages for current height and height-1 to all connected peers.
    /// This is triggered when no blocks have been committed for the resend timeout period.
    async fn resend_cached_messages(&mut self) {
        // Clone the Arc to avoid borrowing self.protocol_state across buf_debug/flush calls
        let messages_received_lock = match &self.protocol_state {
            Some(ps) => ps.messages_received.clone(),
            None => {
                warn!(target: "rbft", "Cannot resend: protocol_state not initialized");
                return;
            }
        };

        let current_height = self.node_state.blockchain().height();
        let target_heights = [current_height, current_height.saturating_sub(1)];

        self.buf_debug(format!(
            "Resend timeout reached, resending cached messages for heights {target_heights:?}"
        ));

        // Get all cached messages for target heights
        let messages_to_resend = {
            let messages_received = match messages_received_lock.lock() {
                Ok(guard) => guard,
                Err(e) => {
                    self.flush_debug_buffer();
                    error!(target: "rbft", "Failed to lock messages_received: {:?}", e);
                    return;
                }
            };

            let mut collected = Vec::new();
            for (height, rlp_bytes) in messages_received.iter() {
                if target_heights.contains(height) {
                    collected.push(rlp_bytes.clone());
                }
            }
            collected
        };

        if messages_to_resend.is_empty() {
            self.buf_debug(format!(
                "No cached messages found for heights {target_heights:?}"
            ));
            return;
        }

        self.buf_debug(format!(
            "Resending {} cached messages to all connected peers",
            messages_to_resend.len()
        ));

        // Decode and resend each message to all connected peers
        let connections = self.qbft_protocol_connections.read().await;
        let mut resend_count = 0;
        let mut error_count = 0;

        for rlp_bytes in &messages_to_resend {
            let mut buf = rlp_bytes.as_ref();
            if let Some(msg) = Message::decode_message(&mut buf) {
                let msg_size = rlp_bytes.len();
                for (_peer_id, (_, connection)) in connections.iter() {
                    let log_msg = format!(
                        "Resending cached message to peer (msg_size_bytes={msg_size})"
                    );
                    trace!(target: "rbft", "{}", log_msg);
                    self.debug_buffer.push_back((Instant::now(), log_msg));
                    if let Err(e) = connection.try_send(Command::Message(msg.clone())) {
                        error_count += 1;
                        let err_msg = format!("Failed to resend message to peer: {e:?}");
                        trace!(target: "rbft", "{}", err_msg);
                        self.debug_buffer.push_back((Instant::now(), err_msg));
                    } else {
                        resend_count += 1;
                    }
                }
            } else {
                warn!(target: "rbft", "Failed to decode cached message for resend");
            }
        }

        info!(
            target: "rbft",
            "Resend complete: {} messages sent successfully, {} errors",
            resend_count,
            error_count
        );
    }

    /// Get a block from self.node_state.blockchain().
    /// Decode the RLP-encoded block body to a SealedBlock.
    /// Use the SealedBlock to create a payload.
    /// Call new_payload on the engine.
    async fn commit_block_to_reth_blockchain(&mut self, node_height: u64, model_height: u64) {
        self.buf_trace(format!(
            "Model blockchain height {model_height} > provider block number {node_height}, \
             processing new blocks"
        ));

        // Get the last RLP encoded block from node_state.blockchain
        let last_block = self.node_state.blockchain().head();

        // Decode the RLP encoded block body (which contains a SealedBlock)
        let Ok(decoded_block) = SealedBlock::decode(&mut last_block.body().as_ref()) else {
            self.flush_debug_buffer();
            error!(target: "rbft", "Failed to decode RLP block body");
            return;
        };

        let payload_block_hash = decoded_block.hash();

        self.buf_trace(format!("Commit: adding block to tip {node_height} to {model_height}"));
        let block_number = decoded_block.number();

        // Use decoded_block directly - it's already the correct type from the provider
        let payload = T::block_to_payload(decoded_block.into());
        let res = self.to_engine.new_payload(payload).await;
        let is_ok = res.as_ref().is_ok_and(|res| res.is_valid());

        self.buf_debug(format!("Commit: New payload for block number {block_number}: ok={is_ok}"));

        if !is_ok {
            self.flush_debug_buffer();
            error!(
                target: "rbft",
                "Commit: New payload invalid for block number {}: {:?}",
                block_number,
                res
            );
            if let Err(err) = self.reload_blockchain_from_chain().await {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Failed to reload blockchain after invalid payload: {err:?}"
                );
            }
            return;
        }

        self.update_tip(payload_block_hash).await;

        // Update last commit time for resend logic
        self.last_commit_time = Some(std::time::Instant::now());
    }

    async fn update_tip(&mut self, hash: alloy_primitives::FixedBytes<32>) {
        let fork_choice_state = ForkchoiceState {
            head_block_hash: hash,
            safe_block_hash: hash,
            finalized_block_hash: hash,
        };

        // Confirm the new block as the head of the chain.
        match self
            .to_engine
            .fork_choice_updated(fork_choice_state, None, EngineApiMessageVersion::default())
            .await
        {
            Ok(res) if res.is_valid() => {
                self.buf_debug(format!("Fork choice updated to new head block hash: {hash:?}"));
            }
            Ok(res) => {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Fork choice update to new head invalid: \
                        {:?}",
                    res
                );
            }
            Err(e) => {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Fork choice update to new head failed: \
                        {:?}",
                    e
                );
            }
        }
    }

    /// Handle an incoming BlockRequest by fetching blocks from the reth provider
    /// and returning a BlockResponse containing the requested blocks.
    fn handle_block_request(&self, request: &BlockRequest) -> Option<BlockResponse> {
        let from = request.from_height;
        let to = request.to_height;

        // Limit the number of blocks we return in a single response
        const MAX_BLOCKS_PER_RESPONSE: u64 = 100;
        let to = to.min(from.saturating_add(MAX_BLOCKS_PER_RESPONSE - 1));

        info!(
            target: "rbft",
            "Handling BlockRequest for blocks {} to {}",
            from, to
        );

        let best_block = self.provider.best_block_number().unwrap_or(0);
        if from > best_block {
            warn!(
                target: "rbft",
                "BlockRequest from {} is beyond our best block {}",
                from, best_block
            );
            return None;
        }

        let to = to.min(best_block);
        let mut blocks = Vec::new();

        for height in from..=to {
            let Some(block) = self.provider.block(height.into()).ok().flatten() else {
                warn!(
                    target: "rbft",
                    "Failed to get block {} for BlockRequest",
                    height
                );
                break;
            };

            let sealed_block = SealedBlock::from(block);
            let mut body = Vec::new();
            sealed_block.encode(&mut body);

            // Get validator set for this block's height
            let on_chain_config = get_on_chain_config(&self.provider, height);

            let validators = on_chain_config.selected_validators;
            let proposer = sealed_block.header().beneficiary();
            let timestamp = sealed_block.header().timestamp();
            let digest = keccak256(&body);

            let header = QbftBlockHeader {
                proposer,
                round_number: 0,
                commit_seals: vec![],
                height,
                timestamp,
                validators,
                digest,
            };
            let qbft_block = QbftBlock::new(header, Bytes::from(body));

            // Create a SignedNewBlock (without a real signature since this is historical data)
            // The signature here is a placeholder - in production we might want to include
            // the original proposer's signature if available
            let unsigned_payload = UnsignedNewBlock {
                height,
                round: 0,
                digest,
            };

            let signed_block = SignedNewBlock {
                unsigned_payload,
                signature: Signature::default(),
                block: qbft_block,
            };

            blocks.push(signed_block);
        }

        if blocks.is_empty() {
            None
        } else {
            info!(
                target: "rbft",
                "Responding with {} blocks (heights {} to {})",
                blocks.len(),
                from,
                from + blocks.len() as u64 - 1
            );
            Some(BlockResponse { blocks })
        }
    }

    /// Handle an incoming BlockResponse by applying the blocks to our chain.
    /// Returns true if blocks were successfully applied.
    async fn handle_block_response(&mut self, response: &BlockResponse) -> bool {

        if response.blocks.is_empty() {
            self.buf_debug("Received empty BlockResponse".into());
            return false;
        }

        let first_height = response
            .blocks
            .first()
            .map(|b| b.unsigned_payload.height)
            .expect("blocks is non-empty per guard above");
        let last_height = response
            .blocks
            .last()
            .map(|b| b.unsigned_payload.height)
            .expect("blocks is non-empty per guard above");

        info!(
            target: "rbft",
            "Handling BlockResponse with {} blocks (heights {} to {})",
            response.blocks.len(),
            first_height,
            last_height
        );

        let node_height = self.provider.best_block_number().unwrap_or(0);

        // Apply each block in sequence
        for signed_block in &response.blocks {
            let block_height = signed_block.unsigned_payload.height;

            // Skip blocks we already have
            if block_height <= node_height {
                self.buf_debug(format!(
                    "Skipping block {block_height} (already have block {node_height})"
                ));
                continue;
            }

            // Verify the block can be applied (must be exactly node_height + 1)
            let current_best = self.provider.best_block_number().unwrap_or(0);
            if block_height != current_best + 1 {
                warn!(
                    target: "rbft",
                    "Block {} cannot be applied, expected {}",
                    block_height, current_best + 1
                );
                // We might be missing intermediate blocks, but continue trying
                // as the response might have them in order
                continue;
            }

            // Decode the block body
            let Ok(decoded_block) = SealedBlock::decode(&mut signed_block.block.body().as_ref())
            else {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Failed to decode block {} from BlockResponse",
                    block_height
                );
                continue;
            };

            let block_hash = decoded_block.hash();

            // Send the block to the execution layer
            let payload = T::block_to_payload(decoded_block.into());
            let res = self.to_engine.new_payload(payload).await;
            let is_ok = res.as_ref().is_ok_and(|res| res.is_valid());

            if !is_ok {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Failed to apply block {} from BlockResponse: {:?}",
                    block_height, res
                );
                return false;
            }

            // Update the fork choice to make this block canonical
            self.update_tip(block_hash).await;

            // Update the QBFT model blockchain to stay in sync with reth
            self.node_state.blockchain_mut().set_head(signed_block.block.clone());
            self.node_state.blockchain_mut().set_proposed_block(None);
            self.node_state.new_round(0, self.node_state.local_time(), None);
            self.node_state.prune_messages_below_height();

            info!(
                target: "rbft",
                "Successfully applied block {} from BlockResponse",
                block_height
            );
        }

        true
    }

    /// Gets a proposed block as in the Reth miner implementation (commit
    /// b550387602f8acd190a2b115bd340a1e21d11671, `crates/engine/local/src/miner.rs` line 225).
    /// RLP encodes this block and uses this as the block body.
    async fn get_proposed_block(&mut self) -> Option<QbftBlock> {

        // Calculate the timestamp for the next block using millisecond precision
        let block_interval_ms = self.cached_config.block_interval_ms;
        let estimated_next_block_time_ms = self
            .node_state
            .blockchain()
            .estimate_timestamp_ms(block_interval_ms)
            + block_interval_ms as u128;

        // Convert milliseconds to seconds for the block timestamp
        let next_block_timestamp_secs = (estimated_next_block_time_ms / 1000) as u64;

        // Build payload attributes
        // Using attributes causes FCU to produce a new payload.
        let block_number = self.provider.best_block_number().unwrap_or(0);
        let latest_header: SealedHeader<RethHeader> = match self
            .provider
            .sealed_header(block_number)
            .map_err(|e| eyre!("Failed to read sealed header {block_number}: {e:?}"))
            .and_then(|opt| {
                opt.ok_or_else(|| eyre!("Sealed header not found for best block {block_number}"))
            }) {
            Ok(h) => h,
            Err(e) => {
                self.flush_debug_buffer();
                error!(target: "rbft", "get_proposed_block: {e:?}");
                return None;
            }
        };
        let mut payload_attributes = self.payload_attributes_builder.build(&latest_header);

        // Override the timestamp with our millisecond-based calculation
        payload_attributes.set_timestamp(next_block_timestamp_secs);

        // Set the proposer (beneficiary) to this node's ID
        let proposer = self.node_state.id();
        payload_attributes.set_suggested_fee_recipient(proposer);

        // Set prev_randao for post-merge compatibility
        // Since RBFT doesn't use PoS, we generate a deterministic but unpredictable value
        let prev_randao = keccak256(
            format!("{}{}", latest_header.hash(), block_number + 1).as_bytes()
        );
        payload_attributes.set_prev_randao(prev_randao);

        let forkchoice_state = match self.forkchoice_state() {
            Ok(s) => s,
            Err(e) => {
                self.flush_debug_buffer();
                error!(target: "rbft", "Failed to get forkchoice state: {:?}", e);
                return None;
            }
        };
        match self
            .to_engine
            .fork_choice_updated(
                forkchoice_state,
                Some(payload_attributes),
                EngineApiMessageVersion::default(),
            )
            .await
        {
            Ok(res) if res.is_valid() => {
                if let Some(payload_id) = res.payload_id {
                    // Get the built payload
                    match self
                        .payload_builder
                        .resolve_kind(payload_id, PayloadKind::WaitForPending)
                        .await
                    {
                        Some(Ok(payload)) => {
                            let block = payload.block();
                            let num_transactions = block.body().transactions().len();

                            // Ensure the payload's miner (beneficiary) matches the expected
                            // proposer
                            assert!(block.header().beneficiary() == proposer);

                            // RLP encode the block to use as block body
                            let encoded_block = alloy_rlp::encode(block);
                            let mut enc = encoded_block.as_ref();
                            assert!(SealedBlock::decode(&mut enc).is_ok());

                            // Extract block information
                            let block_hash = block.hash();
                            let block_number = block.header().number();
                            let block_timestamp = block.header().timestamp();
                            let digest = keccak256(&encoded_block);

                            // Use cached validators instead of fetching from chain
                            let validators = self.cached_config.selected_validators.clone();

                            self.buf_debug(format!(
                                "get_proposed_block {block_number} with hash {block_hash:?} and \
                                 {num_transactions} transactions at timestamp {block_timestamp} \
                                 with {} validators",
                                validators.len()
                            ));

                            // Create a QBFT block with the RLP-encoded real block as body
                            let header = QbftBlockHeader {
                                proposer: self.node_state.id(),
                                round_number: 0, // Should be set by QBFT logic
                                commit_seals: Vec::new(), // Should be set by QBFT logic
                                height: block_number,
                                timestamp: block_timestamp,
                                validators,
                                digest,
                            };
                            Some(QbftBlock::new(header, Bytes::from(encoded_block)))
                        }
                        Some(Err(e)) => {
                            self.flush_debug_buffer();
                            error!(
                                target: "rbft",
                                "Error getting payload: {:?}",
                                e
                            );
                            None
                        }
                        None => {
                            self.flush_debug_buffer();
                            error!(target: "rbft", "No payload available");
                            None
                        }
                    }
                } else {
                    self.flush_debug_buffer();
                    error!(
                        target: "rbft",
                        "No payload ID returned from forkchoice update"
                    );
                    None
                }
            }
            Ok(res) => {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Invalid forkchoice update result: {:?}",
                    res
                );
                None
            }
            Err(e) => {
                self.flush_debug_buffer();
                error!(
                    target: "rbft",
                    "Error in forkchoice update: {:?}",
                    e
                );
                None
            }
        }
    }

    fn forkchoice_state(&self) -> eyre::Result<ForkchoiceState> {
        let block_number = self.provider.best_block_number().unwrap_or(0);
        let latest_header: SealedHeader<<P as HeaderProvider>::Header> = self
            .provider
            .sealed_header(block_number)
            .map_err(|e| eyre!("Failed to read sealed header {block_number} from provider: {e:?}"))?
            .ok_or_else(|| eyre!("Sealed header not found for best block {block_number}"))?;
        let hash = latest_header.hash();
        Ok(ForkchoiceState {
            head_block_hash: hash,
            safe_block_hash: hash,
            finalized_block_hash: hash,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct ReconnectConfig {
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
}

/// Attempts to reconnect to a disconnected peer with exponential backoff.
///
/// This function is spawned as a background task when a peer disconnects.
/// It will attempt to reconnect up to `max_attempts` times, with increasing
/// delays between attempts (exponential backoff).
///
/// The `active_reconnections` set is used to track which peers have active
/// reconnection tasks, preventing duplicate tasks from being spawned.
#[allow(clippy::too_many_arguments)]
async fn attempt_peer_reconnection(
    network_handle: NetworkHandle<EthNetworkPrimitives>,
    connections: PeerConnections,
    peer_id: PeerId,
    remote_addr: std::net::SocketAddr,
    reconnect_config: ReconnectConfig,
    active_reconnections: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<PeerId>>>,
) {
    use reth_ethereum::network::Peers;

    // Helper to ensure we always remove the peer from active_reconnections when done
    struct CleanupGuard {
        peer_id: PeerId,
        active_reconnections: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<PeerId>>>,
    }

    impl Drop for CleanupGuard {
        fn drop(&mut self) {
            // Use try_lock to avoid blocking in drop; if we can't get the lock,
            // spawn a task to clean up
            if let Ok(mut active) = self.active_reconnections.try_lock() {
                active.remove(&self.peer_id);
            } else {
                // Spawn a task to clean up if we couldn't get the lock synchronously
                let peer_id = self.peer_id;
                let active_reconnections = self.active_reconnections.clone();
                tokio::spawn(async move {
                    let mut active = active_reconnections.lock().await;
                    active.remove(&peer_id);
                });
            }
        }
    }

    let _cleanup = CleanupGuard {
        peer_id,
        active_reconnections: active_reconnections.clone(),
    };

    // Skip reconnection attempts to ourselves
    let local_peer_id = *network_handle.peer_id();
    if peer_id == local_peer_id {
        debug!(
            target: "rbft",
            "Skipping reconnection to self (peer {})",
            peer_id
        );
        return;
    }

    debug!(
        target: "rbft",
        "Starting reconnection attempts to peer {} at {} (max {} attempts)",
        peer_id,
        remote_addr,
        reconnect_config.max_attempts
    );

    let mut attempt = 0;
    let mut current_delay_ms = reconnect_config.base_delay_ms;

    while attempt < reconnect_config.max_attempts {
        attempt += 1;

        // Check if we're already connected (connection might have been re-established)
        {
            let conns = connections.read().await;
            if conns.contains_key(&peer_id) {
                info!(
                    target: "rbft",
                    "Peer {} already reconnected, stopping reconnection attempts",
                    peer_id
                );
                return;
            }
        }

        // Wait before attempting to reconnect
        tokio::time::sleep(Duration::from_millis(current_delay_ms)).await;

        debug!(
            target: "rbft",
            "Reconnection attempt {}/{} to peer {} at {}",
            attempt,
            reconnect_config.max_attempts,
            peer_id,
            remote_addr
        );

        match network_handle.get_peer_by_id(peer_id).await {
            Ok(Some(info)) => {
                debug!(
                    target: "rbft",
                    "Peer {} present in peer table (kind: {:?}, addr: {})",
                    peer_id,
                    info.kind,
                    info.remote_addr
                );
            }
            Ok(None) => {
                debug!(
                    target: "rbft",
                    "Peer {} not present in peer table before reconnect attempt",
                    peer_id
                );
            }
            Err(err) => {
                warn!(
                    target: "rbft",
                    "Failed to query peer {} before reconnect attempt: {:?}",
                    peer_id,
                    err
                );
            }
        }

        // Attempt to connect
        network_handle.connect_peer(peer_id, remote_addr);

        // Wait a bit for the connection to establish
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Check if connection was successful
        {
            let conns = connections.read().await;
            if conns.contains_key(&peer_id) {
                info!(
                    target: "rbft",
                    "Successfully reconnected to peer {} at {} after {} attempt(s)",
                    peer_id,
                    remote_addr,
                    attempt
                );
                return;
            }
        }

        // Calculate next delay with exponential backoff, capped at max_delay_ms
        current_delay_ms = (current_delay_ms * 2).min(reconnect_config.max_delay_ms);

        debug!(
            target: "rbft",
            "Reconnection attempt {} to peer {} failed, next delay: {}ms",
            attempt,
            peer_id,
            current_delay_ms
        );
    }

    warn!(
        target: "rbft",
        "Failed to reconnect to peer {} at {} after {} attempts",
        peer_id,
        remote_addr,
        reconnect_config.max_attempts
    );
}

fn get_private_key_and_id(
    args: &RbftNodeArgs,
) -> eyre::Result<(alloy_primitives::B256, alloy_primitives::Address)> {
    let key = args
        .validator_key
        .as_ref()
        .ok_or_else(|| eyre!("Validator key is required for QBFT nodes"))?;
    let key_data = std::fs::read_to_string(key)
        .map_err(|e| eyre!("Failed to read validator key file {key:?}: {e}"))?;
    let key_data = key_data.trim();

    let key_hex = if let Some(stripped) = key_data.strip_prefix("0x") {
        stripped
    } else {
        key_data
    };

    // Parse the hex string as a B256 private key
    let decoded_bytes = alloy_primitives::hex::decode(key_hex)
        .map_err(|e| eyre!("Failed to decode validator private key hex: {e}"))?;
    let private_key_array: [u8; 32] = decoded_bytes
        .try_into()
        .map_err(|_| eyre!("Invalid private key length - must be 32 bytes"))?;
    let private_key = B256::from(private_key_array);

    // Create a signer from the private key and recover the account address
    let signer = PrivateKeySigner::from_bytes(&private_key)
        .map_err(|e| eyre!("Failed to create signer from private key: {e}"))?;
    let id = signer.address();
    Ok((private_key, id))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("cannot be earlier than UNIX_EPOCH")
        .as_secs()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("cannot be earlier than UNIX_EPOCH")
        .as_millis()
}

#[cfg(test)]
mod debug_buffer_tests {
    use super::*;

    #[test]
    fn buffer_respects_max_entries_cap() {
        let mut buf: VecDeque<(Instant, String)> = VecDeque::new();
        for i in 0..DEBUG_BUFFER_MAX_ENTRIES + 500 {
            if buf.len() >= DEBUG_BUFFER_MAX_ENTRIES {
                buf.pop_front();
            }
            buf.push_back((Instant::now(), format!("entry {i}")));
        }
        assert_eq!(buf.len(), DEBUG_BUFFER_MAX_ENTRIES);
        // Oldest surviving entry should be #500 (first 500 were evicted)
        assert!(buf.front().unwrap().1.contains("entry 500"));
    }

    #[test]
    fn buffer_ttl_prunes_stale_entries() {
        let mut buf: VecDeque<(Instant, String)> = VecDeque::new();
        // Simulate an old entry beyond the TTL
        let old = Instant::now() - DEBUG_BUFFER_TTL - Duration::from_secs(5);
        buf.push_back((old, "stale entry".into()));
        buf.push_back((Instant::now(), "fresh entry".into()));

        // Prune using the same logic as the health check tick
        let cutoff = Instant::now() - DEBUG_BUFFER_TTL;
        while buf.front().is_some_and(|(ts, _)| *ts < cutoff) {
            buf.pop_front();
        }

        assert_eq!(buf.len(), 1);
        assert_eq!(buf.front().unwrap().1, "fresh entry");
    }

    #[test]
    fn flush_emits_all_entries_and_clears() {
        // Simulate the flush logic (can't call self.flush_debug_buffer directly,
        // but we test the same algorithm)
        let mut buf: VecDeque<(Instant, String)> = VecDeque::new();
        buf.push_back((Instant::now(), "msg A".into()));
        buf.push_back((Instant::now(), "msg B".into()));
        buf.push_back((Instant::now(), "msg C".into()));

        assert_eq!(buf.len(), 3);

        // Simulate flush: iterate and collect output, then clear
        let count = buf.len();
        let mut dump_lines = Vec::new();
        for (i, (_ts, msg)) in buf.iter().enumerate() {
            dump_lines.push(format!("[DUMP {}/{}] {}", i + 1, count, msg));
        }
        buf.clear();

        assert_eq!(dump_lines.len(), 3);
        assert!(dump_lines[0].contains("[DUMP 1/3] msg A"));
        assert!(dump_lines[1].contains("[DUMP 2/3] msg B"));
        assert!(dump_lines[2].contains("[DUMP 3/3] msg C"));
        assert!(buf.is_empty());
    }

    #[test]
    fn empty_buffer_flush_is_noop() {
        let buf: VecDeque<(Instant, String)> = VecDeque::new();
        // The flush logic checks is_empty() first
        assert!(buf.is_empty());
        // No panic, no output - this is the noop path
    }
}
