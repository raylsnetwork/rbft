// SPDX-License-Identifier: Apache-2.0
//! This module is a 1:1 translation of the L1_AuxiliaryFunctionsAndLemmas module from Dafny to
//! Rust. It should look as similar as posible to the original.
//!
//! * The functions are in the same order as in the Dafny module.
//! * The variable names are the same as in the Dafny module, using snake_case for Rust.
//!
//! Some variation is necessary. For example, signing messages requires a private key.
//!
//! Sets are kept as slices and iterators for simplicity. Do not change them to HashSet without
//! careful consideration.

use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, warn};

use crate::types::{
    Address, Block, Blockchain, Hash, NodeState, Optional, Prepare, Proposal,
    ProposalJustification, QbftMessage, QbftMessageWithRecipient, RoundChange, Signature,
    SignedCommit, SignedNewBlock, SignedPrepare, SignedProposal, SignedRoundChange, UnsignedCommit,
    UnsignedNewBlock, UnsignedPrepare, UnsignedProposal, UnsignedRoundChange,
};

// =======================================================================
// CRYPTOGRAPHIC PRIMITIVES
// =======================================================================

// Note that signing occurs before we send the messages externally.
// Likewise, recovery occurs on receipt of messages with the exception of forwarding.

/// Returns the digest of the block header (including any commit seals).
pub fn digest(block: &Block) -> Hash {
    let header = block.header.clone();
    let encoded = alloy_rlp::encode(&header);
    alloy_primitives::keccak256(encoded)
}

/// Signs an unsigned proposal and returns a signed proposal.
pub fn sign_proposal(msg: &UnsignedProposal, node_state: &NodeState) -> SignedProposal {
    use alloy_rlp::Encodable;

    // RLP encode the message
    let mut encoded = Vec::new();
    msg.encode(&mut encoded);

    // Create signature using the node's private key
    let signature = Signature::sign_message(&encoded, &node_state.private_key());

    SignedProposal {
        unsigned_payload: msg.clone(),
        signature,
    }
}

/// Recovers the author of a signed proposal.
pub fn recover_signed_proposal_author(msg: &SignedProposal) -> Address {
    msg.signature.author()
}

/// Signs an unsigned prepare and returns a signed prepare.
pub fn sign_prepare(msg: &UnsignedPrepare, node_state: &NodeState) -> SignedPrepare {
    use alloy_rlp::Encodable;

    // RLP encode the message
    let mut encoded = Vec::new();
    msg.encode(&mut encoded);

    // Create signature using the node's private key
    let signature = Signature::sign_message(&encoded, &node_state.private_key());

    SignedPrepare {
        unsigned_payload: msg.clone(),
        signature,
    }
}

/// Recovers the author of a signed prepare.
pub fn recover_signed_prepare_author(msg: &SignedPrepare) -> Address {
    msg.signature.author()
}

/// Signs an unsigned commit and returns a signed commit.
pub fn sign_commit(msg: &UnsignedCommit, node_state: &NodeState) -> SignedCommit {
    use alloy_rlp::Encodable;

    // RLP encode the message
    let mut encoded = Vec::new();
    msg.encode(&mut encoded);

    // Create signature using the node's private key
    let signature = Signature::sign_message(&encoded, &node_state.private_key());

    SignedCommit {
        unsigned_payload: msg.clone(),
        signature,
    }
}

/// Recovers the author of a signed commit.
pub fn recover_signed_commit_author(msg: &SignedCommit) -> Address {
    msg.signature.author()
}

/// Signs an unsigned round change and returns a signed round change.
pub fn sign_round_change(msg: &UnsignedRoundChange, node_state: &NodeState) -> SignedRoundChange {
    use alloy_rlp::Encodable;

    // RLP encode the message
    let mut encoded = Vec::new();
    msg.encode(&mut encoded);

    // Create signature using the node's private key
    let signature = Signature::sign_message(&encoded, &node_state.private_key());

    SignedRoundChange {
        unsigned_payload: msg.clone(),
        signature,
    }
}

/// Recovers the author of a signed round change.
pub fn recover_signed_round_change_author(msg: &SignedRoundChange) -> Address {
    msg.signature.author()
}

/// Signs an unsigned NewBlock payload and returns a signed NewBlock message with the block.
pub fn sign_new_block(block: &Block, node_state: &NodeState) -> SignedNewBlock {
    use alloy_rlp::Encodable;

    let unsigned = UnsignedNewBlock {
        height: block.header.height,
        round: block.header.round_number,
        digest: digest(block),
    };

    let mut encoded = Vec::new();
    unsigned.encode(&mut encoded);

    let signature = Signature::sign_message(&encoded, &node_state.private_key());

    SignedNewBlock {
        unsigned_payload: unsigned,
        signature,
        block: block.clone(),
    }
}

/// Recovers the author of a signed NewBlock message.
pub fn recover_signed_new_block_author(signature: &Signature) -> Address {
    signature.author()
}

/// Signs a hash and returns a signature.
pub fn sign_hash(hash: &Hash, node_state: &NodeState) -> Signature {
    // Create signature using the node's private key
    Signature::sign_message(hash.as_slice(), &node_state.private_key())
}

/// Recovers the author of a signed hash.
pub fn recover_signed_hash_author(_hash: &Hash, signature: &Signature) -> Address {
    signature.author()
}

// =======================================================================
// UNDEFINED FUNCTIONS
// =======================================================================

// =======================================================================
// GENERAL FUNCTIONS
// =======================================================================

/// Checks if an optional value is present.
pub fn _option_is_present<T>(o: &Optional<T>) -> bool {
    matches!(o, Optional::Some(_))
}

/// Gets the value of an optional, assuming it is present.
pub fn _option_get<T>(o: &Optional<T>) -> &T {
    match o {
        Optional::Some(value) => value,
        Optional::None => panic!("Optional value is not present"),
    }
}

/// Computes the union of a sequence of sets.
pub fn _set_union_on_seq<T: Clone + Eq + std::hash::Hash>(
    sets: &[std::collections::HashSet<T>],
) -> std::collections::HashSet<T> {
    sets.iter().flatten().cloned().collect()
}

