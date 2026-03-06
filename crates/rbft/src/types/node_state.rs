// SPDX-License-Identifier: Apache-2.0
//! QBFT Specification predicates and functions translated from Dafny.
//!
//! This module contains the methods relating to the NodeState struct.
//!
//! Much of the logic is in the node_auxilliary_functions module.
//!
//! There are some practical details here which are not in the Dafny spec.
//! * The Blockchain struct has been modified to hold a proposed_block field.
//! * The Block struct has a list of validators for that block height.

use alloy_primitives::FixedBytes;
use itertools::Itertools;
use std::collections::HashSet;
use tracing::debug;

use crate::{
    node_auxilliary_functions::{
        is_valid_proposal, proposer, quorum, round_timeout, valid_node_state, validators,
    },
    types::{qbft_message::msg_error_code, SignedRoundChange},
};

use super::{
    qbft_message::summarise_messages, Address, Block, Blockchain, Configuration, PrivateKey,
    Proposal, QbftMessage, QbftMessageWithRecipient,
};

// The state of a QBFT node, including its local blockchain, configuration, and messages received.
// Based on the Dafny spec but with some additional fields for practical implementation.
// The Dafny spec is based on the IBFT paper with some additions for practical implementation.
//
// See https://arxiv.org/pdf/2002.03613.pdf for the IBFT paper.
// See https://github.com/Consensys/qbft-formal-spec-and-verification for the Dafny spec.
#[derive(Debug)]
pub struct NodeState {
    /// IBFT inputValue[i]: blockchain.proposed_block
    /// IBFT λ[i]: blockchain.height()
    blockchain: Blockchain,

    /// IBFT r[i]
    round: u64,

    /// Set before advancing the state.
    local_time: u64,

    /// IBFT λ[i]
    id: Address,

    /// QBFT only
    configuration: Configuration,

    /// Received messages sorted by (height, round, sender)
    /// Deduplicated so that there is only one message of each type from each sender.
    messages_received: Vec<QbftMessage>,

    /// QBFT only
    proposal_accepted_for_current_round: Option<Proposal>,

    /// IBFT pv[i]
    last_prepared_block: Option<Block>,

    /// IBFT pr[i]
    last_prepared_round: Option<u64>,

    /// IBFT timer[i]
    time_last_round_start: u64,

    /// Last observed round-0 prepare sender count for the current height.
    round_zero_progress_prepares: usize,

    /// Last observed round-0 commit sender count for the current height.
    round_zero_progress_commits: usize,

    /// Local time when we last observed round-0 progress.
    round_zero_last_progress_time: u64,

    /// Added for practical implementation, not in the Dafny spec.
    /// This is used to sign messages sent by this node.
    /// TODO: followers do not have a private key. This should be an Option.
    private_key: PrivateKey,

    /// The first message at or above the current height.
    first_future_message: usize,

    /// Dedup index for received messages (height, round, type, author).
    message_keys: HashSet<(u64, u64, u8, Address)>,
}

impl NodeState {
    pub fn new(
        blockchain: Blockchain,
        configuration: Configuration,
        id: Address,
        private_key: PrivateKey,
        local_time: u64,
    ) -> Self {
        let block_time = configuration.block_time;
        Self {
            blockchain,
            round: 0,
            local_time,
            id,
            configuration,
            messages_received: vec![],
            proposal_accepted_for_current_round: None,
            last_prepared_block: None,
            last_prepared_round: None,

            // A divergence from the spec to avoid immediate round timeout.
            // Offset the initial round start so round 0 doesn't timeout before block timeout.
            time_last_round_start: local_time + block_time,

            round_zero_progress_prepares: 0,
            round_zero_progress_commits: 0,
            round_zero_last_progress_time: local_time,

            private_key,
            first_future_message: 0,
            message_keys: HashSet::new(),
        }
    }

    pub fn node_next(&mut self, time: u64) -> Vec<QbftMessageWithRecipient> {
        crate::node::node_next(self, time)
    }

    /// Add incoming messages to the node state, validating and filtering them as needed.
    /// Messages are stored as a sorted, deduplicated list in messages_received.
    /// This function also checks message authenticity and validity.
    pub fn add_messages(&mut self, incoming_messages: Vec<QbftMessage>, time: u64) {
        // Set the local time before processing messages so the summary string is accurate
        self.local_time = time;

        // Validate and filter incoming messages
        let current_validators = validators(self.blockchain()).to_vec();

        // Validate incoming messages.
        for msg in incoming_messages {
            if msg.height() < self.blockchain().height() {
                let code = msg_error_code(&msg);
                debug!(
                    target: "qbft",
                    "Rejected message {code} from previous height"
                );
                continue;
            }

            let key = msg.components();
            if self.message_keys.contains(&key) {
                continue;
            }

            let code = msg_error_code(&msg);
            let accepted = msg.check_authors(current_validators.as_slice())
                && msg.check_signatures(self.blockchain()).unwrap_or(false)
                && msg.check_digest();

            if accepted {
                self.insert_message_sorted(msg);
            } else {
                debug!(target: "qbft", "Rejected message {code}");
            }
        }

        self.prune_messages_below_height();

        self.blockchain_mut().prune();
    }

