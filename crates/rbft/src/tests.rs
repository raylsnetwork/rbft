// SPDX-License-Identifier: Apache-2.0
//! This is a test to ensure that the Rust code matches the Dafny spec.
use alloy_primitives::{Bytes, B256};
use tracing::info;

use crate::node_auxilliary_functions::{
    digest, f, has_received_proposal_justification, hash_block_for_commit_seal, proposer, quorum,
    round_timeout, sign_prepare, sign_proposal, sign_round_change,
};
use crate::types::qbft_message::msg_error_code;
use crate::types::{
    qbft_message::summarise_messages, Address, Block, BlockHeader, Blockchain, Commit,
    Configuration, Hash, NodeState, Prepare, Proposal, QbftMessage, RoundChange, RoundChangeConfig,
    Signature, SignedCommit, SignedNewBlock, SignedPrepare, SignedProposal, SignedRoundChange,
    UnsignedCommit, UnsignedNewBlock, UnsignedPrepare, UnsignedProposal, UnsignedRoundChange,
};

use std::{collections::VecDeque, fmt, num::ParseIntError, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TestTime {
    time: u64,
    tick: u64,
}

impl TestTime {
    fn new(time: u64, tick: u64) -> Self {
        Self { time, tick }
    }

    fn add(&self, other: &TestTime) -> TestTime {
        let mut new_time = self.time + other.time;
        let mut new_tick = self.tick + other.tick;
        if new_tick >= NodeSwarm::TICKS_PER_SECOND {
            new_time += new_tick / NodeSwarm::TICKS_PER_SECOND;
            new_tick %= NodeSwarm::TICKS_PER_SECOND;
        }
        TestTime {
            time: new_time,
            tick: new_tick,
        }
    }
}

/// A delayed message to be delivered to a specific node at a specific time.
struct DelayedMessage {
    message: QbftMessage,
    node: usize,
    deliver_at: TestTime,
    is_new: bool,
}

/// A test node swarm with N nodes.
///
/// This struct is used to simulate a network of nodes for testing purposes.
///
/// Outgoing messages are broadcast to all nodes.
struct NodeSwarm {
    nodes: Vec<NodeState>,
    validators: Vec<Address>,
    private_keys: Vec<B256>,
    enabled: Vec<bool>,
    messages_in_transit: Vec<DelayedMessage>,
}

impl NodeSwarm {
    const SECONDS_PER_BLOCK: u64 = 2;
    const TICKS_PER_SECOND: u64 = 10;

    pub fn new(num_nodes: usize) -> Self {
        info!(
            target: "swarm",
            "Creating a swarm with {num_nodes} nodes quorum={} f={}",
            quorum(num_nodes),
            f(num_nodes)
        );
        let private_keys: Vec<B256> = (0..num_nodes)
            .map(|j| B256::from([(j + 1) as u8; 32]))
            .collect();

        let validators: Vec<Address> = private_keys
            .iter()
            .map(|private_key| {
                use alloy_signer_local::PrivateKeySigner;
                let signer =
                    PrivateKeySigner::from_bytes(private_key).expect("Private key should be valid");
                signer.address()
            })
            .collect();

        let nodes = validators
            .iter()
            .enumerate()
            .map(|(i, &id)| {
                // Block zero proposer is not tested.
                let proposer = Address::ZERO;

                // Create genesis block with the test node as proposer
                let header = BlockHeader {
                    proposer,
                    round_number: 0,
                    commit_seals: vec![],
                    height: 0,
                    timestamp: 0,
                    validators: validators.clone(),
                    digest: Default::default(),
                };
                let genesis_block = Block::new(header, Default::default());

                let configuration = Configuration {
                    nodes: validators.clone(),
                    genesis_block: genesis_block.clone(),
                    block_time: Self::SECONDS_PER_BLOCK,
                    round_change_config: RoundChangeConfig {
                        start_time: 0.0,
                        first_interval: 1.0,
                        growth_factor: 2.0,
                        max_round: 10,
                        round_change_on_first_block: true,
                    },
                };

                let blockchain = Blockchain::new(VecDeque::from([genesis_block]));

                NodeState::new(blockchain, configuration, id, private_keys[i], 0)
            })
            .collect();
        Self {
            nodes,
            validators,
            private_keys,
            enabled: vec![true; num_nodes],
            messages_in_transit: vec![],
        }
    }

    /// Get one of the nodes in the swarm. Panics if index is out of bounds.
    #[allow(dead_code)]
    fn node(&self, index: usize) -> &NodeState {
        &self.nodes[index]
    }

    /// Get an immutable list of nodes.
    #[allow(dead_code)]
    fn nodes(&self) -> &[NodeState] {
        &self.nodes
    }

    /// Get a mutable list of nodes.
    #[allow(dead_code)]
    fn nodes_mut(&mut self) -> &mut [NodeState] {
        &mut self.nodes
    }

    /// Get the deterministic private keys backing this swarm.
    fn private_keys(&self) -> &[B256] {
        &self.private_keys
    }

    /// Produce a summary string for the swarm at the given local time.
    #[allow(dead_code)]
    pub fn summarise(&mut self, local_time: u64) -> String {
        for n in &mut self.nodes {
            n.set_local_time(local_time);
        }
        let s = self
            .nodes
            .iter()
            .map(|n| n.summarise())
            .collect::<Vec<String>>()
            .join("\n");
        format!("@{local_time}\n{s}")
    }

    /// Simulate a tick for all nodes in the swarm
    fn tick(&mut self, time: u64, tick: u64) {
        self.prepare_state(time);

        self.process_messages(time, tick);

        info!(target: "swarm", "--------------- @{time}.{tick} --------------");
        let mut outgoing = Vec::new();
        for (i, node) in self.nodes.iter_mut().enumerate() {
            if self.enabled[i] {
                let before = node.summarise();
                info!(target: "swarm", "validator {i}");
                info!(target: "swarm", "{before}");
                let out = node.node_next(time);
                let after = node.summarise();
                if out.is_empty() {
                    info!(target: "swarm", "{after}");
                } else {
                    let quorum_size = quorum(self.validators.len());
                    let o = summarise_messages(out.iter().map(|m| &m.message), quorum_size);
                    info!(target: "swarm", "{after} -> {o}");
                }
                outgoing.extend(out);
            }
        }

        // Broadcast to all interested nodes.
        let (deliver_at_time, deliver_at_tick) = Self::next_tick(time, tick);
        let deliver_at = TestTime::new(deliver_at_time, deliver_at_tick);
        for i in 0..self.nodes.len() {
            for msg in &outgoing {
                let delayed_message = DelayedMessage {
                    message: msg.message.clone(),
                    node: i,
                    deliver_at: deliver_at.clone(),
                    is_new: true,
                };
                self.messages_in_transit.push(delayed_message);
            }
        }
    }

    fn process_messages(&mut self, time: u64, tick: u64) {
        self.messages_in_transit
            .sort_by(|a, b| a.deliver_at.cmp(&b.deliver_at));
        let current_time = TestTime::new(time, tick);
        let first_current = self
            .messages_in_transit
            .partition_point(|m| m.deliver_at < current_time);
        self.messages_in_transit.drain(0..first_current);
        let first_future = self
            .messages_in_transit
            .partition_point(|m| m.deliver_at <= current_time);

        let mut incomming = (0..self.nodes.len())
            .map(|_| Vec::new())
            .collect::<Vec<Vec<QbftMessage>>>();
        for msg in self.messages_in_transit.drain(0..first_future) {
            incomming[msg.node].push(msg.message);
        }
        for (i, (node, incomming)) in self.nodes.iter_mut().zip(incomming.into_iter()).enumerate() {
            if self.enabled[i] {
                node.set_local_time(time);
                node.add_messages(incomming, time);
            }
        }
    }

    /// Temporaily disable a node in the swarm.
    fn disable_node(&mut self, index: usize) {
        self.enabled[index] = false;
    }

    /// Broadcast a crafted message so each validator receives it on the specified tick.
    fn broadcast_immediate(
        &mut self,
        message: QbftMessage,
        deliver_at_time: u64,
        deliver_at_tick: u64,
    ) {
        let deliver_at = TestTime::new(deliver_at_time, deliver_at_tick);
        for node in 0..self.nodes.len() {
            self.messages_in_transit.push(DelayedMessage {
                message: message.clone(),
                node,
                deliver_at: deliver_at.clone(),
                is_new: true,
            });
        }
    }

    /// Add a proposal to all nodes in the swarm.
    /// Creates a SignedProposal signed by the current proposer for the given round and height.
    /// This creates a Proposal message and broadcasts it immediately to all nodes.
    #[allow(dead_code)]
    pub fn add_proposal_for_round_0(&mut self) {
        let round = 0;

        // Determine the proposer for this round
        // Use the first node's blockchain to determine proposer (they should all be in sync)
        let proposer_node = self.proposer_node(0);
        let height = proposer_node.height();

        let proposed_block = proposer_node.get_new_block(round).unwrap();

        // Create the unsigned proposal
        let unsigned_proposal = UnsignedProposal {
            height,
            round,
            digest: digest(&proposed_block),
        };

        // Sign the proposal with the proposer's key
        let signed_proposal = sign_proposal(&unsigned_proposal, proposer_node);

        // Create the full proposal message
        let proposal = Proposal {
            proposal_payload: signed_proposal,
            proposed_block,
            proposal_justification: vec![],
            round_change_justification: vec![],
        };
        let message = QbftMessage::Proposal(proposal);

        for node in self.nodes_mut() {
            node.add_messages(vec![message.clone()], 0);
        }
    }

    /// Add prepare messages from all validators for a specific proposal
    /// This simulates validators preparing for a proposal they've received
    #[allow(dead_code)]
    fn add_prepares(&mut self) {
        let height = self.node(0).height();
        let round = self.node(0).round();
        let proposer_node = self.proposer_node(0);
        let proposed_block = proposer_node.get_new_block(round).unwrap();
        let digest = digest(&proposed_block);

        // Create unsigned prepare message
        let unsigned_prepare = UnsignedPrepare {
            height,
            round,
            digest,
        };

        let proposal = proposer_node
            .messages_received()
            .iter()
            .find_map(|msg| {
                if let QbftMessage::Proposal(proposal) = msg {
                    Some(proposal)
                } else {
                    None
                }
            })
            .expect("Proposer should have a proposal message")
            .clone();

        // Get the proposer for this round to exclude them (proposer doesn't send prepare)
        let mut prepare_messages = Vec::new();

        for validator_node in self.nodes_mut() {
            let message = QbftMessage::Prepare(Prepare {
                prepare_payload: sign_prepare(&unsigned_prepare, validator_node),
            });
            prepare_messages.push(message);
            validator_node.set_last_prepared_block_and_round(proposed_block.clone(), 0);
            validator_node.set_proposal_accepted_for_current_round(Some(proposal.clone()))
        }

        // Add all prepare messages to all nodes
        for message in prepare_messages {
            for node in self.nodes_mut() {
                node.add_messages(vec![message.clone()], 0);
            }
        }
    }

    fn proposer_node(&mut self, round: u64) -> &NodeState {
        let blockchain = &self.nodes[0].blockchain();
        let proposer_address = proposer(round, blockchain);

        // Find the proposer node in the swarm
        let proposer_node_index = self
            .validators
            .iter()
            .position(|&validator| validator == proposer_address)
            .expect("Proposer should be in the validator set");

        &self.nodes[proposer_node_index]
    }

    /// Re-enable a node in the swarm.
    #[allow(dead_code)]
    fn enable_node(&mut self, index: usize) {
        self.enabled[index] = true;
    }

    /// Get the minimum height among all nodes.
    fn min_height(&self) -> u64 {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(i, _n)| self.enabled[*i])
            .map(|(_, n)| n.height())
            .min()
            .unwrap_or(0)
    }

    /// Map over delayed messages in transit, allowing modification or removal.
    #[allow(dead_code)]
    fn map_delayed_messages(
        &mut self,
        mut f: impl FnMut(DelayedMessage) -> Option<DelayedMessage>,
    ) {
        let mut messages_in_transit = Vec::new();
        for msg in &mut self.messages_in_transit.drain(..) {
            if let Some(mut new_msg) = f(msg) {
                new_msg.is_new = false;
                messages_in_transit.push(new_msg);
            }
        }
        self.messages_in_transit = messages_in_transit;
    }

    /// Set the proposed block for the proposer node.
    /// Note that there may be several competing proposers if there are multiple nodes.
    fn prepare_state(&mut self, _time: u64) {
        for (i, node) in self.nodes.iter_mut().enumerate() {
            // Ensure the cached proposal tracks the current blockchain height so round-change
            // leaders can propose immediately.
            if node.proposed_height() < node.height() {
                let timestamp =
                    node.blockchain().head().header.timestamp + node.configuration().block_time;
                let header = BlockHeader {
                    proposer: node.id(),
                    round_number: node.round(),
                    commit_seals: vec![],
                    height: node.height(),
                    timestamp,
                    validators: self.validators.clone(),
                    digest: Default::default(),
                };
                let block = Block::new(
                    header,
                    Bytes::from(format!("test block body {i}").as_bytes().to_vec()),
                );
                node.blockchain_mut().set_proposed_block(Some(block));
                assert!(
                    node.proposed_height() == node.height(),
                    "Proposed height should match expected height"
                );
            }
        }
    }

    #[allow(dead_code)]
    fn node_mut(&mut self, index: usize) -> &mut NodeState {
        self.nodes.get_mut(index).unwrap()
    }

    fn from_snapshot(snapshot_text: &str) -> Result<Self, SnapshotParseError> {
        let snapshot = SwarmSnapshot::parse(snapshot_text)?;
        let mut swarm = NodeSwarm::new(snapshot.nodes.len());
        swarm.apply_snapshot(&snapshot)?;
        Ok(swarm)
    }

    fn apply_snapshot(&mut self, snapshot: &SwarmSnapshot) -> Result<(), SnapshotParseError> {
        for node_snapshot in &snapshot.nodes {
            let node = self
                .nodes
                .get_mut(node_snapshot.index)
                .ok_or(SnapshotParseError::InvalidNodeIndex(node_snapshot.index))?;
            node_snapshot.apply(node, &self.validators)?;
        }
        Ok(())
    }

    fn next_tick(time: u64, tick: u64) -> (u64, u64) {
        let new_tick = (tick + 1) % Self::TICKS_PER_SECOND;
        let new_time = if new_tick == 0 { time + 1 } else { time };
        (new_time, new_tick)
    }
}