/// Computes 2 raised to the power of `exp`.
pub fn _power_of_2(exp: u64) -> u64 {
    1 << exp
}

// =======================================================================
// QBFT GENERAL FUNCTIONS
// =======================================================================

/// Retrieves the validators for a blockchain.
pub fn validators(blockchain: &Blockchain) -> &[Address] {
    blockchain.validators()
}

/// Returns the f value (maximum faulty nodes).
pub fn f(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        (n - 1) / 3
    }
}

/// Computes the quorum size for a given number of validators.
pub fn quorum(n: usize) -> usize {
    (n * 2 - 1) / 3 + 1
}

fn format_addresses<I>(addrs: I) -> String
where
    I: IntoIterator<Item = Address>,
{
    let mut list: Vec<String> = addrs.into_iter().map(|a| format!("{a:?}")).collect();
    list.sort();
    list.join(",")
}

/// Returns the round timeout for a given round.
/// Uses the RoundChangeConfig from the node state to calculate:
/// start_time + first_interval * (growth_factor ^ round)
pub fn round_timeout(current: &NodeState) -> u64 {
    current
        .configuration()
        .round_change_config
        .timeout_for_round(current.round())
}

/// Returns round-0 prepare/commit sender counts for the accepted proposal, if any.
pub fn round_zero_progress_counts(current: &NodeState) -> Option<(usize, usize)> {
    if current.round() != 0 {
        return None;
    }

    let proposal = current.proposal_accepted_for_current_round()?;
    let height = current.blockchain().height();
    let block_digest = digest(&proposal.proposed_block);
    let validators = validators(current.blockchain());

    let prepare_senders: HashSet<Address> = valid_prepares_for_height_round_and_digest(
        height,
        0,
        &block_digest,
        current.messages_received(),
    )
    .into_iter()
    .map(recover_signed_prepare_author)
    .filter(|addr| validators.contains(addr))
    .collect();

    let commit_senders: HashSet<Address> = valid_commits_for_height_round_and_digest(
        height,
        0,
        &block_digest,
        current.messages_received(),
    )
    .into_iter()
    .map(recover_signed_commit_author)
    .filter(|addr| validators.contains(addr))
    .collect();

    Some((prepare_senders.len(), commit_senders.len()))
}

/// Replaces the round in a block with a new round.
pub fn replace_round_in_block(block: &Block, new_round: u64) -> Block {
    let mut new_block = block.clone();
    new_block.header.round_number = new_round;
    new_block
}

/// Computes the hash of a block for commit seal.
pub fn hash_block_for_commit_seal(block: &Block) -> Hash {
    // Commit seals sign the header without any commit seals to avoid circularity.
    let mut header = block.header.clone();
    header.commit_seals.clear();
    let encoded = alloy_rlp::encode(&header);
    alloy_primitives::keccak256(encoded)
}

/// Determines the proposer for a given round and blockchain.
pub fn proposer(round: u64, blockchain: &Blockchain) -> Address {
    // TODO: check the dafny as this is probbaly wrong.
    let validators = validators(blockchain);
    if validators.is_empty() {
        return Address::ZERO;
    }
    let round_zero_index = if blockchain.height() == 1 {
        0
    } else {
        let previous_proposer = &blockchain.head().header.proposer;
        validators
            .iter()
            .position(|v| v == previous_proposer)
            .map(|idx| idx + 1)
            .unwrap_or(0)
    };
    validators[(round_zero_index + round as usize) % validators.len()]
}

/// Multicasts a message to a set of recipients.
pub fn multicast(_recipients: &[Address], message: QbftMessage) -> Vec<QbftMessageWithRecipient> {
    // Multicast is done externally.
    vec![QbftMessageWithRecipient {
        message: message.clone(),
        recipient: Address::ZERO,
    }]

    // recipients
    //     .iter()
    //     .map(|recipient| QbftMessageWithRecipient {
    //         message: message.clone(),
    //         recipient: recipient.clone(),
    //     })
    //     .collect()
}

/// Computes the digest of an optional block.
pub fn _digest_optional_block(block: &Optional<Block>) -> Optional<Hash> {
    block.as_ref().map(digest)
}

// =======================================================================
// PROPOSAL AND ROUND-CHANGE FUNCTIONS
// =======================================================================

/// Returns the set of all Prepare messages included in node's received messages.
pub fn _received_prepares(current: &NodeState) -> Vec<&SignedPrepare> {
    current
        .messages_received()
        .iter()
        .filter_map(|msg| {
            if let QbftMessage::Prepare(Prepare { prepare_payload: p }) = msg {
                Some(p)
            } else {
                None
            }
        })
        .collect()
}