    fn insert_message_sorted(&mut self, message: QbftMessage) {
        let key = message.components();
        if self.message_keys.contains(&key) {
            return;
        }
        let index = self
            .messages_received
            .binary_search_by_key(&key, |m| m.components())
            .unwrap_or_else(|idx| idx);
        self.messages_received.insert(index, message);
        self.message_keys.insert(key);
    }

    fn rebuild_message_index(&mut self) {
        self.messages_received
            .sort_by_key(|message| message.components());
        self.messages_received
            .dedup_by(|a, b| a.components().eq(&b.components()));
        self.message_keys = self
            .messages_received
            .iter()
            .map(|message| message.components())
            .collect();
    }

    pub fn prune_messages_below_height(&mut self) {
        let first_at_this_height = self
            .messages_received
            .partition_point(|m| m.height() < self.blockchain().height());

        // Remove messages below the current height.
        for msg in self.messages_received.drain(0..first_at_this_height) {
            self.message_keys.remove(&msg.components());
        }

        self.first_future_message = self
            .messages_received
            .partition_point(|m| m.height() <= self.blockchain().height());
    }

    /// Returns true if the next block timeout has been reached.
    pub fn next_block_timeout(&self) -> u64 {
        self.blockchain().head().header.timestamp + self.configuration.block_time
    }

    /// Returns true if the block timeout is ready and we are the proposer for round 0.
    /// Returns false if a proposal has already been sent.
    pub fn upon_block_timeout_ready(&self) -> bool {
        self.round == 0
            && proposer(0, self.blockchain()) == self.id
            && self.local_time() >= self.next_block_timeout()
            && self.proposal_accepted_for_current_round().is_none()
    }

    /// Check that the current node state is valid.
    pub fn is_valid(&self) -> bool {
        valid_node_state(self)
    }

    pub fn next_round_timeout(&self) -> u64 {
        self.time_last_round_start + round_timeout(self)
    }

    /// Generate a new block for the given round based on the proposed block in the blockchain.
    ///
    /// Note that this assumes that the proposed block has already been set in the blockchain.
    pub fn get_new_block(&self, round: u64) -> Option<Block> {
        let block = self.blockchain().proposed_block()?;

        let mut block = block.clone();

        block.header.round_number = round;

        Some(block)
    }

    /// Return all messages received for the current height.
    pub fn messages_received(&self) -> &[QbftMessage] {
        &self.messages_received[0..self.first_future_message]
    }

    /// Return all messages received for the current and future heights.
    pub fn all_messages_received(&self) -> &[QbftMessage] {
        &self.messages_received
    }

    /// Return all messages received for the current height and round.
    pub fn messages_received_at_round(&self, round: u64) -> &[QbftMessage] {
        let lower = self
            .messages_received()
            .partition_point(|m| m.round() < round);
        let upper = self
            .messages_received()
            .partition_point(|m| m.round() <= round);
        &self.messages_received[lower..upper]
    }

    /// Diagnostic summary string for the current node state and messages.
    pub fn summarise(&self) -> String {
        let _height = self.blockchain().height();
        let round = self.round;
        let block_timeout = self.local_time() as i64 - self.next_block_timeout() as i64;
        let round_timeout = self.local_time() as i64 - self.next_round_timeout() as i64;
        let quorum_size = quorum(validators(self.blockchain()).len());
        let m_in = summarise_messages(self.all_messages_received().iter(), quorum_size);

        let pr = self.last_prepared_round.is_some() as u8;
        let pv = self.last_prepared_block.is_some() as u8;
        let pa = self.proposal_accepted_for_current_round().is_some() as u8;

        // Just list the head block plus the blocks with the same timestamp
        // in the case of sub-second block times.
        let head_timestamp = self.blockchain().head().header.timestamp;
        let ht: Vec<_> = self
            .blockchain()
            .blocks()
            .iter()
            .rev()
            .map_while(|b| {
                if b.header.timestamp == head_timestamp {
                    Some(format!("{}/{}", b.header.height, b.header.timestamp))
                } else {
                    None
                }
            })
            .collect();
        let ht = ht.into_iter().rev().join(",");

        let height = self.height();
        let label = self.role();
        format!(
            "[{label} h={height} bt={block_timeout} rt={round_timeout} chain={ht} r={round} \
             prva={pr}{pv}{pa} in={m_in}]",
        )
    }