fn round_change_for_block(
    swarm: &NodeSwarm,
    signer_index: usize,
    round: u64,
    prepared_round: Option<u64>,
    block: Option<&Block>,
) -> RoundChange {
    let signer_node = swarm.node(signer_index);
    let prepared_value = block.and_then(|block| {
        let prepared_round = prepared_round?;
        let mut prepared_block = block.clone();
        prepared_block.header.round_number = prepared_round;
        prepared_block.header.proposer = proposer(prepared_round, signer_node.blockchain());
        Some(digest(&prepared_block))
    });
    let unsigned = UnsignedRoundChange {
        height: signer_node.blockchain().height(),
        round,
        prepared_value,
        prepared_round,
    };

    RoundChange {
        round_change_payload: sign_round_change(&unsigned, signer_node),
        round_change_justification: vec![],
        proposed_block_for_next_round: block.cloned(),
    }
}

fn prepare_for_block(swarm: &NodeSwarm, signer_index: usize, round: u64, block: &Block) -> Prepare {
    let signer_node = swarm.node(signer_index);
    let mut prepared_block = block.clone();
    prepared_block.header.round_number = round;
    prepared_block.header.proposer = proposer(round, signer_node.blockchain());
    let unsigned = UnsignedPrepare {
        height: signer_node.blockchain().height(),
        round,
        digest: digest(&prepared_block),
    };

    Prepare {
        prepare_payload: sign_prepare(&unsigned, signer_node),
    }
}

const SNAPSHOT_DEFAULT_LOCAL_TIME: u64 = 1_000_000;

#[derive(Debug, Clone)]
struct SwarmSnapshot {
    _time: u64,
    nodes: Vec<NodeSnapshot>,
}

impl SwarmSnapshot {
    fn parse(input: &str) -> Result<Self, SnapshotParseError> {
        let mut time = None;
        let mut nodes = Vec::new();
        for raw_line in input.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('@') {
                let prefix = rest
                    .split(['.', ' '])
                    .next()
                    .ok_or_else(|| SnapshotParseError::InvalidHeader(line.to_string()))?;
                let parsed_time =
                    prefix
                        .parse::<u64>()
                        .map_err(|err| SnapshotParseError::InvalidInteger {
                            field: "time",
                            value: prefix.to_string(),
                            source: err,
                        })?;
                time = Some(parsed_time);
                continue;
            }
            if line.starts_with('[') {
                nodes.push(line.parse()?);
                continue;
            }
            return Err(SnapshotParseError::UnrecognizedLine(line.to_string()));
        }
        if nodes.is_empty() {
            return Err(SnapshotParseError::EmptySnapshot);
        }
        Ok(Self {
            _time: time.unwrap_or(SNAPSHOT_DEFAULT_LOCAL_TIME),
            nodes,
        })
    }
}

#[derive(Debug, Clone)]
struct NodeSnapshot {
    index: usize,
    height: u64,
    block_timeout_delta: i64,
    round_timeout_delta: i64,
    chain: Vec<(u64, u64)>,
    round: u64,
    has_prepared: bool,
    has_prepared_block: bool,
    has_accepted_proposal: bool,
    incoming_summary: Option<String>,
}

impl NodeSnapshot {
    fn apply(
        &self,
        node: &mut NodeState,
        validators: &[Address],
    ) -> Result<(), SnapshotParseError> {
        let blockchain = build_blockchain(&self.chain, validators)?;
        node.set_blockchain(blockchain);
        node.set_round(self.round);

        if node.blockchain().height() != self.height {
            return Err(SnapshotParseError::HeightMismatch {
                expected: self.height,
                actual: node.blockchain().height(),
            });
        }

        let local_time = apply_signed_delta(node.next_block_timeout(), self.block_timeout_delta)?;
        node.set_local_time(local_time);

        let next_round_deadline = apply_signed_delta(local_time, -self.round_timeout_delta)?;
        let timeout = round_timeout(node);
        node.set_time_last_round_start(next_round_deadline.saturating_sub(timeout));

        if self.has_prepared && self.has_prepared_block {
            let prepared_block = node.blockchain().head().clone();
            node.set_last_prepared_block_and_round(prepared_block, self.round);
        }

        if self.has_accepted_proposal {
            let proposal = placeholder_proposal(node.blockchain().head(), self.round, validators);
            node.set_proposal_accepted_for_current_round(Some(proposal));
        } else {
            node.set_proposal_accepted_for_current_round(None);
        }

        let mut messages = if let Some(summary) = &self.incoming_summary {
            decode_message_summary(summary, validators)?
        } else {
            Vec::new()
        };
        messages.sort_by_key(|message| message.components());
        node.set_messages_received(messages);
        let len = node.messages_received_len();
        node.set_first_future_message(len);
        node.blockchain_mut().prune();
        Ok(())
    }
}