/// Extracts signed prepares from a set of QBFT messages.
pub fn extract_signed_prepares<'a>(messages: &'a [&'a Prepare]) -> Vec<&'a SignedPrepare> {
    messages.iter().map(|msg| &msg.prepare_payload).collect()
}

/// Returns the set of all RoundChange messages included in node's received messages.
pub fn _received_round_changes(current: &NodeState) -> Vec<&RoundChange> {
    current
        .messages_received()
        .iter()
        .filter_map(|msg| {
            if let QbftMessage::RoundChange(rc) = msg {
                Some(rc)
            } else {
                None
            }
        })
        .collect()
}

/// Returns received signed round changes for current height and future rounds.
pub fn received_signed_round_changes_for_current_height_and_future_rounds(
    current: &NodeState,
) -> Vec<&SignedRoundChange> {
    current
        .messages_received()
        .iter()
        .filter_map(|msg| {
            if let QbftMessage::RoundChange(rc) = msg {
                if rc.round_change_payload.unsigned_payload.height == current.blockchain().height()
                    && rc.round_change_payload.unsigned_payload.round > current.round()
                {
                    Some(&rc.round_change_payload)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Extracts signed round changes from a set of QBFT messages.
/// AT Note: This seems wrong as it ignores the height.
pub fn extract_signed_round_changes<'a>(
    messages: &'a [&'a RoundChange],
) -> Vec<&'a SignedRoundChange> {
    messages
        .iter()
        .map(|msg| &msg.round_change_payload)
        .collect()
}

/// Gets the set of round change senders.
pub fn get_set_of_round_change_senders<'a>(
    round_changes: &'a [&'a SignedRoundChange],
) -> std::collections::HashSet<Address> {
    round_changes
        .iter()
        .map(|rc| recover_signed_round_change_author(rc))
        .collect()
}

/// Returns the set of all blocks included in RoundChange messages.
///
/// This function extracts blocks from the `proposed_block_for_next_round` field
/// of RoundChange messages.
///
/// Translation of receivedBlocksInRoundChanges function from Dafny.
pub fn received_blocks_in_round_changes<'a>(messages: &'a [&'a RoundChange]) -> Vec<&'a Block> {
    let mut res: Vec<_> = messages
        .iter()
        .filter_map(|round_change| round_change.proposed_block_for_next_round.as_ref())
        .collect();
    res.sort();
    res.dedup();
    res
}

/// Validates that a block is valid for a Proposal message for round that does not carry any
/// RoundChange Justification under the assumption that the current blockchain is blockchain.
/// Translation of validateNonPreparedBlock predicate from Dafny.
pub fn validate_non_prepared_block(block: &Block, blockchain: &Blockchain, round: u64) -> bool {
    // First check: block.header.proposer == proposer(round, blockchain)
    block.header.proposer == proposer(round, blockchain)
        // Second check: |validators(blockchain + [block])| > 0
        && !block.header.validators.is_empty()
}

/// Validates that sPayload is a valid signed RoundChange payload for height, round under the
/// assumption that the current set of validators is validators.
/// Translation of validRoundChange predicate from Dafny.
pub fn valid_round_change(
    s_payload: &SignedRoundChange,
    height: u64,
    round: u64,
    validators: &[Address],
) -> bool {
    let u_payload = &s_payload.unsigned_payload;

    // First check: uPayload.height == height
    u_payload.height == height
        // Second check: uPayload.round == round
        && u_payload.round == round
        // Third check: complex conditional logic for prepared round/value
        && {
            if u_payload.prepared_round.is_none() && u_payload.prepared_value.is_none() {
                // Both are None - valid case
                true
            } else if u_payload.prepared_round.is_some() && u_payload.prepared_value.is_some() {
                // Both are Some - check that prepared round < current round
                u_payload.prepared_round.expect("prepared_round is Some per branch guard") < round
            } else {
                // One is Some and one is None - invalid case
                false
            }
        }
        // Fourth check: recoverSignedRoundChangeAuthor(sPayload) in validators
        && validators.contains(&recover_signed_round_change_author(s_payload))
}

/// Checks if sPayload is one of the signed RoundChange payloads with the highest round in the set
/// roundChanges. Translation of isHighestPrepared predicate from Dafny.
pub fn is_highest_prepared(
    s_payload: &SignedRoundChange,
    round_changes: &[&SignedRoundChange],
) -> bool {
    // First check: optionIsPresent(sPayload.unsignedPayload.preparedRound)
    s_payload.unsigned_payload.prepared_round.is_some()
        // Second check: optionIsPresent(sPayload.unsignedPayload.preparedValue)
        && s_payload.unsigned_payload.prepared_value.is_some()
        // Third check: forall m | m in roundChanges :: optionIsPresent(m.unsignedPayload.
        // preparedRound) ==> optionGet(m.unsignedPayload.preparedRound) <= optionGet(
        // sPayload.unsignedPayload.preparedRound)
        && round_changes.iter().all(|m| {
            if let Some(m_prepared_round) = m.unsigned_payload.prepared_round {
                if let Some(s_prepared_round) = s_payload.unsigned_payload.prepared_round {
                    m_prepared_round <= s_prepared_round
                } else {
                    false // This case shouldn't happen due to first check, but being defensive
                }
            } else {
                true // If m doesn't have a prepared round, the implication is vacuously true
            }
        })
}

/// This function is called in two ways:
///
/// 1) In upon_proposal -> is_valid_proposal with parameters from the proposal message
///    - round_changes: the set of round changes included in the proposal
///    - prepares: the set of prepares included in the proposal
///    - set_of_available_blocks: the single block included in the proposal
///    - round: the round included in the proposal
///
/// 2) In upon_round_change -> has_received_proposal_justification with parameters from the node
///    state
///   - round_changes: the set of round changes received by the node for current height and future
///     rounds
///   - prepares: the set of prepares received by the node for current height and future rounds
///   - set_of_available_blocks: the proposed_block_for_next_round field of round change messages
///     received by the node for current height and future rounds
///
/// In IBFT, this is:
/// 3: predicate JustifyPrePrepare(〈PRE-PREPARE, λi, round, value〉)
/// 4: return
///       round = 1
///       ∨ received a quorum Qrc of valid 〈ROUND-CHANGE, λi, round, prj , pvj 〉 messages
///          such that:
///             ∀〈ROUND-CHANGE, λi, round, prj , pvj 〉 ∈Qrc : prj = ⊥∧prj = ⊥
///             ∨ received a quorum of valid 〈PREPARE, λi, pr, value〉messages such that:
///                (pr, value) = HighestPrepared(Qrc)
#[allow(clippy::too_many_arguments)]
pub fn is_proposal_justification<'a>(
    round_changes: &'a [&'a SignedRoundChange],
    prepares: &'a [&'a SignedPrepare],
    set_of_available_blocks: &[&Block],
    height: u64,
    round: u64,
    block: &Block,
    validate_block: impl Fn(&Block) -> bool,
    round_leader: &Address,
    validators: &[Address],
    blockchain: &Blockchain,
) -> bool {
    // debug!(target: "qbft", "is_proposal_justification for round {} block={:?}", round, block);

    if round == 0 {
        // Round 0 case. We don't need supporting prepares or round changes.
        let success = validate_block(block)
            && block.header.round_number == 0
            && block.header.height == height
            && block.header.proposer == *round_leader;
        if !success && block.header.height != height {
            debug!(
                target: "qbft",
                "is_proposal_justification: block height {} does not match expected height {}",
                block.header.height,
                height
            );
        }
        debug!(target: "qbft", "is_proposal_justification: round 0 {success}");
        success
    } else {
        // Round > 0 case
        let round_change_senders = get_set_of_round_change_senders(round_changes);
        let quorum_size = quorum(validators.len());

        // Check quorum of round change senders
        // Note that messages are pre-filtered to only include those from validators.
        if round_change_senders.len() < quorum_size {
            warn!(
                target: "qbft",
                "is_proposal_justification: round change quorum miss height {} round {} \
                 have {} need {} senders=[{}] validators=[{}]",
                height,
                round,
                round_change_senders.len(),
                quorum_size,
                format_addresses(round_change_senders.iter().copied()),
                format_addresses(validators.iter().copied()),
            );
            return false;
        }

        // Check all round changes are valid
        if !round_changes
            .iter()
            .all(|m| valid_round_change(m, height, round, validators))
        {
            debug!(target: "qbft", "is_proposal_justification: fail valid round changes");
            return false;
        }

        let prepared_support_quorum = f(validators.len()) + 1;
        let mut prepared_support: HashMap<u64, Vec<&SignedRoundChange>> = HashMap::new();
        for rc in round_changes.iter().copied() {
            let (Some(prepared_round), Some(prepared_value)) = (
                rc.unsigned_payload.prepared_round,
                rc.unsigned_payload.prepared_value,
            ) else {
                continue;
            };

            let expected_proposer = proposer(prepared_round, blockchain);
            let mut prepared_block = replace_round_in_block(block, prepared_round);
            prepared_block.header.proposer = expected_proposer;
            if digest(&prepared_block) != prepared_value {
                continue;
            }

            prepared_support.entry(prepared_round).or_default().push(rc);
        }

        let highest_prepared_round = prepared_support
            .iter()
            .filter_map(|(round, changes)| {
                let senders = get_set_of_round_change_senders(changes);
                if senders.len() < prepared_support_quorum {
                    return None;
                }
                Some(*round)
            })
            .max();

        if let Some(prepared_round) = highest_prepared_round {
            // Note that strangely, there is no check for the proposer here in the Dafny spec.

            // Check if we have:
            // received a quorum of valid 〈PREPARE, λi, pr, value〉messages such that:
            //                (pr, value) = HighestPrepared(Qrc)
            // Some round changes have prepared values - need to validate with highest prepared
            if !set_of_available_blocks
                .iter()
                .any(|b| b.round_sort_key() == block.round_sort_key())
            {
                debug!(
                    target: "qbft",
                    "is_proposal_justification: fail block {block:?} not available"
                );
                return false;
            }

            // if block.header.round_number != round || block.header.height != height {
            if block.header.height != height {
                debug!(
                    target: "qbft",
                    "is_proposal_justification: fail block {block:?} round/height {:?}/{:?}",
                    (block.header.round_number, block.header.height),
                    (round, height)
                );
                return false;
            }

            let matching_round_changes = prepared_support
                .get(&prepared_round)
                .cloned()
                .unwrap_or_default();

            let matching_senders = get_set_of_round_change_senders(&matching_round_changes);
            if matching_senders.len() < quorum_size {
                warn!(
                    target: "qbft",
                    "is_proposal_justification: prepared round change quorum miss height {} \
                     round {} prepared_round {} have {} need {} senders=[{}]",
                    height,
                    round,
                    prepared_round,
                    matching_senders.len(),
                    quorum_size,
                    format_addresses(matching_senders.iter().copied()),
                );
                return false;
            }
            let prepare_senders: HashSet<Address> = prepares
                .iter()
                .map(|pm| recover_signed_prepare_author(pm))
                .collect();
            if prepare_senders.len() < quorum_size {
                warn!(
                    target: "qbft",
                    "is_proposal_justification: prepare quorum miss height {} round {} \
                     prepared_round {} have {} need {} senders=[{}]",
                    height,
                    round,
                    prepared_round,
                    prepare_senders.len(),
                    quorum_size,
                    format_addresses(prepare_senders.iter().copied()),
                );
                return false;
            }

            // Check exists rcm in roundChanges such that conditions hold
            let success = round_changes.iter().any(|rcm| {
                if rcm.unsigned_payload.prepared_round != Some(prepared_round) {
                    debug!(target: "qbft", "is_proposal_justification: fail prepared round match");
                    return false;
                }

                if let (Some(msg_prepared_round), Some(prepared_value)) = (
                    rcm.unsigned_payload.prepared_round,
                    rcm.unsigned_payload.prepared_value,
                ) {
                    let expected_proposer = proposer(msg_prepared_round, blockchain);
                    let mut proposed_block_with_old_round =
                        replace_round_in_block(block, msg_prepared_round);
                    proposed_block_with_old_round.header.proposer = expected_proposer;

                    if prepared_value != digest(&proposed_block_with_old_round) {
                        debug!(target: "qbft", "is_proposal_justification: fail prepared value");
                        return false;
                    }

                    // Check all prepares are valid for this height, round, and digest
                    prepares.iter().all(|pm| {
                        valid_signed_prepare_for_height_round_and_digest(
                            pm,
                            height,
                            msg_prepared_round,
                            &prepared_value,
                            validators,
                        )
                    })
                } else {
                    debug!(target: "qbft", "is_proposal_justification: fail highest prepared");
                    false
                }
            });
            if !success {
                debug!(target: "qbft", "is_proposal_justification: fail final check");
            } else {
                debug!(target: "qbft", "is_proposal_justification: successful");
            }
            success
        } else {
            // ∀〈ROUND-CHANGE, λi, round, prj , pvj 〉 ∈Qrc : prj = ⊥∧prj = ⊥
            // No round changes have prepared values - simple validation
            // In this case we expect the block to be a new one as no other block is available.
            let success = validate_block(block)
                && block.header.round_number == round
                && block.header.height == height
                && block.header.proposer == *round_leader;
            if !success {
                debug!(target: "qbft", "is_proposal_justification: fail all null prepared");
            }

            success
        }
    }
}