    /// Return the role of this node.
    fn role(&self) -> char {
        let role = if self.private_key().is_zero() {
            // Follower node - no private key.
            'f'
        } else if self.is_the_proposer_for_current_round() {
            // Proposer node
            'p'
        } else if self.blockchain().validators().contains(&self.id) {
            // Active validator
            'a'
        } else {
            // Inactive validator
            'x'
        };
        role
    }

    // /// Return all messages received for the current height.
    // /// Use the fact that messages are sorted to find the relevant slice.
    // pub fn messages_received_for_current_height(&self) -> &[QbftMessage] {
    //     let height = self.blockchain.height();
    //     let start = self.messages_received.partition_point(|m| m.height() < height);
    //     let end = self.messages_received.partition_point(|m| m.height() <= height);
    //     &self.messages_received[start..end]
    // }

    /// Return the first Proposal message for the current height, if any.
    pub fn valid_proposal(&self) -> Option<&Proposal> {
        // In theory, there should only be one valid proposal per height and round.
        // Because:
        // * The proposal author has a unique proposer for each round.
        // * We deduplicate messages by (height, round, type, author).
        // So we can just find the first valid proposal in the messages for the current height.

        self.messages_received.iter().find_map(|m| {
            if let QbftMessage::Proposal(proposal) = m {
                if is_valid_proposal(proposal, self) {
                    Some(proposal)
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    pub fn proposer(&self, round: u64) -> Address {
        proposer(round, self.blockchain())
    }

    pub fn whoami(&self) -> usize {
        self.validator_index().unwrap_or(99)
    }

    pub fn validator_index(&self) -> Option<usize> {
        self.blockchain()
            .validators()
            .iter()
            .position(|v| v == &self.id)
    }

    pub fn label(&self) -> String {
        if let Some(idx) = self.validator_index() {
            return format!("val{idx}");
        }

        let nodes = &self.configuration.nodes;
        if !nodes.is_empty() {
            let validators = self.blockchain().validators();
            let mut non_validators = Vec::new();
            for node in nodes {
                if !validators.iter().any(|v| v == node) {
                    non_validators.push(node);
                }
            }
            if let Some(idx) = non_validators.iter().position(|node| *node == &self.id) {
                return format!("node{idx}");
            }
            if let Some(idx) = nodes.iter().position(|node| node == &self.id) {
                return format!("node{idx}");
            }
        }

        format!("node{}", self.whoami())
    }

    pub fn proposed_height(&self) -> u64 {
        self.blockchain()
            .proposed_block()
            .as_ref()
            .map(|b| b.header.height)
            .unwrap_or(0)
    }

    pub fn head(&self) -> &Block {
        self.blockchain().head()
    }

    pub fn height(&self) -> u64 {
        self.blockchain().height()
    }

    /// Returns the proposer address for the current round.
    ///
    /// Returns the address we should be forwarding transactions to.
    ///
    /// - No proposal accepted yet for this round → current proposer (`self.round`); they still need
    ///   our transactions to build the block.
    /// - Proposal already accepted → next proposer (`self.round + 1`); the current proposer's block
    ///   is already decided, so forward to whoever will propose next.
    ///
    /// Returns `Address::ZERO` when the validator set is empty.
    pub fn next_proposer_for_current_round(&self) -> Address {
        let round = if self.proposal_accepted_for_current_round.is_some() {
            self.round + 1
        } else {
            self.round
        };
        crate::node_auxilliary_functions::proposer(round, &self.blockchain)
    }

    pub fn blockchain(&self) -> &Blockchain {
        &self.blockchain
    }

    pub fn round(&self) -> u64 {
        self.round
    }

    pub fn local_time(&self) -> u64 {
        self.local_time
    }

    pub fn id(&self) -> Address {
        self.id
    }

    pub fn configuration(&self) -> &Configuration {
        &self.configuration
    }

    pub fn proposal_accepted_for_current_round(&self) -> Option<&Proposal> {
        self.proposal_accepted_for_current_round.as_ref()
    }

    pub fn last_prepared_block(&self) -> Option<&Block> {
        self.last_prepared_block.as_ref()
    }

    pub fn last_prepared_round(&self) -> Option<u64> {
        self.last_prepared_round
    }

    pub fn time_last_round_start(&self) -> u64 {
        self.time_last_round_start
    }

    pub fn round_zero_last_progress_time(&self) -> u64 {
        self.round_zero_last_progress_time
    }

    pub fn private_key(&self) -> FixedBytes<32> {
        self.private_key
    }

    pub fn first_future_message(&self) -> usize {
        self.first_future_message
    }

    pub fn set_blockchain(&mut self, blockchain: Blockchain) {
        self.blockchain = blockchain;
    }

    pub fn set_round(&mut self, round: u64) {
        self.round = round;
    }

    pub fn set_local_time(&mut self, local_time: u64) {
        self.local_time = local_time;
    }

    pub fn set_time_last_round_start(&mut self, time_last_round_start: u64) {
        self.time_last_round_start = time_last_round_start;
    }

    pub fn set_round_zero_last_progress_time(&mut self, time: u64) {
        self.round_zero_last_progress_time = time;
    }

    pub fn set_id(&mut self, id: Address) {
        self.id = id;
    }

    pub fn set_configuration(&mut self, configuration: Configuration) {
        self.configuration = configuration;
    }

    pub fn set_messages_received(&mut self, messages_received: Vec<QbftMessage>) {
        self.messages_received = messages_received;
        self.rebuild_message_index();
        self.first_future_message = self.first_future_message.min(self.messages_received.len());
    }

    pub fn set_proposal_accepted_for_current_round(
        &mut self,
        proposal_accepted_for_current_round: Option<Proposal>,
    ) {
        self.proposal_accepted_for_current_round = proposal_accepted_for_current_round;
    }

    pub fn set_first_future_message(&mut self, first_future_message: usize) {
        self.first_future_message = first_future_message;
    }

    pub fn push_message(&mut self, message: QbftMessage) {
        self.insert_message_sorted(message);
    }

    pub fn messages_received_len(&self) -> usize {
        self.messages_received.len()
    }

    pub fn get_message_at_index(&self, index: usize) -> Option<&QbftMessage> {
        self.messages_received.get(index)
    }

    /// These must be set atomically.
    pub fn set_last_prepared_block_and_round(
        &mut self,
        last_prepared_block: Block,
        last_prepared_round: u64,
    ) {
        self.last_prepared_block = Some(last_prepared_block);
        self.last_prepared_round = Some(last_prepared_round);
    }

    /// Start a new round, resetting relevant fields.
    pub fn new_round(
        &mut self,
        round: u64,
        time_last_round_start: u64,
        proposal_accepted_for_current_round: Option<Proposal>,
    ) {
        self.round = round;
        self.proposal_accepted_for_current_round = proposal_accepted_for_current_round;
        self.time_last_round_start = time_last_round_start;
        if round == 0 {
            self.last_prepared_block = None;
            self.last_prepared_round = None;
            self.round_zero_progress_prepares = 0;
            self.round_zero_progress_commits = 0;
            self.round_zero_last_progress_time = time_last_round_start;
        }
    }

    pub fn blockchain_mut(&mut self) -> &mut Blockchain {
        &mut self.blockchain
    }

    /// Updates the round-0 progress marker for the current height.
    ///
    /// Returns true if progress increased compared to the previous marker.
    pub fn update_round_zero_progress(&mut self, prepares: usize, commits: usize) -> bool {
        let progressed = prepares > self.round_zero_progress_prepares
            || commits > self.round_zero_progress_commits;
        if progressed {
            self.round_zero_progress_prepares = prepares;
            self.round_zero_progress_commits = commits;
            self.round_zero_last_progress_time = self.local_time;
        }
        progressed
    }

    pub fn is_the_proposer_for_current_round(&self) -> bool {
        proposer(self.round, self.blockchain()) == self.id
    }

    /// Returns a list of available rounds from received round change messages that are greater than
    /// the current round.
    pub fn get_future_rounds_from_round_changes(&self) -> Vec<u64> {
        let mut rounds = self
            .messages_received()
            .iter()
            .map(QbftMessage::round)
            .filter(|&r| r > self.round)
            .collect::<Vec<_>>();
        rounds.dedup();
        rounds
    }

    pub(crate) fn received_signed_round_changes_for_round(
        &self,
        round: u64,
    ) -> Vec<&SignedRoundChange> {
        self.messages_received_at_round(round)
            .iter()
            .filter_map(|msg| {
                if let QbftMessage::RoundChange(rc) = msg {
                    Some(&rc.round_change_payload)
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use alloy_primitives::Bytes;

    use super::*;
    use crate::types::{
        configuration::{Configuration, RoundChangeConfig},
        Block, BlockHeader,
    };

    fn make_node_state(validators: Vec<Address>, proposer: Address) -> NodeState {
        let header = BlockHeader {
            proposer,
            height: 0,
            validators: validators.clone(),
            ..BlockHeader::default()
        };
        let genesis = Block::new(header, Bytes::new());
        let configuration = Configuration {
            nodes: validators.clone(),
            genesis_block: genesis.clone(),
            block_time: 1,
            round_change_config: RoundChangeConfig {
                start_time: 0.0,
                first_interval: 1.0,
                growth_factor: 2.0,
                max_round: 10,
                round_change_on_first_block: true,
            },
        };
        let blockchain = Blockchain::new(VecDeque::from([genesis]));
        NodeState::new(
            blockchain,
            configuration,
            validators[0],
            PrivateKey::default(),
            0,
        )
    }

    #[test]
    fn test_next_proposer_round_zero() {
        // On genesis (height == 1), round_zero_index = 0.
        // No proposal accepted: round 0 → validators[0] = v0
        // Proposal accepted:    round 1 → validators[1] = v1
        let v0 = Address::from([0u8; 20]);
        let v1 = Address::from([1u8; 20]);
        let v2 = Address::from([2u8; 20]);
        let mut node = make_node_state(vec![v0, v1, v2], v0);
        assert_eq!(node.next_proposer_for_current_round(), v0);

        node.set_proposal_accepted_for_current_round(Some(Proposal::default()));
        assert_eq!(node.next_proposer_for_current_round(), v1);
    }

    #[test]
    fn test_next_proposer_round_one() {
        // On genesis, round 1 → v1 (no proposal); with proposal → round 2 → v2
        let v0 = Address::from([0u8; 20]);
        let v1 = Address::from([1u8; 20]);
        let v2 = Address::from([2u8; 20]);
        let mut node = make_node_state(vec![v0, v1, v2], v0);
        node.set_round(1);
        assert_eq!(node.next_proposer_for_current_round(), v1);

        node.set_proposal_accepted_for_current_round(Some(Proposal::default()));
        assert_eq!(node.next_proposer_for_current_round(), v2);
    }

    #[test]
    fn test_next_proposer_after_first_block() {
        // After genesis is committed (height == 2), previous proposer = v0.
        // round_zero_index = position(v0)+1 = 1.
        // No proposal: round 0 → v1; with proposal: round 1 → v2
        let v0 = Address::from([0u8; 20]);
        let v1 = Address::from([1u8; 20]);
        let v2 = Address::from([2u8; 20]);
        let validators = vec![v0, v1, v2];
        let mut node = make_node_state(validators.clone(), v0);

        // Commit a block with v0 as proposer, advancing height to 2.
        let h = BlockHeader {
            proposer: v0,
            height: 1,
            validators,
            ..BlockHeader::default()
        };
        node.blockchain_mut().set_head(Block::new(h, Bytes::new()));

        assert_eq!(node.next_proposer_for_current_round(), v1);
        node.set_proposal_accepted_for_current_round(Some(Proposal::default()));
        assert_eq!(node.next_proposer_for_current_round(), v2);
    }

    #[test]
    fn test_next_proposer_wraps_around() {
        // After genesis, previous proposer = v2; round_zero_index = (2+1)%3 = 0.
        // round 0 → v0; round 1 → v1
        let v0 = Address::from([0u8; 20]);
        let v1 = Address::from([1u8; 20]);
        let v2 = Address::from([2u8; 20]);
        let validators = vec![v0, v1, v2];
        let mut node = make_node_state(validators.clone(), v0);

        let h = BlockHeader {
            proposer: v2,
            height: 1,
            validators,
            ..BlockHeader::default()
        };
        node.blockchain_mut().set_head(Block::new(h, Bytes::new()));

        assert_eq!(node.next_proposer_for_current_round(), v0);
        node.set_round(1);
        assert_eq!(node.next_proposer_for_current_round(), v1);
    }

    #[test]
    fn test_next_proposer_empty_validators() {
        // Guard: returns Address::ZERO when validator list is empty.
        let v0 = Address::from([1u8; 20]);
        let mut node = make_node_state(vec![v0], v0);
        // Push a block with an empty validator list to simulate the guard condition.
        let empty_header = BlockHeader {
            height: 1,
            validators: vec![],
            ..BlockHeader::default()
        };
        node.blockchain_mut()
            .set_head(Block::new(empty_header, Bytes::new()));
        assert_eq!(node.next_proposer_for_current_round(), Address::ZERO);
    }
}