impl FromStr for NodeSnapshot {
    type Err = SnapshotParseError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let trimmed = line.trim();
        if !trimmed.starts_with('[') {
            return Err(SnapshotParseError::InvalidLine(trimmed.to_string()));
        }
        let end = trimmed
            .find(']')
            .ok_or_else(|| SnapshotParseError::InvalidLine(trimmed.to_string()))?;
        let inner = &trimmed[1..end];
        let mut tokens = inner.split_whitespace();
        let index_str = tokens
            .next()
            .ok_or(SnapshotParseError::MissingField("index"))?;
        let index_str = index_str
            .strip_prefix("val")
            .or_else(|| index_str.strip_prefix("node"))
            .unwrap_or(index_str);
        let index =
            index_str
                .parse::<usize>()
                .map_err(|err| SnapshotParseError::InvalidInteger {
                    field: "index",
                    value: index_str.to_string(),
                    source: err,
                })?;

        let mut height = None;
        let mut block_timeout_delta = None;
        let mut round_timeout_delta = None;
        let mut chain = None;
        let mut round = None;
        let mut prva = None;
        let mut incoming = None;

        for token in tokens {
            let (key, value) = token
                .split_once('=')
                .ok_or_else(|| SnapshotParseError::InvalidToken(token.to_string()))?;
            match key {
                "h" => {
                    height =
                        Some(
                            value
                                .parse()
                                .map_err(|err| SnapshotParseError::InvalidInteger {
                                    field: "h",
                                    value: value.to_string(),
                                    source: err,
                                })?,
                        )
                }
                "bt" => {
                    block_timeout_delta =
                        Some(
                            value
                                .parse()
                                .map_err(|err| SnapshotParseError::InvalidInteger {
                                    field: "bt",
                                    value: value.to_string(),
                                    source: err,
                                })?,
                        )
                }
                "rt" => {
                    round_timeout_delta =
                        Some(
                            value
                                .parse()
                                .map_err(|err| SnapshotParseError::InvalidInteger {
                                    field: "rt",
                                    value: value.to_string(),
                                    source: err,
                                })?,
                        )
                }
                "chain" => chain = Some(parse_chain(value)?),
                "r" => {
                    round =
                        Some(
                            value
                                .parse()
                                .map_err(|err| SnapshotParseError::InvalidInteger {
                                    field: "r",
                                    value: value.to_string(),
                                    source: err,
                                })?,
                        )
                }
                "prva" => prva = Some(parse_prva_bits(value)?),
                "in" => {
                    if !value.is_empty() {
                        incoming = Some(value.to_string());
                    }
                }
                _ => {}
            }
        }

        let (has_prepared, has_prepared_block, has_accepted_proposal) =
            prva.ok_or(SnapshotParseError::MissingField("prva"))?;

        Ok(Self {
            index,
            height: height.ok_or(SnapshotParseError::MissingField("h"))?,
            block_timeout_delta: block_timeout_delta
                .ok_or(SnapshotParseError::MissingField("bt"))?,
            round_timeout_delta: round_timeout_delta
                .ok_or(SnapshotParseError::MissingField("rt"))?,
            chain: chain.ok_or(SnapshotParseError::MissingField("chain"))?,
            round: round.ok_or(SnapshotParseError::MissingField("r"))?,
            has_prepared,
            has_prepared_block,
            has_accepted_proposal,
            incoming_summary: incoming,
        })
    }
}

#[derive(Debug)]
enum SnapshotParseError {
    EmptySnapshot,
    InvalidHeader(String),
    InvalidLine(String),
    InvalidToken(String),
    InvalidNodeIndex(usize),
    MissingField(&'static str),
    UnrecognizedLine(String),
    InvalidInteger {
        field: &'static str,
        value: String,
        source: ParseIntError,
    },
    UnknownMessageCode(char),
    NegativeDeadline(i64),
    HeightMismatch {
        expected: u64,
        actual: u64,
    },
}

impl fmt::Display for SnapshotParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotParseError::EmptySnapshot => {
                write!(f, "snapshot must include at least one node")
            }
            SnapshotParseError::InvalidHeader(line) => {
                write!(f, "invalid snapshot header: {line}")
            }
            SnapshotParseError::InvalidLine(line) => {
                write!(f, "invalid snapshot line: {line}")
            }
            SnapshotParseError::InvalidToken(token) => {
                write!(f, "invalid token `{token}` in snapshot line")
            }
            SnapshotParseError::InvalidNodeIndex(idx) => {
                write!(f, "snapshot references missing node index {idx}")
            }
            SnapshotParseError::MissingField(field) => {
                write!(f, "snapshot missing required field `{field}`")
            }
            SnapshotParseError::UnrecognizedLine(line) => {
                write!(f, "unrecognized snapshot line `{line}`")
            }
            SnapshotParseError::InvalidInteger { field, value, .. } => {
                write!(f, "invalid integer for {field}: {value}")
            }
            SnapshotParseError::UnknownMessageCode(code) => {
                write!(f, "unknown message code `{code}` in summary")
            }
            SnapshotParseError::NegativeDeadline(delta) => {
                write!(f, "deadline underflow when applying delta {delta}")
            }
            SnapshotParseError::HeightMismatch { expected, actual } => {
                write!(
                    f,
                    "snapshot height mismatch: expected {}, reconstructed {}",
                    expected, actual
                )
            }
        }
    }
}