/// Validates a proposal message.
///
///    predicate isValidProposal(m:QbftMessage, current:NodeState)
///    requires validNodeState(current)
///    {
///        && m.Proposal?
///        && var roundLeader :=
/// proposer(m.proposalPayload.unsignedPayload.round,current.blockchain());        && m.
/// proposalPayload.unsignedPayload.height == |current.blockchain()|
///        && recoverSignedProposalAuthor(m.proposalPayload) == roundLeader
///        && isProposalJustification(
///            m.proposalJustification,
///            m.roundChangeJustification,
///            {m.proposedBlock},
///            |current.blockchain()|,
///            m.proposalPayload.unsignedPayload.round,
///            m.proposedBlock,
///            b =>
/// validateNonPreparedBlock(b,current.blockchain(),m.proposalPayload.unsignedPayload.round),
///            roundLeader,
///            validators(current.blockchain())
///        )
///        // NOTE: This check is not required by the QBFT paper as the message structure is a bit
/// different        && digest(m.proposedBlock) == m.proposalPayload.unsignedPayload.digest
///        && (
///            || (
///                && !optionIsPresent(current.proposalAcceptedForCurrentRound)
///                && m.proposalPayload.unsignedPayload.round == current.round()
///            )
///            || (
///                && optionIsPresent(current.proposalAcceptedForCurrentRound)
///                && m.proposalPayload.unsignedPayload.round > current.round()
///            )
///        )
///    }
pub fn is_valid_proposal(m: &Proposal, current: &NodeState) -> bool {
    // Check that the proposal height matches current blockchain length

    // && m.proposalPayload.unsignedPayload.height == |current.blockchain()|
    if m.proposal_payload.unsigned_payload.height != current.blockchain().height() {
        debug!(target: "qbft", "Invalid proposal height");
        return false;
    }

    let round = m.proposal_payload.unsigned_payload.round;

    // Check that the proposer is correct for this round

    // && var roundLeader := proposer(m.proposalPayload.unsignedPayload.round,current.blockchain());
    let round_leader = proposer(round, current.blockchain());

    // && recoverSignedProposalAuthor(m.proposalPayload) == roundLeader
    if recover_signed_proposal_author(&m.proposal_payload) != round_leader {
        debug!(target: "qbft", "Invalid proposal author");
        return false;
    }

    // && isProposalJustification(
    //     m.proposalJustification,
    //     m.roundChangeJustification,
    //     {m.proposedBlock},
    //     |current.blockchain()|,
    //     m.proposalPayload.unsignedPayload.round,
    //     m.proposedBlock,
    //     b => validateNonPreparedBlock(b,current.blockchain(),m.proposalPayload.unsignedPayload.
    // round),     roundLeader,
    //     validators(current.blockchain())
    // )
    let proposal_justification_refs: Vec<&SignedRoundChange> =
        m.proposal_justification.iter().collect();
    let round_change_justification_refs: Vec<&SignedPrepare> =
        m.round_change_justification.iter().collect();
    if !is_proposal_justification(
        &proposal_justification_refs,
        &round_change_justification_refs,
        &[&m.proposed_block],
        current.blockchain().height(),
        m.proposal_payload.unsigned_payload.round,
        &m.proposed_block,
        |b| {
            validate_non_prepared_block(
                b,
                current.blockchain(),
                m.proposal_payload.unsigned_payload.round,
            )
        },
        &round_leader,
        validators(current.blockchain()),
        current.blockchain(),
    ) {
        debug!(target: "qbft", "Invalid proposal justification");
        return false;
    }

    // Check that the digest matches the proposed block
    // NOTE: This check is not required by the QBFT paper as the message structure is a bit
    // different && digest(m.proposedBlock) == m.proposalPayload.unsignedPayload.digest
    if m.proposal_payload.unsigned_payload.digest != digest(&m.proposed_block) {
        debug!(target: "qbft", "Invalid proposal digest");
        return false;
    }

    let pround = m.proposal_payload.unsigned_payload.round;
    let cround = current.round();
    if current.proposal_accepted_for_current_round().is_none() {
        // No currently accepted proposal.
        // We will only accept a proposal for the current round only
        if pround != cround {
            debug!(
                target: "qbft",
                "Invalid proposal proposal round={pround} != current round={cround} \
                 (proposal must be for this round)",
            );
            return false;
        }
    } else {
        // We already have a proposal for this round.
        // We will accept a proposal for a higher round only
        if pround <= cround {
            debug!(
                target: "qbft",
                "Invalid proposal proposal round={pround} <= current round={cround} \
                 (already have a proposal)",
            );
            return false;
        }
    }
    true
}

/// Returns the Proposal Justification that a QBFT node with state current has received
///
/// There should be only one.
///
/// The original function used inference to find the subsets of round changes, available blocks, and
/// prepares. Here, we explicitly filter the messages to find valid subsets.
///
/// Consider this case: [0 h=85 bt=1 rt=-1 chain=83/166,84/168 r=1 prva=110
/// in=85.0:Pppppcc+85.1:rrr] The node has received a proposal for round 0 with justification but
/// the commit messages did not arrive in time. The node is now in round 1 and has received round
/// change messages for round 1.
pub fn has_received_proposal_justification<'a>(
    current: &'a NodeState,
) -> Option<ProposalJustification<'a>> {
    // By the round condition check in is_received_proposal_justification
    // the round must be greater than the current round.
    let mut rounds = current
        .messages_received()
        .iter()
        .map(QbftMessage::round)
        .filter(|&r| is_proposal_justification_round_valid(r, current))
        .collect::<Vec<_>>();
    rounds.dedup();

    if rounds.is_empty() {
        return None;
    }

    debug!(
        target: "qbft",
        "has_received_proposal_justification rounds: {:?}",
        rounds
    );

    for new_round in rounds {
        // This test is not in the Dafny, but it makes sense.
        if proposer(new_round, current.blockchain()) != current.id() {
            debug!(
                target: "qbft",
                "Skipping round {} as not proposer",
                new_round
            );
            continue;
        }

        // Prefilter messages for valid round changes and prepares at this height and round.
        let round_changes = valid_round_changes(current, new_round);
        let available_blocks = received_blocks_in_round_changes(&round_changes);

        let highest_prepared_round = round_changes
            .iter()
            .filter(|rc| {
                rc.round_change_payload
                    .unsigned_payload
                    .prepared_value
                    .is_some()
            })
            .filter_map(|rc| rc.round_change_payload.unsigned_payload.prepared_round)
            .max();

        let prepares = if let Some(highest_prepared_round) = highest_prepared_round {
            // If we have any prepared round, we need to find the matching prepares that generated
            // it.
            valid_prepares(current, highest_prepared_round)
        } else {
            // All null prepared case.
            vec![]
        };

        debug!(
            target: "qbft",
            ?available_blocks,
            ?highest_prepared_round,
            "has_received_proposal_justification {} prepares",
            prepares.len()
        );

        // J2: The justification has a quorum of valid 〈PREPARE, λi, pr, pv〉
        //     messages such that 〈ROUND-CHANGE, λi, r, pr, pv〉
        //     is the message with the highest prepared round different than ⊥ in Qrc
        for new_block in available_blocks {
            if is_received_proposal_justification(
                &round_changes,
                &prepares,
                new_round,
                new_block,
                current,
            ) {
                debug!(
                    target: "qbft",
                    "Found round change proposal justification for round {} with block \
                     {:?}",
                    new_round,
                    new_block
                );

                return Some(ProposalJustification {
                    round: new_round,
                    round_changes: round_changes.to_vec(),
                    prepares: prepares.to_vec(),
                    block: new_block.clone(),
                });
            }
        }

        // J1: The justification has no prepared values (null case).

        // Try with our own proposed block.
        // This should match validate_block_fn in is_proposal_justification.
        let Some(new_block) = current.get_new_block(new_round) else {
            error!(target: "qbft", "No new block found for round {}", new_round);
            continue;
        };

        debug!(
            target: "qbft",
            "Our block for round {}: {:?}",
            new_round,
            new_block
        );

        // Try with each block proposed in round changes. For example for the round 0 case.
        if is_received_proposal_justification(
            &round_changes,
            &prepares,
            new_round,
            &new_block,
            current,
        ) {
            debug!(
                target: "qbft",
                "Found new block proposal justification for round {} with our block {:?}",
                new_round,
                new_block
            );
            return Some(ProposalJustification {
                round: new_round,
                round_changes,
                prepares,
                block: new_block,
            });
        }
    }

    None
}