impl std::error::Error for SnapshotParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SnapshotParseError::InvalidInteger { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn apply_signed_delta(base: u64, delta: i64) -> Result<u64, SnapshotParseError> {
    if delta >= 0 {
        Ok(base.saturating_add(delta as u64))
    } else {
        let offset = (-delta) as u64;
        base.checked_sub(offset)
            .ok_or(SnapshotParseError::NegativeDeadline(delta))
    }
}

fn parse_chain(value: &str) -> Result<Vec<(u64, u64)>, SnapshotParseError> {
    let mut entries = Vec::new();
    for pair in value.split(',').filter(|part| !part.is_empty()) {
        let (height, timestamp) = pair
            .split_once('/')
            .ok_or_else(|| SnapshotParseError::InvalidToken(pair.to_string()))?;
        let h = height
            .parse::<u64>()
            .map_err(|err| SnapshotParseError::InvalidInteger {
                field: "chain.height",
                value: height.to_string(),
                source: err,
            })?;
        let ts = timestamp
            .parse::<u64>()
            .map_err(|err| SnapshotParseError::InvalidInteger {
                field: "chain.timestamp",
                value: timestamp.to_string(),
                source: err,
            })?;
        entries.push((h, ts));
    }
    if entries.is_empty() {
        return Err(SnapshotParseError::InvalidToken(value.to_string()));
    }
    Ok(entries)
}

fn parse_prva_bits(value: &str) -> Result<(bool, bool, bool), SnapshotParseError> {
    if value.len() != 3 {
        return Err(SnapshotParseError::InvalidToken(value.to_string()));
    }
    let mut chars = value.chars();
    let pr = chars.next().unwrap() == '1';
    let pv = chars.next().unwrap() == '1';
    let pa = chars.next().unwrap() == '1';
    Ok((pr, pv, pa))
}

fn build_blockchain(
    chain: &[(u64, u64)],
    validators: &[Address],
) -> Result<Blockchain, SnapshotParseError> {
    let mut blocks = VecDeque::new();
    for &(height, timestamp) in chain {
        blocks.push_back(snapshot_block(height, timestamp, 0, validators));
    }
    if blocks.is_empty() {
        return Err(SnapshotParseError::InvalidLine("chain".into()));
    }
    Ok(Blockchain::new(blocks))
}

fn snapshot_block(height: u64, timestamp: u64, round: u64, validators: &[Address]) -> Block {
    let proposer = if validators.is_empty() {
        Address::ZERO
    } else {
        let idx = (height as usize) % validators.len();
        validators[idx]
    };
    let header = BlockHeader {
        proposer,
        round_number: round,
        commit_seals: vec![],
        height,
        timestamp,
        validators: validators.to_vec(),
        digest: Default::default(),
    };
    Block::new(
        header,
        Bytes::from(format!("snapshot-block-{height}-{timestamp}-{round}").into_bytes()),
    )
}

fn decode_message_summary(
    summary: &str,
    validators: &[Address],
) -> Result<Vec<QbftMessage>, SnapshotParseError> {
    if summary.is_empty() {
        return Ok(Vec::new());
    }
    let mut messages = Vec::new();
    for chunk in summary.split('+').filter(|c| !c.is_empty()) {
        let (prefix, codes) = chunk
            .split_once(':')
            .ok_or_else(|| SnapshotParseError::InvalidToken(chunk.to_string()))?;
        let (height_str, round_str) = prefix
            .split_once('.')
            .ok_or_else(|| SnapshotParseError::InvalidToken(prefix.to_string()))?;
        let height =
            height_str
                .parse::<u64>()
                .map_err(|err| SnapshotParseError::InvalidInteger {
                    field: "message.height",
                    value: height_str.to_string(),
                    source: err,
                })?;
        let round = round_str
            .parse::<u64>()
            .map_err(|err| SnapshotParseError::InvalidInteger {
                field: "message.round",
                value: round_str.to_string(),
                source: err,
            })?;
        for code in codes.chars() {
            if code.is_whitespace() || code == '!' {
                continue;
            }
            messages.push(build_message_from_code(code, height, round, validators)?);
        }
    }
    Ok(messages)
}

fn build_message_from_code(
    code: char,
    height: u64,
    round: u64,
    validators: &[Address],
) -> Result<QbftMessage, SnapshotParseError> {
    let author = if validators.is_empty() {
        Address::ZERO
    } else {
        let idx = (height as usize + round as usize) % validators.len();
        validators[idx]
    };
    let signature = fake_signature(author);
    match code {
        'P' => {
            let mut block = snapshot_block(height, height * 2, round, validators);
            block.header.round_number = round;
            let unsigned = UnsignedProposal {
                height,
                round,
                digest: digest(&block),
            };
            Ok(QbftMessage::Proposal(Proposal {
                proposal_payload: SignedProposal {
                    unsigned_payload: unsigned,
                    signature,
                },
                proposed_block: block,
                ..Default::default()
            }))
        }
        'p' => {
            let unsigned = UnsignedPrepare {
                height,
                round,
                digest: Hash::default(),
            };
            Ok(QbftMessage::Prepare(Prepare {
                prepare_payload: SignedPrepare {
                    unsigned_payload: unsigned,
                    signature,
                },
            }))
        }
        'c' => {
            let unsigned = UnsignedCommit {
                height,
                round,
                commit_seal: signature.clone(),
                digest: Hash::default(),
            };
            Ok(QbftMessage::Commit(Commit {
                commit_payload: SignedCommit {
                    unsigned_payload: unsigned,
                    signature,
                },
            }))
        }
        'r' | 'R' => {
            let unsigned = UnsignedRoundChange {
                height,
                round,
                prepared_value: None,
                prepared_round: if code == 'R' {
                    Some(round.saturating_sub(1))
                } else {
                    None
                },
            };
            Ok(QbftMessage::RoundChange(RoundChange {
                round_change_payload: SignedRoundChange {
                    unsigned_payload: unsigned,
                    signature,
                },
                round_change_justification: vec![],
                proposed_block_for_next_round: None,
            }))
        }
        'b' => {
            let block = snapshot_block(height, height * 2, round, validators);
            Ok(QbftMessage::NewBlock(fake_signed_new_block(block, author)))
        }
        other => Err(SnapshotParseError::UnknownMessageCode(other)),
    }
}

fn placeholder_proposal(head: &Block, round: u64, validators: &[Address]) -> Proposal {
    let mut block = head.clone();
    block.header.round_number = round;
    let unsigned = UnsignedProposal {
        height: head.header.height + 1,
        round,
        digest: digest(&block),
    };
    let author = validators.first().copied().unwrap_or(Address::ZERO);
    Proposal {
        proposal_payload: SignedProposal {
            unsigned_payload: unsigned,
            signature: fake_signature(author),
        },
        proposed_block: block,
        ..Default::default()
    }
}

fn fake_signature(author: Address) -> Signature {
    Signature {
        author,
        signature: [0u8; 65],
    }
}

fn fake_signed_new_block(block: Block, author: Address) -> SignedNewBlock {
    let unsigned = UnsignedNewBlock {
        height: block.header.height,
        round: block.header.round_number,
        digest: digest(&block),
    };
    SignedNewBlock {
        unsigned_payload: unsigned,
        signature: fake_signature(author),
        block,
    }
}

fn signed_new_block_with_key(block: Block, private_key: &B256) -> SignedNewBlock {
    let unsigned = UnsignedNewBlock {
        height: block.header.height,
        round: block.header.round_number,
        digest: digest(&block),
    };
    let encoded = alloy_rlp::encode(&unsigned);
    let signature = Signature::sign_message(&encoded, private_key);
    SignedNewBlock {
        unsigned_payload: unsigned,
        signature,
        block,
    }
}

fn build_signed_block_for_timestamp(swarm: &NodeSwarm, timestamp: u64) -> SignedNewBlock {
    let header = BlockHeader {
        proposer: swarm.validators[0],
        round_number: 0,
        commit_seals: vec![],
        height: swarm.node(0).height(),
        timestamp,
        validators: swarm.validators.clone(),
        digest: Default::default(),
    };

    let mut block = Block::new(
        header,
        Bytes::from(format!("future timestamp {timestamp}").into_bytes()),
    );
    let seal_hash = hash_block_for_commit_seal(&block);
    let quorum_size = quorum(swarm.validators.len());
    block.header.commit_seals = swarm
        .private_keys()
        .iter()
        .take(quorum_size)
        .map(|key| Signature::sign_message(seal_hash.as_slice(), key))
        .collect();
    signed_new_block_with_key(block, &swarm.private_keys()[0])
}

fn oversized_round_change_message(
    swarm: &NodeSwarm,
    target_round: u64,
    signer_index: usize,
) -> RoundChange {
    let unsigned_round_change = UnsignedRoundChange {
        height: swarm.node(signer_index).height(),
        round: target_round,
        prepared_value: None,
        prepared_round: None,
    };
    let encoded = alloy_rlp::encode(&unsigned_round_change);
    let signature = Signature::sign_message(&encoded, &swarm.private_keys()[signer_index]);
    RoundChange {
        round_change_payload: SignedRoundChange {
            unsigned_payload: unsigned_round_change,
            signature,
        },
        round_change_justification: vec![],
        proposed_block_for_next_round: None,
    }
}

/// The purpose of this test is to ensure that the Rust code matches the Dafny spec
/// when all nodes behave correctly in the happy path.
///
/// For this test, time is fixed at the first block production time.
#[test]
fn full_happy_path() {
    setup_tracing();
    let mut swarm = NodeSwarm::new(4);

    for t in 0..12 {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            swarm.tick(t, tick);
        }
    }
    // With hardened timeouts we still expect steady progress.
    let expected_blocks = 2;
    assert!(
        swarm.min_height() >= expected_blocks,
        "At least {expected_blocks} blocks should have been produced"
    );
}

#[test]
fn full_happy_path_with_unique_proposers() {
    setup_tracing();
    let mut swarm = NodeSwarm::new(4);

    for t in 0..14 {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            swarm.tick(t, tick);
        }
    }
    // Unique proposer rotation slows production and round changes add delay.
    let expected_blocks = 2;
    assert!(
        swarm.min_height() >= expected_blocks,
        "At least {expected_blocks} blocks should have been produced"
    );
}

/// Test case where the proposer fails to propose a block.
///
/// This test forces a round change by disabling the first validator.
/// As it does not produce a block at t=2, the second validator must take up the task
/// through the round change mechanism at t=3 when it emits four round change messages for round
/// 1.1:
///
/// ```
/// RUST_LOG=info cargo test qbft::tests::failing_propser -- --nocapture
/// ```
/// ```
///  INFO swarm: --------------- @3 --------------
///  INFO swarm: [1 h=1 bt=1 rt=0 chain=0/0 r=0 prva=000 i= P=1345] -> 1.1:r
///  INFO swarm: [2 h=1 bt=1 rt=0 chain=0/0 r=0 prva=000 i= P=1345] -> 1.1:r
///  INFO swarm: [3 h=1 bt=1 rt=0 chain=0/0 r=0 prva=000 i= P=1345] -> 1.1:r
///  INFO swarm: [4 h=1 bt=1 rt=0 chain=0/0 r=0 prva=000 i= P=1345] -> 1.1:r
///  ```
///
///  Once we have round change messages, we need the next validator (1) to receive
///  a proposal justification and emit a proposal for its block.
/// ```
///  INFO swarm: --------------- @3 --------------
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: 1 has_received_proposal_justification for round 1.
///  INFO swarm: [1 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:rrrr P=345] -> 1.1:P
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: is_proposal_justification: fail all null prepared
///  INFO swarm: [2 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:rrrr P=345] -> 1.1:r
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: is_proposal_justification: fail all null prepared
///  INFO swarm: [3 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:rrrr P=345] -> 1.1:r
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: is_proposal_justification: fail all null prepared
///  INFO swarm: [4 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:rrrr P=345] -> 1.1:r
/// ```
/// 
/// Note that we also send more (duplicate) round change messages. This may not
/// be intentional, but is harmless.
///
/// Next we generate prepares and set the accepted proposal state flag (prva=001)
/// ```
///  INFO swarm: --------------- @3 --------------
///  INFO swarm: [1 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:Prrrr P=345] -> 1.1:p
///  INFO swarm: [2 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:Prrrr P=345] -> 1.1:p
///  INFO swarm: [3 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:Prrrr P=345] -> 1.1:p
///  INFO swarm: [4 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=000 i=1.1:Prrrr P=345] -> 1.1:p
/// ```
/// 
/// Next we commit. We are prevented from sending more prepares by is_valid_proposal
/// because we have an accepted proposal for this round (1).
/// ```
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO swarm: [1 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=001 i=1.1:Ppppprrrr P=35] -> 1.1:c
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO swarm: [2 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=001 i=1.1:Ppppprrrr P=35] -> 1.1:c
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO swarm: [3 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=001 i=1.1:Ppppprrrr P=35] -> 1.1:c
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO swarm: [4 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=001 i=1.1:Ppppprrrr P=35] -> 1.1:c
///  ```
///
///  Finally, we commit and set the block height to 2:
///  We are prevented from sending more commits because we have done this already.
///
///  ```
///   INFO swarm: --------------- @3 --------------
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO qbft: 1 has already sent commit for height 1 round 1
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: is_proposal_justification: fail round change quorum for round 1
///  INFO swarm: [1 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=111 i=1.1:Pppppccccrrrr P=35]
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO qbft: 2 has already sent commit for height 1 round 1
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: is_proposal_justification: fail round change quorum for round 1
///  INFO swarm: [2 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=111 i=1.1:Pppppccccrrrr P=35]
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO qbft: 3 has already sent commit for height 1 round 1
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: is_proposal_justification: fail round change quorum for round 1
///  INFO swarm: [3 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=111 i=1.1:Pppppccccrrrr P=35]
///  INFO qbft: Invalid proposal proposal round=1 <= current round=1 (already have a proposal)
///  INFO qbft: 4 has already sent commit for height 1 round 1
///  INFO qbft: Checking for proposal justification at round 1
///  INFO qbft: is_proposal_justification: fail round change quorum for round 1
///  INFO swarm: [4 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=111 i=1.1:Pppppccccrrrr P=35]
///  ```
/// INFO swarm: [4 h=1 bt=1 rt=-1 chain=0/0 r=1 prva=111 i=1.1:Pppppccccrrrr P=35]
#[test]
fn failing_proposer() {
    setup_tracing();
    let mut swarm = NodeSwarm::new(5);

    // Disable node 0 which should be the first proposer.
    // This node remains silent throughout the test.
    swarm.disable_node(0);

    for t in 0..13 {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            swarm.tick(t, tick);
        }
    }

    let expected_blocks = 2; // With one proposer disabled, expect slightly fewer blocks.
    assert!(
        swarm.min_height() >= expected_blocks,
        "At least {expected_blocks} blocks should have been produced despite failing proposer"
    );
}

/// Injects a malicious NewBlock with a timestamp far in the future.
/// Honest nodes should continue producing blocks instead of waiting for that timestamp.
#[test]
fn future_timestamp_block_should_not_freeze_chain() {
    setup_tracing();
    let mut swarm = NodeSwarm::new(4);
    let expected_height_after_attack = 10;
    let attack_height = expected_height_after_attack - 3;
    let desired_progress_height = expected_height_after_attack + 1;
    let mut pre_attack_height = None;

    // Generous bound for slow rounds.
    let max_ticks = desired_progress_height * NodeSwarm::TICKS_PER_SECOND * 20;
    let mut time = 0;
    let mut tick = 0;
    for _ in 0..max_ticks {
        if pre_attack_height.is_none() && swarm.min_height() >= attack_height {
            pre_attack_height = Some(swarm.min_height());
            let node0 = swarm.node(0);
            let future_timestamp = node0.local_time() + 10_000;
            let malicious_block = build_signed_block_for_timestamp(&swarm, future_timestamp);
            println!(
                "Injecting malicious block when min_height reached {attack_height} \
                 (t={time}.{tick}): node0 local_time={} head_ts={} nominal_timeout={} (delta={}), \
                 forged_timestamp={future_timestamp}",
                node0.local_time(),
                node0.head().header.timestamp,
                node0.next_block_timeout(),
                node0
                    .next_block_timeout()
                    .saturating_sub(node0.local_time()),
            );
            swarm.broadcast_immediate(QbftMessage::NewBlock(malicious_block), time, tick);
        }

        swarm.tick(time, tick);

        tick += 1;
        if tick >= NodeSwarm::TICKS_PER_SECOND {
            tick = 0;
            time += 1;
        }

        if pre_attack_height.is_some() && swarm.min_height() >= desired_progress_height {
            break;
        }
    }

    let baseline = pre_attack_height.expect("attack height was never reached");
    assert_eq!(
        baseline, attack_height,
        "Malicious block should trigger exactly when min_height reaches attack height"
    );
    let post_attack_height = swarm.min_height();
    println!(
        "Baseline height {baseline}, post-attack min height {post_attack_height} (expected >= \
         {expected_height_after_attack})"
    );
    assert!(
        post_attack_height >= expected_height_after_attack,
        "Nodes should advance to at least height {expected_height_after_attack} even though the \
         malicious block was injected at height {attack_height}"
    );
    for (idx, node) in swarm.nodes.iter().enumerate() {
        let timeout = node.next_block_timeout();
        let delta = timeout.saturating_sub(node.local_time());
        println!(
            "node {idx}: height={} local_time={} next_block_timeout={} (delta={}) \
             latest_block_ts={}",
            node.height(),
            node.local_time(),
            timeout,
            delta,
            node.head().header.timestamp,
        );
    }
    println!(
        "Nodes stay at round 0 waiting for timeout >> local_time, so no new proposals are \
         triggered."
    );

    assert!(
        post_attack_height >= desired_progress_height,
        "Swarm stalled after future timestamp block: expected to reach at least height \
         {desired_progress_height} but only reached {post_attack_height}"
    );
}

/// Delivers a forged round-change message for an absurd round number.
/// Nodes should ignore it and keep making progress.
#[test]
fn ignores_unbounded_round_change_requests() {
    setup_tracing();
    let mut swarm = NodeSwarm::new(4);
    let attack_round = 1_000_000;
    let attack_time = 4;
    let attack_tick = 0;
    let mut pre_attack_height = None;

    for t in 0..12 {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            if t == attack_time && tick == attack_tick && pre_attack_height.is_none() {
                pre_attack_height = Some(swarm.min_height());
                let forged_round_change = oversized_round_change_message(&swarm, attack_round, 0);
                swarm.broadcast_immediate(QbftMessage::RoundChange(forged_round_change), t, tick);
            }
            swarm.tick(t, tick);
        }
    }

    let baseline = pre_attack_height.expect("attack should have been scheduled");
    let post_attack_height = swarm.min_height();
    assert!(
        post_attack_height > baseline,
        "Swarm should continue to make progress despite absurd round-change message"
    );

    for (idx, node) in swarm.nodes.iter().enumerate() {
        assert!(
            node.round() < attack_round,
            "Node {idx} adopted unrealistic round {}",
            node.round()
        );
    }
}

#[test]
fn random_message_delays() {
    setup_tracing();
    let mut swarm = NodeSwarm::new(4);

    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    let mut rng = StdRng::seed_from_u64(42);

    // Note this is known to fail with a larger time period.
    for t in 0..200 {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            swarm.tick(t, tick);
            swarm.map_delayed_messages(|mut msg| {
                // Randomly delay some messages
                if msg.is_new && rng.gen_bool(0.5) {
                    let delay_ticks = rng.gen_range(1..=NodeSwarm::TICKS_PER_SECOND / 2);
                    let delay = TestTime::new(0, delay_ticks);
                    msg.deliver_at = msg.deliver_at.add(&delay);
                    info!(
                        target: "swarm",
                        "Delaying message {} to node {} by {} ticks",
                        msg_error_code(&msg.message),
                        msg.node,
                        delay_ticks
                    );
                }
                Some(msg)
            });
        }
    }
    // With 4 nodes and random delays, we expect at least 2 blocks to be produced.
    let expected_blocks = 4;
    assert!(
        swarm.min_height() >= expected_blocks,
        "At least {expected_blocks} blocks should have been produced"
    );
}

#[test]
fn highest_prepared_round_requires_quorum_for_same_block() {
    setup_tracing();
    let mut swarm = NodeSwarm::new(4);
    let new_round = 3;
    let highest_prepared_round = 2;
    let height = swarm.node(0).blockchain().height();
    let validators = swarm.validators.clone();

    let block_with_quorum = snapshot_block(height, 11, highest_prepared_round, &validators);
    let conflicting_block = snapshot_block(height, 22, highest_prepared_round, &validators);

    let proposer_address = proposer(new_round, swarm.node(0).blockchain());
    let proposer_index = swarm
        .validators
        .iter()
        .position(|addr| *addr == proposer_address)
        .expect("proposer should exist");

    swarm.node_mut(proposer_index).set_round(new_round);

    let mut initial_messages = Vec::new();
    initial_messages.push(QbftMessage::RoundChange(round_change_for_block(
        &swarm,
        0,
        new_round,
        Some(highest_prepared_round),
        Some(&block_with_quorum),
    )));
    initial_messages.push(QbftMessage::RoundChange(round_change_for_block(
        &swarm,
        1,
        new_round,
        Some(highest_prepared_round),
        Some(&block_with_quorum),
    )));
    initial_messages.push(QbftMessage::RoundChange(round_change_for_block(
        &swarm,
        2,
        new_round,
        Some(highest_prepared_round),
        Some(&conflicting_block),
    )));

    for signer in 0..3 {
        initial_messages.push(QbftMessage::Prepare(prepare_for_block(
            &swarm,
            signer,
            highest_prepared_round,
            &block_with_quorum,
        )));
    }

    swarm
        .node_mut(proposer_index)
        .add_messages(initial_messages, 0);

    assert!(
        has_received_proposal_justification(swarm.node(proposer_index)).is_none(),
        "Insufficient round-change quorum for a single block should not justify a proposal"
    );

    let extra_round_change = round_change_for_block(
        &swarm,
        3,
        new_round,
        Some(highest_prepared_round),
        Some(&block_with_quorum),
    );

    swarm
        .node_mut(proposer_index)
        .add_messages(vec![QbftMessage::RoundChange(extra_round_change)], 0);

    let justification = has_received_proposal_justification(swarm.node(proposer_index))
        .expect("round-change quorum for block_with_quorum should now justify");

    assert_eq!(
        digest(&justification.block),
        digest(&block_with_quorum),
        "Justification must choose the block that achieved quorum"
    );
    assert_eq!(
        justification.round, new_round,
        "Justification should reference the expected round"
    );
}