/// Filter for valid round changes.
/// This is a precondition of is_received_proposal_justification.
fn valid_round_changes(current: &NodeState, new_round: u64) -> Vec<&RoundChange> {
    current
        .messages_received_at_round(new_round)
        .iter()
        .filter_map(|msg| {
            if let QbftMessage::RoundChange(rc) = msg {
                if valid_round_change(
                    &rc.round_change_payload,
                    current.blockchain().height(),
                    new_round,
                    current.blockchain().validators(),
                ) {
                    Some(rc)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
}

fn valid_prepares(current: &NodeState, new_round: u64) -> Vec<&Prepare> {
    current
        .messages_received_at_round(new_round)
        .iter()
        .filter_map(|msg| {
            if let QbftMessage::Prepare(p) = msg {
                if p.prepare_payload.unsigned_payload.height == current.blockchain().height()
                    && p.prepare_payload.unsigned_payload.round == new_round
                {
                    Some(p)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
}

/// Checks if roundChanges and prepares are valid Proposal and RoundChange justifications
/// for a Proposal message with round newRound including block under the assumption
/// that the current QBFT node state is current.
///
/// Translation of isReceivedProposalJustification predicate from Dafny.
pub fn is_received_proposal_justification(
    round_changes: &[&RoundChange],
    prepares: &[&Prepare],
    new_round: u64,
    new_block: &Block,
    current: &NodeState,
) -> bool {
    // Note: round_changes and prepares are subsets of messages received by current.
    // For now we take the entire set of received messages.
    let signed_round_changes = extract_signed_round_changes(round_changes);
    let mut signed_prepares = extract_signed_prepares(prepares);
    let height = current.blockchain().height();
    for rc in round_changes {
        for signed_prepare in &rc.round_change_justification {
            if signed_prepare.unsigned_payload.height == height
                && signed_prepare.unsigned_payload.round == new_round
            {
                signed_prepares.push(signed_prepare);
            }
        }
    }
    let available_blocks = received_blocks_in_round_changes(round_changes);

    // For round 0 and no prepared values, we don't need supporting prepares.
    // Do not require the block to match our local proposed block; under load, txpools
    // can diverge and this strict equality can prevent round-change progress.
    let validate_block_fn =
        |b: &Block| validate_non_prepared_block(b, current.blockchain(), new_round);
    let round_leader = proposer(new_round, current.blockchain());

    if !is_proposal_justification(
        &signed_round_changes,
        &signed_prepares,
        &available_blocks,
        current.blockchain().height(),
        new_round,
        new_block,
        validate_block_fn,
        &round_leader,
        validators(current.blockchain()),
        current.blockchain(),
    ) {
        return false;
    }

    // Fourth check: round conditions
    is_proposal_justification_round_valid(new_round, current)
}

fn is_proposal_justification_round_valid(new_round: u64, current: &NodeState) -> bool {
    if current.proposal_accepted_for_current_round().is_none() {
        // No proposal accepted for current round
        new_round == current.round()
    } else {
        // Proposal accepted for current round
        new_round > current.round()
    }
}

/// Returns the RoundChange Justification that a QBFT node with state current has received
/// since adopting its current blockchain, or the empty set if no Round Change Justification
/// has been received since.
/// Translation of getRoundChangeJustification function from Dafny.
pub fn get_round_change_justification(current: &NodeState) -> Vec<&SignedPrepare> {
    if current.last_prepared_block().is_none() {
        return vec![];
    }

    // Find a quorum of valid prepares for the last prepared block
    let last_prepared_round = current
        .last_prepared_round()
        .expect("last_prepared_round is Some per early return guard");
    let last_prepared_block = current
        .last_prepared_block()
        .expect("last_prepared_block is Some per early return guard");
    let block_digest = digest(last_prepared_block);
    let validators_list = validators(current.blockchain());

    let valid_prepares = valid_prepares_for_height_round_and_digest(
        current.blockchain().height(),
        last_prepared_round,
        &block_digest,
        current.messages_received(),
    );

    // Check if we have a quorum
    if valid_prepares.len() >= quorum(validators_list.len()) {
        valid_prepares
    } else {
        vec![]
    }
}

/// Creates a round change message.
/// The RoundChange message that a QBFT node with state `current` would send to move to round
/// `newRound`.
pub fn create_round_change(current: &NodeState, new_round: u64) -> RoundChange {
    let round_change_payload = sign_round_change(
        &UnsignedRoundChange {
            height: current.blockchain().height(),
            round: new_round,
            prepared_value: current.last_prepared_block().map(digest),
            prepared_round: current.last_prepared_round(),
        },
        current,
    );

    RoundChange {
        round_change_payload,
        proposed_block_for_next_round: current.last_prepared_block().cloned(),
        round_change_justification: get_round_change_justification(current)
            .into_iter()
            .cloned()
            .collect(),
    }
}

// =======================================================================
// PREPARE VALIDATION FUNCTIONS
// =======================================================================

/// Validates that sPayload is a valid signed Prepare payload for height, round, block digest
/// under the assumption that the set of validators is validators.
/// Translation of validSignedPrepareForHeightRoundAndDigest predicate from Dafny.
pub fn valid_signed_prepare_for_height_round_and_digest(
    s_payload: &SignedPrepare,
    height: u64,
    round: u64,
    digest: &Hash,
    validators: &[Address],
) -> bool {
    let u_payload = &s_payload.unsigned_payload;

    // First check: uPayload.height == height
    u_payload.height == height
        // Second check: uPayload.round == round
        && u_payload.round == round
        // Third check: uPayload.digest == digest
        && u_payload.digest == *digest
        // Fourth check: recoverSignedPrepareAuthor(sPayload) in validators
        && validators.contains(&recover_signed_prepare_author(s_payload))
}

/// Returns valid prepares for height, round, and digest.
pub fn valid_prepares_for_height_round_and_digest<'a>(
    height: u64,
    round: u64,
    digest: &Hash,
    messages: &'a [QbftMessage],
) -> Vec<&'a SignedPrepare> {
    // TODO: optimize by using partioning as messages are sorted by height and round.
    // TODO: return &SignedPrepare instead of SignedPrepare to avoid cloning.
    messages
        .iter()
        .filter_map(|msg| {
            if let QbftMessage::Prepare(Prepare { prepare_payload: p }) = msg {
                let u_payload = &p.unsigned_payload;
                if u_payload.height == height
                    && u_payload.round == round
                    && u_payload.digest == *digest
                {
                    Some(p)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Returns valid QBFT Prepare messages for height, round, and digest.
/// Translation of validPreparesForHeightRoundAndDigest function from Dafny.
pub fn valid_prepares_for_height_round_and_digest_messages<'a>(
    messages_received: &'a [QbftMessage],
    height: u64,
    round: u64,
    digest: &Hash,
    validators: &[Address],
) -> Vec<&'a QbftMessage> {
    messages_received
        .iter()
        .filter(|msg| {
            if let QbftMessage::Prepare(Prepare {
                prepare_payload: s_payload,
            }) = msg
            {
                valid_signed_prepare_for_height_round_and_digest(
                    s_payload, height, round, digest, validators,
                )
            } else {
                false
            }
        })
        .collect()
}

// =======================================================================
// COMMIT VALIDATION FUNCTIONS
// =======================================================================

/// Validates a commit message.
pub fn validate_commit(
    commit: &SignedCommit,
    height: u64,
    round: u64,
    proposed_block: &Block,
    validators: &[Address],
) -> bool {
    let u_payload = &commit.unsigned_payload;
    u_payload.height == height
        && u_payload.round == round
        && u_payload.digest == digest(proposed_block)
        && validators.contains(&recover_signed_commit_author(commit))
}

/// Returns valid commits for height, round, and digest.
pub fn valid_commits_for_height_round_and_digest<'a>(
    height: u64,
    round: u64,
    digest: &Hash,
    messages: &'a [QbftMessage],
) -> Vec<&'a SignedCommit> {
    messages
        .iter()
        .filter_map(|msg| {
            if let QbftMessage::Commit(commit) = msg {
                let u_payload = &commit.commit_payload.unsigned_payload;
                if u_payload.height == height
                    && u_payload.round == round
                    && u_payload.digest == *digest
                {
                    Some(&commit.commit_payload)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

// =======================================================================
// NEW-BLOCK VALIDATION FUNCTIONS
// =======================================================================

/// Validates a new block.
pub fn valid_new_block(blockchain: &Blockchain, new_block: &SignedNewBlock) -> bool {
    let block = &new_block.block;
    let payload = &new_block.unsigned_payload;
    let block_digest = digest(block);
    let seal_hash = hash_block_for_commit_seal(block);
    let current_height = blockchain.height();
    let validators_list = validators(blockchain);
    let quorum_size = quorum(validators_list.len());

    if payload.height != block.header.height {
        warn!(
            target: "qbft",
            "Invalid new block payload height {} != block height {}",
            payload.height,
            block.header.height
        );
        return false;
    }
    if payload.round != block.header.round_number {
        warn!(
            target: "qbft",
            "Invalid new block payload round {} != block round {}",
            payload.round,
            block.header.round_number
        );
        return false;
    }
    if payload.digest != block_digest {
        warn!(
            target: "qbft",
            "Invalid new block payload digest {} != block digest {}",
            payload.digest,
            block_digest
        );
        return false;
    }

    let height_ok = block.header.height >= current_height;
    if !height_ok {
        warn!(
            target: "qbft",
            "Invalid new block height {} < current height {}",
            block.header.height,
            current_height
        );
    }

    let body_digest_ok = block.validate_digest();
    if !body_digest_ok {
        let computed_body_digest = alloy_primitives::keccak256(block.body());
        warn!(
            target: "qbft",
            "Invalid new block body digest header {} != computed {}",
            block.header.digest,
            computed_body_digest
        );
    }

    // An adversary could send repeated commit seals.
    // But if we count unique ones, then we can prevent that attack.
    let unique_seal_authors: Vec<_> = block
        .header
        .commit_seals
        .iter()
        .map(|seal| seal.author())
        .sorted()
        .dedup()
        .collect();
    let number_of_unique_seals = unique_seal_authors.len();
    let total_seals = block.header.commit_seals.len();
    if number_of_unique_seals < quorum_size {
        warn!(
            target: "qbft",
            "Invalid new block commit seals: {} total, {} unique < quorum {} \
             (height {} round {} authors {:?})",
            total_seals,
            number_of_unique_seals,
            quorum_size,
            block.header.height,
            block.header.round_number,
            unique_seal_authors
        );
    }

    let mut seals_ok = true;
    for (idx, seal) in block.header.commit_seals.iter().enumerate() {
        let author = seal.author();
        let is_validator = validators_list.contains(&author);
        let sig_ok = seal.verify_message(seal_hash.as_slice()).unwrap_or(false);
        if !is_validator || !sig_ok {
            warn!(
                target: "qbft",
                "Invalid commit seal idx {} author {} validator={} sig_ok={} seal_hash {}",
                idx,
                author,
                is_validator,
                sig_ok,
                seal_hash
            );
            seals_ok = false;
        }
    }

    // Not in the spec.
    // Allow catch-up. If a node is behind or a NewBlock message is lost, advance the chain.
    height_ok && body_digest_ok && number_of_unique_seals >= quorum_size && seals_ok
}

/// Validates a new block message.
pub fn valid_new_block_message(blockchain: &Blockchain, msg: &QbftMessage) -> bool {
    match msg {
        QbftMessage::NewBlock(new_block) => valid_new_block(blockchain, new_block),
        _ => false,
    }
}

// =======================================================================
// HELPER FUNCTIONS (not part of the spec)
// =======================================================================

/// Returns the minimum round from round changes.
pub fn min_round(round_changes: &[&SignedRoundChange]) -> u64 {
    round_changes
        .iter()
        .map(|rc| rc.unsigned_payload.round)
        .min()
        .unwrap_or(0)
}

// =======================================================================
// NON-SPECIFICATION FUNCTIONS
//
// This section defines those functions that are used in the specification
// but that are used for verification purposes and not specification
// purposes.
// =======================================================================

/// Validates that a NodeState satisfies all required invariants.
/// Translation of validNodeState predicate from Dafny.
pub fn valid_node_state(node_state: &NodeState) -> bool {
    // First check: (optionIsPresent(nodeState.proposalAcceptedForCurrentRound) ==>
    // optionGet(nodeState.proposalAcceptedForCurrentRound).Proposal?)
    let proposal_check = true; // Since we store Proposal directly, this check is always true.

    // Second check: (!optionIsPresent(nodeState.lastPreparedRound) <==>
    // !optionIsPresent(nodeState.lastPreparedBlock))
    let prepared_consistency =
        node_state.last_prepared_round().is_none() == node_state.last_prepared_block().is_none();

    if !prepared_consistency {
        error!(target: "qbft", "NodeState invariant failed: prepared round/block consistency");
    }

    // Third check: (optionIsPresent(nodeState.lastPreparedRound) ==> exists QofP :: ...)
    let prepared_quorum_check = if let (Some(prepared_round), Some(prepared_block)) = (
        node_state.last_prepared_round(),
        node_state.last_prepared_block(),
    ) {
        let block_digest = digest(prepared_block);
        let validators_list = validators(node_state.blockchain());
        let valid_prepare_messages = valid_prepares_for_height_round_and_digest_messages(
            node_state.messages_received_at_round(prepared_round),
            node_state.blockchain().height(),
            prepared_round,
            &block_digest,
            validators_list,
        );

        // Check that there exists a quorum of valid prepares (modeling existential quantification)
        valid_prepare_messages.len() >= quorum(validators_list.len())
    } else {
        true // If no prepared round/block, this check passes
    };

    if !prepared_quorum_check {
        error!(target: "qbft", "NodeState invariant failed: prepared quorum check");
    }

    // Fourth check: StateBlockchainInvariant(nodeState.blockchain)
    let blockchain_invariant = state_blockchain_invariant(node_state.blockchain());

    if !blockchain_invariant {
        error!(target: "qbft", "NodeState invariant failed: blockchain invariant");
    }

    proposal_check && prepared_consistency && prepared_quorum_check && blockchain_invariant
}

/// Validates that a Blockchain satisfies required invariants.
/// Translation of StateBlockchainInvariant predicate from Dafny.
pub fn state_blockchain_invariant(blockchain: &Blockchain) -> bool {
    // First check: |blockchain| > 0
    if blockchain.height() == 0 {
        return false;
    }

    // Second check: |blockchain| > 1 ==> blockchain[|blockchain|-1].header.proposer in
    // validators(blockchain[..|blockchain|-1])
    let proposer_check = if blockchain.height() > 1 {
        // Get the last block's proposer
        let last_block = blockchain.head();
        let last_proposer = &last_block.header.proposer;

        let prev_validators = &blockchain.blocks()[blockchain.blocks().len() - 2]
            .header
            .validators;

        // Check that the last block's proposer is in the previous validators set
        prev_validators.contains(last_proposer)
    } else {
        true // Only one block, so this implication is vacuously true
    };

    if !proposer_check {
        error!(target: "qbft", "Blockchain invariant failed: proposer check");
    }

    // Third check: |validators(blockchain)| > 0
    let validators_non_empty = !validators(blockchain).is_empty();

    if !validators_non_empty {
        error!(target: "qbft", "Blockchain invariant failed: validators non-empty");
    }

    proposer_check && validators_non_empty
}