#[test]
fn snapshot_round_trip_matches_summary() {
    setup_tracing();
    let snapshot = "\
@5
[val0 h=1 bt=1 rt=0 chain=0/0 r=0 prva=001 in=1.0:P]
[val1 h=1 bt=0 rt=-1 chain=0/0 r=0 prva=001 in=1.0:Pp]
[val2 h=1 bt=0 rt=-1 chain=0/0 r=0 prva=001 in=1.0:Pp]
[val3 h=1 bt=0 rt=-1 chain=0/0 r=0 prva=001 in=1.0:Pp]
";
    let swarm = NodeSwarm::from_snapshot(snapshot).expect("snapshot should parse");
    let expected = [
        "[p h=1 bt=1 rt=0 chain=0/0 r=0 prva=001 in=1.0:P]",
        "[a h=1 bt=0 rt=-1 chain=0/0 r=0 prva=001 in=1.0:Pp]",
        "[a h=1 bt=0 rt=-1 chain=0/0 r=0 prva=001 in=1.0:Pp]",
        "[a h=1 bt=0 rt=-1 chain=0/0 r=0 prva=001 in=1.0:Pp]",
    ];
    for (idx, expected_summary) in expected.into_iter().enumerate() {
        assert_eq!(
            swarm.node(idx).summarise(),
            expected_summary,
            "node {idx} summary mismatch"
        );
    }
}

// #[test]
// fn prepared_round_changes() {
//     setup_tracing();

//     let mut swarm = NodeSwarm::new(5);

//     'wfp: for t in 0..10 {
//         for _ in 0..10 {
//             if swarm.node(0).last_prepared_round().is_some() {
//                 // Once we have a prepared round, force a round change.
//                 break 'wfp;
//             }
//             swarm.tick(t);
//         }
//     }

//     info!(target: "swarm", "############ Removing commits from node 0 to allow round change to
// proceed ############");

//     swarm.node_mut(0).remove_commits();
//     swarm.disable_node(4);

//     for t in swarm.node(0).next_round_timeout()..10 {
//         swarm.tick(t);
//     }
// }

fn setup_tracing() {
    use tracing_subscriber::{self, EnvFilter};

    // Initialize tracing subscriber for test output without timestamps
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(filter)
        .with_ansi(false)
        .without_time()
        .try_init();
}

#[test]
fn test_qbft_message_encode_decode_round_trip() {
    use super::types::{Commit, Prepare, RoundChange};
    use alloy_rlp::{Decodable, Encodable};

    // Helper function to test round trip for any QbftMessage variant
    fn test_round_trip(original: QbftMessage) {
        // Encode the message
        let mut encoded = Vec::new();
        original.encode(&mut encoded);

        // Decode the message
        let mut buf = encoded.as_slice();
        let decoded = QbftMessage::decode(&mut buf).expect("Failed to decode QbftMessage");

        // Verify they are equal
        assert_eq!(
            original, decoded,
            "Round trip failed for QbftMessage variant"
        );
    }

    // Test Proposal variant
    let proposal_msg = QbftMessage::Proposal(Proposal::default());
    test_round_trip(proposal_msg);

    // Test Prepare variant
    let prepare_msg = QbftMessage::Prepare(Prepare::default());
    test_round_trip(prepare_msg);

    // Test Commit variant
    let commit_msg = QbftMessage::Commit(Commit::default());
    test_round_trip(commit_msg);

    // Test RoundChange variant
    let round_change_msg = QbftMessage::RoundChange(RoundChange::default());
    test_round_trip(round_change_msg);

    // Test NewBlock variant with a default block
    let new_block_key = B256::from([0x10; 32]);
    let new_block_msg =
        QbftMessage::NewBlock(signed_new_block_with_key(Block::default(), &new_block_key));
    test_round_trip(new_block_msg);

    info!(target: "swarm", "All QbftMessage round trip tests passed!");
}

#[test]
fn test_qbft_message_encode_decode_with_real_data() {
    use alloy_primitives::Bytes;
    use alloy_rlp::{Decodable, Encodable};

    // Helper function to test round trip for any QbftMessage variant
    fn test_round_trip(original: QbftMessage) {
        // Encode the message
        let mut encoded = Vec::new();
        original.encode(&mut encoded);

        // Decode the message
        let mut buf = encoded.as_slice();
        let decoded = QbftMessage::decode(&mut buf).expect("Failed to decode QbftMessage");

        // Verify they are equal
        assert_eq!(
            original, decoded,
            "Round trip failed for QbftMessage variant"
        );
    }

    // Test NewBlock variant with a block containing actual data
    // Use a valid ECDSA private key
    let test_private_key = B256::from([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x05,
    ]);
    let test_commit_seal = Signature::sign_message(b"test_commit_seal", &test_private_key);

    let header = BlockHeader {
        proposer: "0x1234567890123456789012345678901234567890"
            .parse()
            .unwrap(),
        round_number: 42,
        commit_seals: vec![test_commit_seal],
        height: 100,
        timestamp: 1234567890,
        validators: vec![
            "0x1111111111111111111111111111111111111111"
                .parse()
                .unwrap(),
            "0x2222222222222222222222222222222222222222"
                .parse()
                .unwrap(),
        ],
        digest: "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
            .parse()
            .unwrap(),
    };
    let test_block = Block::new_with_transactions(
        header,
        Bytes::from("test block body with some data"),
        vec![Bytes::from("transaction1"), Bytes::from("transaction2")],
    );

    let new_block_msg =
        QbftMessage::NewBlock(signed_new_block_with_key(test_block, &test_private_key));
    test_round_trip(new_block_msg);

    info!(target: "swarm", "QbftMessage round trip test with real data passed!");
}

#[test]
fn test_qbft_message_comprehensive_encoding() {
    use super::types::{Commit, Prepare, RoundChange};
    use alloy_rlp::{Decodable, Encodable};

    // Test that each QbftMessage variant encodes and decodes correctly
    // and that different variants produce different encodings

    let proposal_msg = QbftMessage::Proposal(Proposal::default());
    let prepare_msg = QbftMessage::Prepare(Prepare::default());
    let commit_msg = QbftMessage::Commit(Commit::default());
    let round_change_msg = QbftMessage::RoundChange(RoundChange::default());
    let new_block_key = B256::from([0x11; 32]);
    let new_block_msg =
        QbftMessage::NewBlock(signed_new_block_with_key(Block::default(), &new_block_key));

    let messages = vec![
        ("Proposal", proposal_msg),
        ("Prepare", prepare_msg),
        ("Commit", commit_msg),
        ("RoundChange", round_change_msg),
        ("NewBlock", new_block_msg),
    ];

    let mut encodings = Vec::new();

    for (name, msg) in messages {
        // Test round trip
        let mut encoded = Vec::new();
        msg.encode(&mut encoded);

        let mut buf = encoded.as_slice();
        let decoded = QbftMessage::decode(&mut buf).expect("Failed to decode");

        assert_eq!(msg, decoded, "Round trip failed for {}", name);

        // Verify we can distinguish the message type after decode
        match (&msg, &decoded) {
            (QbftMessage::Proposal(_), QbftMessage::Proposal(_)) => {}
            (QbftMessage::Prepare(_), QbftMessage::Prepare(_)) => {}
            (QbftMessage::Commit(_), QbftMessage::Commit(_)) => {}
            (QbftMessage::RoundChange(_), QbftMessage::RoundChange(_)) => {}
            (QbftMessage::NewBlock(_), QbftMessage::NewBlock(_)) => {}
            _ => panic!("Decoded message type doesn't match original for {}", name),
        }

        encodings.push((name, encoded.clone()));
        info!(
            "{} encodes to {} bytes, starts with: {:?}",
            name,
            encoded.len(),
            &encoded[..std::cmp::min(5, encoded.len())]
        );
    }

    // Verify all encodings are different
    for i in 0..encodings.len() {
        for j in i + 1..encodings.len() {
            assert_ne!(
                encodings[i].1, encodings[j].1,
                "{} and {} should encode differently",
                encodings[i].0, encodings[j].0
            );
        }
    }

    info!("All QbftMessage variants encode/decode correctly and uniquely!");
}

#[test]
fn test_signature_structure() {
    use alloy_primitives::B256;

    // Test creating signatures through proper cryptographic signing
    // Use valid ECDSA private keys
    let private_key1 = B256::from([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01,
    ]);
    let private_key2 = B256::from([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x04,
    ]);
    let message1 = b"test message 1";
    let message2 = b"test message 2";

    // Create signatures using cryptographic signing
    let sig1 = Signature::sign_message(message1, &private_key1);
    let sig2 = Signature::sign_message(message2, &private_key2);

    // Verify signatures have different authors (from different private keys)
    assert_ne!(sig1.author(), sig2.author());

    // Verify signatures are different (different messages and keys)
    assert_ne!(sig1.signature_bytes(), sig2.signature_bytes());

    // Test RLP round trip
    use alloy_rlp::{Decodable, Encodable};
    let mut encoded = Vec::new();
    sig2.encode(&mut encoded);

    let mut buf = encoded.as_slice();
    let decoded_sig = Signature::decode(&mut buf).expect("Failed to decode signature");
    assert_eq!(sig2, decoded_sig);

    // Test signing with a private key
    // Use a valid ECDSA private key
    let private_key = B256::from([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01,
    ]);
    let message = b"test message";

    let signed_sig = Signature::sign_message(message, &private_key);
    assert_ne!(signed_sig.signature, [0u8; 65]); // Ensure signature was created

    // Test that the same private key produces the same signature for the same message
    let signed_sig2 = Signature::sign_message(message, &private_key);
    assert_eq!(signed_sig.signature, signed_sig2.signature);
    assert_eq!(signed_sig.author(), signed_sig2.author());

    // Test that different messages produce different signatures
    let different_message = b"different test message";
    let signed_sig3 = Signature::sign_message(different_message, &private_key);
    assert_ne!(signed_sig.signature, signed_sig3.signature);
    assert_eq!(signed_sig.author(), signed_sig3.author()); // Same author for same key

    // Verify all signatures from the same key have the same author
    let expected_address = signed_sig.author();
    assert_eq!(signed_sig.author(), expected_address);
    assert_eq!(signed_sig2.author(), expected_address);
    assert_eq!(signed_sig3.author(), expected_address);

    info!("Signature structure tests passed!");
}

#[test]
fn test_message_author_validation() {
    use super::types::{SignedPrepare, UnsignedPrepare};
    use alloy_primitives::B256;

    // Create known validators
    let validators: Vec<Address> = vec![
        Address::from([0x01; 20]),
        Address::from([0x02; 20]),
        Address::from([0x03; 20]),
    ];

    // Create a signature from a validator's private key (from our test setup)
    // Use a valid ECDSA private key (must be non-zero and less than secp256k1 curve order)
    let validator_private_key = B256::from([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x02,
    ]);

    // Create the unsigned prepare payload first
    let unsigned_prepare_payload = UnsignedPrepare {
        height: 1,
        round: 0,
        digest: Default::default(),
    };

    // Sign the RLP-encoded unsigned payload (this is what check_signatures expects)
    let unsigned_prepare_rlp = alloy_rlp::encode(&unsigned_prepare_payload);
    let valid_signature = Signature::sign_message(&unsigned_prepare_rlp, &validator_private_key);
    let valid_author = valid_signature.author(); // Get the actual author from the signature

    // Update validators list to include the actual signer
    let mut updated_validators = validators.clone();
    updated_validators[1] = valid_author; // Replace with the actual signer

    // Update the node state with the correct validators
    let header = BlockHeader {
        proposer: updated_validators[0],
        round_number: 0,
        commit_seals: vec![],
        height: 0,
        timestamp: 0,
        validators: updated_validators.clone(),
        digest: Default::default(),
    };
    let updated_genesis_block = Block::new(header, Default::default());

    let updated_configuration = Configuration {
        nodes: updated_validators.clone(),
        genesis_block: updated_genesis_block.clone(),
        block_time: 10,
        round_change_config: RoundChangeConfig {
            start_time: 0.0,
            first_interval: 1.0,
            growth_factor: 2.0,
            max_round: 10,
            round_change_on_first_block: false,
        },
    };

    let updated_blockchain = Blockchain::new(VecDeque::from([updated_genesis_block]));
    let mut node_state = NodeState::new(
        updated_blockchain,
        updated_configuration,
        updated_validators[0],
        B256::from([0x01; 32]),
        0,
    );

    // Test 1: Valid message from a validator should be accepted
    let valid_message = QbftMessage::Prepare(Prepare {
        prepare_payload: SignedPrepare {
            unsigned_payload: unsigned_prepare_payload,
            signature: valid_signature, // Valid validator with proper cryptographic signature
        },
    });

    // Test 2: Invalid message from non-validator should be rejected
    // Use a different valid ECDSA private key
    let invalid_private_key = B256::from([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x03,
    ]);
    let invalid_unsigned_prepare = UnsignedPrepare {
        height: 1,
        round: 0,
        digest: Default::default(),
    };
    let invalid_unsigned_prepare_rlp = alloy_rlp::encode(&invalid_unsigned_prepare);
    let invalid_signature =
        Signature::sign_message(&invalid_unsigned_prepare_rlp, &invalid_private_key);
    let invalid_message = QbftMessage::Prepare(Prepare {
        prepare_payload: SignedPrepare {
            unsigned_payload: invalid_unsigned_prepare,
            signature: invalid_signature, // Invalid validator with proper cryptographic signature
        },
    });

    // Before adding messages, there should be no messages
    assert_eq!(node_state.messages_received_len(), 0);

    // Add both messages
    node_state.add_messages(vec![valid_message.clone(), invalid_message.clone()], 0);

    // Only the valid message should be added
    assert_eq!(node_state.messages_received_len(), 1);
    assert_eq!(
        node_state.get_message_at_index(0).unwrap().author(),
        valid_author
    );

    info!("Message author validation tests passed!");
}

//#[test]
// TODO: Fix and re-enable this test once the signature checking logic is finalized.
fn _test_check_signatures() {
    use super::types::{
        Commit, Proposal, SignedCommit, SignedPrepare, SignedProposal, SignedRoundChange,
        UnsignedCommit, UnsignedPrepare, UnsignedProposal, UnsignedRoundChange,
    };
    use alloy_primitives::B256;
    use std::collections::VecDeque;

    // Helper function to create valid ECDSA private keys
    fn create_private_key(index: u8) -> B256 {
        B256::from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, index,
        ])
    }

    let dummy_header = BlockHeader {
        proposer: Address::ZERO,
        round_number: 0,
        commit_seals: vec![],
        height: 0,
        timestamp: 0,
        validators: vec![],
        digest: Default::default(),
    };
    let dummy_blockchain = Blockchain::new(VecDeque::from([Block::new(
        dummy_header,
        Default::default(),
    )]));

    // Test 1: Valid Prepare message
    let private_key1 = create_private_key(1);
    let unsigned_prepare = UnsignedPrepare {
        height: 1,
        round: 0,
        digest: Default::default(),
    };
    let unsigned_prepare_rlp = alloy_rlp::encode(&unsigned_prepare);
    let signature = Signature::sign_message(&unsigned_prepare_rlp, &private_key1);

    let prepare_msg = QbftMessage::Prepare(Prepare {
        prepare_payload: SignedPrepare {
            unsigned_payload: unsigned_prepare.clone(),
            signature: signature.clone(),
        },
    });

    assert!(
        prepare_msg.check_signatures(&dummy_blockchain).unwrap(),
        "Valid Prepare signature should pass"
    );

    // Test 2: Invalid Prepare message (signature doesn't match payload)
    let different_unsigned_prepare = UnsignedPrepare {
        height: 2, // Different height
        round: 0,
        digest: Default::default(),
    };
    let invalid_prepare_msg = QbftMessage::Prepare(Prepare {
        prepare_payload: SignedPrepare {
            unsigned_payload: different_unsigned_prepare, // Different from what was signed
            signature,                                    // Same signature from before
        },
    });

    assert!(
        !invalid_prepare_msg
            .check_signatures(&dummy_blockchain)
            .unwrap(),
        "Invalid Prepare signature should fail"
    );

    // Test 3: Valid Commit message
    let private_key2 = create_private_key(2);
    let unsigned_commit = UnsignedCommit {
        height: 1,
        round: 0,
        commit_seal: Signature::sign_message(b"test_seal", &private_key2),
        digest: Default::default(),
    };
    let unsigned_commit_rlp = alloy_rlp::encode(&unsigned_commit);
    let commit_signature = Signature::sign_message(&unsigned_commit_rlp, &private_key2);

    let commit_msg = QbftMessage::Commit(Commit {
        commit_payload: SignedCommit {
            unsigned_payload: unsigned_commit,
            signature: commit_signature,
        },
    });

    assert!(
        commit_msg.check_signatures(&dummy_blockchain).unwrap(),
        "Valid Commit signature should pass"
    );

    // Test 4: Valid Proposal message with justifications
    let private_key3 = create_private_key(3);
    let unsigned_proposal = UnsignedProposal {
        height: 1,
        round: 0,
        digest: Default::default(),
    };
    let unsigned_proposal_rlp = alloy_rlp::encode(&unsigned_proposal);
    let proposal_signature = Signature::sign_message(&unsigned_proposal_rlp, &private_key3);

    // Create valid round change justification
    let unsigned_rc = UnsignedRoundChange {
        height: 1,
        round: 0,
        prepared_value: None,
        prepared_round: None,
    };
    let unsigned_rc_rlp = alloy_rlp::encode(&unsigned_rc);
    let rc_signature = Signature::sign_message(&unsigned_rc_rlp, &private_key3);
    let round_change_justification = SignedRoundChange {
        unsigned_payload: unsigned_rc,
        signature: rc_signature,
    };

    // Create valid prepare justification
    let prepare_justification = SignedPrepare {
        unsigned_payload: unsigned_prepare,
        signature: Signature::sign_message(&unsigned_prepare_rlp, &private_key3),
    };

    let proposal_msg = QbftMessage::Proposal(Proposal {
        proposal_payload: SignedProposal {
            unsigned_payload: unsigned_proposal,
            signature: proposal_signature,
        },
        proposed_block: Block::default(),
        proposal_justification: vec![round_change_justification],
        round_change_justification: vec![prepare_justification],
    });

    assert!(
        proposal_msg.check_signatures(&dummy_blockchain).unwrap(),
        "Valid Proposal with justifications should pass"
    );

    // Test 5: NewBlock message with commit seals
    let private_key4 = create_private_key(4);
    let proposer = Signature::sign_message(b"proposer", &private_key4).author();
    let proposer_header = BlockHeader {
        proposer,
        round_number: 0,
        commit_seals: vec![],
        height: 0,
        timestamp: 0,
        validators: vec![proposer],
        digest: Default::default(),
    };
    let proposer_blockchain = Blockchain::new(VecDeque::from([Block::new(
        proposer_header,
        Default::default(),
    )]));
    let header = BlockHeader {
        proposer,
        commit_seals: vec![],
        ..Default::default()
    };
    let mut block_with_seals = Block::new(header, Default::default());
    let seal_hash = hash_block_for_commit_seal(&block_with_seals);
    let commit_seal = Signature::sign_message(seal_hash.as_slice(), &private_key4);
    block_with_seals.header.commit_seals = vec![commit_seal];

    let new_block_msg =
        QbftMessage::NewBlock(signed_new_block_with_key(block_with_seals, &private_key4));
    assert!(
        new_block_msg
            .check_signatures(&proposer_blockchain)
            .unwrap(),
        "Valid NewBlock with commit seals should pass"
    );

    // // Test 6: NewBlock message with invalid commit seal
    // // Create a commit seal that was signed for a different block than the one it's attached to
    // let seal_for_different_block =
    //     Signature::sign_message(b"different_block_content", &private_key4);
    // let seal_author = seal_for_different_block.author();

    // let header = BlockHeader {
    //     proposer: seal_author,
    //     commit_seals: vec![seal_for_different_block], // This seal is not for this block
    //     ..Default::default()
    // };
    // let block_with_invalid_seal = Block::new(header, Default::default());

    // let invalid_new_block_msg = QbftMessage::NewBlock(signed_new_block_with_key(
    //     block_with_invalid_seal,
    //     &private_key4,
    // ));
    // assert!(
    //     !invalid_new_block_msg.check_signatures().unwrap(),
    //     "Invalid NewBlock with wrong commit seals should fail"
    // );

    info!("check_signatures tests passed!");
}

#[test]
fn test_qbft_message_size_regression() {
    use std::mem::size_of;

    // Test to ensure QbftMessage size doesn't grow unexpectedly
    // This helps catch accidental additions of large fields or inefficient layouts

    const EXPECTED_SIZE: usize = 376; // Current size in bytes on 64-bit systems
    let actual_size = size_of::<QbftMessage>();

    assert_eq!(
        actual_size, EXPECTED_SIZE,
        "QbftMessage size has changed from {} to {} bytes. If this change is intentional, update \
         EXPECTED_SIZE. Otherwise, consider if the size increase is necessary for performance.",
        EXPECTED_SIZE, actual_size
    );

    // Also test individual variant sizes to help identify which one changed
    println!("QbftMessage size breakdown:");
    println!("  Total size: {} bytes", actual_size);

    println!("  Proposal variant: {} bytes", size_of::<Proposal>());
    println!("  Prepare variant: {} bytes", size_of::<Prepare>());
    println!("  Commit variant: {} bytes", size_of::<Commit>());
    println!("  RoundChange variant: {} bytes", size_of::<RoundChange>());
    println!("  NewBlock variant: {} bytes", size_of::<SignedNewBlock>());

    info!(
        "QbftMessage size regression test passed - size is {} bytes",
        actual_size
    );
}

// WIP
//
// #[test]
// fn provoke_round_change() {
//     setup_tracing();

//     let mut swarm = NodeSwarm::new(4);
//     let t = swarm.node(0).next_block_timeout();
//     swarm.tick(t, 0);
//     swarm.tick(t, 1);
//     swarm.tick(t, 2);

//     let mut swarm = NodeSwarm::new(4);
//     let t = swarm.node(0).next_block_timeout();
//     swarm.prepare_state(t);
//     swarm.add_proposal_for_round_0();
//     swarm.add_prepares();
//     swarm.process_messages(t, 1);
//     for node in swarm.nodes.iter() {
//         info!(
//             target: "swarm",
//             "{}",
//             node.summarise(),
//         );
//     }
// }

/// Check that dropping all Prepare messages for round 0 still produces blocks.
///
/// In this case, we should produce the block from the round 1 proposer.
#[test]
fn provoke_all_null_prepare() {
    setup_tracing();

    let mut swarm = NodeSwarm::new(4);

    for t in 0..=6 {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            swarm.tick(t, tick);
            swarm.map_delayed_messages(|msg| {
                // Drop all Prepare messages for round 0
                if let QbftMessage::Prepare(prepare) = &msg.message {
                    if prepare.prepare_payload.unsigned_payload.round == 0 {
                        info!(
                            target: "swarm",
                            "Dropping Prepare message to node {} for round 0",
                            msg.node
                        );
                        return None; // Drop the message
                    }
                }
                Some(msg)
            });
        }
    }

    // With hardened timeouts we still expect steady progress.
    let expected_blocks = 2;
    assert!(
        swarm.min_height() >= expected_blocks,
        "At least {expected_blocks} blocks should have been produced"
    );
}

#[test]
/// Check that dropping all Commit messages for round 0 still produces blocks.
///
/// In this case, we should produce the block from the round 1 proposer.
fn provoke_prepared_round_change() {
    setup_tracing();

    let mut swarm = NodeSwarm::new(4);
    let duration_blocks = 3;
    let expected_blocks = 2;

    for t in 0..=duration_blocks * NodeSwarm::SECONDS_PER_BLOCK {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            swarm.tick(t, tick);
            swarm.map_delayed_messages(|msg| {
                // Drop all Commit messages for round 0
                if let QbftMessage::Commit(commit) = &msg.message {
                    if commit.commit_payload.unsigned_payload.round == 0 {
                        info!(
                            target: "swarm",
                            "Dropping Commit message to node {} for round 0",
                            msg.node
                        );
                        return None; // Drop the message
                    }
                }
                Some(msg)
            });
        }
    }

    assert!(
        swarm.min_height() >= expected_blocks,
        "At least {expected_blocks} blocks should have been produced"
    );
}

#[test]
/// Provoke a round change switch in the case of f+1 round changes for future rounds.
///
/// ```
///  INFO swarm: [0 h=2 bt=2 rt=-9995 chain=0/0,1/2 r=0 prva=001 in=2.0:Pp+2.2:rr]
///  INFO qbft: node0: upon_round_change: received 2/2 senders for future rounds.
///  INFO swarm: [0 h=2 bt=2 rt=-9995 chain=0/0,1/2 r=2 prva=000 in=2.0:Pp+2.2:rr] -> 2.2:r
/// ```
fn provoke_upon_round_change_case_2() {
    setup_tracing();

    let mut swarm = NodeSwarm::new(4);
    let duration_blocks = 3;
    let expected_blocks = 2;

    let max_adversary_nodes = f(swarm.nodes().len());

    info!(target: "swarm", "Max adversary nodes: {}", max_adversary_nodes);

    // Run for longer to accommodate exponential backoff in round timeouts
    for t in 0..=(duration_blocks * NodeSwarm::SECONDS_PER_BLOCK * 2) {
        for tick in 0..NodeSwarm::TICKS_PER_SECOND {
            swarm.tick(t, tick);
            // Prevent time-bassed round change for two nodes.
            // This means that we won't get a quorum of round changes for round 1 based on timeout.
            swarm.node_mut(0).set_time_last_round_start(10000);
            swarm.node_mut(1).set_time_last_round_start(10000);

            swarm.map_delayed_messages(|msg| {
                match &msg.message {
                    QbftMessage::Prepare(_) => {
                        if msg.message.round() < 2 {
                            // Prevent node 0 from receieving round 1 changes.
                            return None;
                        }
                    }
                    QbftMessage::RoundChange(_) => {
                        if msg.node == 0 && msg.message.round() < 2 {
                            // Prevent node 0 from receieving round 1 changes.
                            return None;
                        }
                    }
                    _ => {}
                }
                Some(msg)
            });
        }
    }

    let min_height = swarm.min_height();
    assert!(
        min_height >= expected_blocks,
        "At least {expected_blocks} blocks should have been produced, but got {min_height}"
    );
}
