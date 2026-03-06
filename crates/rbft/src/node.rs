// SPDX-License-Identifier: Apache-2.0
//! QBFT Specification predicates and functions translated from Dafny node.dfy.
//!
//! Each of the upon_<event_name> functions specifies how the state of a QBFT node
//! should evolve and which messages should be transmitted when the event <event_name>
//! occurs on the next local clock tick.

use tracing::{debug, info, trace, warn};

use crate::{
    node_auxilliary_functions::*,
    types::{
        Commit, NodeState, Prepare, Proposal, QbftMessage, QbftMessageWithRecipient, Signature,
        SignedCommit, UnsignedCommit, UnsignedPrepare, UnsignedProposal,
    },
};

/// Try UponBlockTimeout transition
/// Algorithm 2 IBFT pseudocode for process pi: normal case operation
/// 11: procedure Start(λ, value)
/// 12: λi ←λ
/// 13: ri ←1
/// 14: pri ←⊥
/// 15: pvi ←⊥
/// 16: inputValuei ←value
/// 17: if leader(hi, ri) = pi then
/// 18: broadcast 〈PRE-PREPARE, λi, ri, inputValuei〉 message
/// 19: set timeri to running and expire after t(ri)
pub fn upon_block_timeout(current: &mut NodeState) -> Option<Vec<QbftMessageWithRecipient>> {
    if !upon_block_timeout_ready(current) {
        return None;
    }

    // The proposed block must be set externally before calling node_next().
    let new_block = current.get_new_block(0)?;

    let digest = digest(&new_block);
    let proposal = Proposal {
        proposal_payload: sign_proposal(
            &UnsignedProposal {
                height: current.blockchain().height(),
                round: 0,
                digest,
            },
            current,
        ),
        proposed_block: new_block,
        proposal_justification: vec![],
        round_change_justification: vec![],
    };

    // Update state to prevent triggering again
    // Don't add to messages_received to avoid triggering upon_proposal immediately
    current.set_proposal_accepted_for_current_round(Some(proposal.clone()));

    // In IBFT but not QBFT. Without this we get immediate round timeouts.
    current.set_time_last_round_start(current.local_time());

    let proposal = QbftMessage::Proposal(proposal);

    // This is not in the spec, but without it, we have too few prepares.
    let prepare = QbftMessage::Prepare(Prepare {
        prepare_payload: sign_prepare(
            &UnsignedPrepare {
                height: current.blockchain().height(),
                round: 0,
                digest,
            },
            current,
        ),
    });

    debug!(
        target: "qbft",
        "upon_block_timeout: proposing block at height {} round 0",
        current.blockchain().height(),
    );

    let mut out = multicast(validators(current.blockchain()), proposal);
    out.extend(multicast(validators(current.blockchain()), prepare));
    Some(out)
}

/// Returns true if the block timeout is ready and we are the proposer for round 0.
/// Returns false if a proposal has already been sent.
fn upon_block_timeout_ready(current: &NodeState) -> bool {
    current.round() == 0
        && proposer(0, current.blockchain()) == current.id()
        && current.local_time() >= current.next_block_timeout()
        && current.proposal_accepted_for_current_round().is_none()
}

/// Try UponProposal transition
/// Algorithm 2 IBFT pseudocode for process pi: normal case operation
/// 1: upon receiving a valid 〈PRE-PREPARE, λi, ri, value〉 message m from leader(λi,round)
///    such that JustifyPrePrepare(m) do
/// 2: set timeri to running and expire after t(ri)
/// 3: broadcast 〈PREPARE, λi, ri, value〉
pub fn upon_proposal(current: &mut NodeState) -> Option<Vec<QbftMessageWithRecipient>> {
    for m in current.messages_received() {
        if let QbftMessage::Proposal(proposal) = m {
            if !is_valid_proposal(proposal, current) {
                continue;
            }

            let new_round = proposal.proposal_payload.unsigned_payload.round;
            let prepare = QbftMessage::Prepare(Prepare {
                prepare_payload: sign_prepare(
                    &UnsignedPrepare {
                        height: current.blockchain().height(),
                        round: new_round,
                        digest: digest(&proposal.proposed_block),
                    },
                    current,
                ),
            });

            // Update state
            current.set_proposal_accepted_for_current_round(Some(proposal.clone()));
            if new_round > current.round() {
                current.set_time_last_round_start(current.local_time())
            };
            current.set_round(new_round);
            current.push_message(prepare.clone());

            return Some(multicast(validators(current.blockchain()), prepare));
        }
    }
    None
}

/// Try UponPrepare transition
/// Algorithm 2 IBFT pseudocode for process pi: normal case operation  
/// 4: upon receiving a quorum of valid 〈PREPARE, λi, ri, value〉 messages do
/// 5: pri ← ri
/// 6: pvi ← value
/// 7: broadcast 〈COMMIT, λi, ri, value〉
pub fn upon_prepare(current: &mut NodeState) -> Option<Vec<QbftMessageWithRecipient>> {
    if let Some(prop) = current.proposal_accepted_for_current_round() {
        let proposal_digest = digest(&prop.proposed_block);
        let valid_prepares = valid_prepares_for_height_round_and_digest(
            current.blockchain().height(),
            current.round(),
            &proposal_digest,
            current.messages_received(),
        );
        let quorum_size = quorum(validators(current.blockchain()).len());
        let mut prepare_authors: Vec<_> = valid_prepares
            .iter()
            .map(|prepare| recover_signed_prepare_author(prepare))
            .collect();
        prepare_authors.sort();
        prepare_authors.dedup();
        trace!(
            target: "qbft",
            "upon_prepare: valid prepares {}/{} for height {} round {} \
             digest {} authors {:?}",
            valid_prepares.len(),
            quorum_size,
            current.blockchain().height(),
            current.round(),
            proposal_digest,
            prepare_authors
        );

        if valid_prepares.len() < quorum_size {
            trace!(
                target: "qbft",
                "upon_prepare: not enough valid prepares: {}/{} for height {} round {}",
                valid_prepares.len(),
                quorum_size,
                current.blockchain().height(),
                current.round(),
            );
            return None;
        }

        // Check if we haven't already sent a commit for this round
        let has_sent_commit = current.messages_received().iter().any(|m| {
            if let QbftMessage::Commit(commit) = m {
                commit.commit_payload.unsigned_payload.height == current.blockchain().height()
                    && commit.commit_payload.unsigned_payload.round == current.round()
                    && recover_signed_commit_author(&commit.commit_payload) == current.id()
            } else {
                false
            }
        });

        if has_sent_commit {
            trace!(
                target: "qbft",
                "has already sent commit for height {} round {}",
                current.blockchain().height(),
                current.round(),
            );
            return None;
        }

        let commit = QbftMessage::Commit(Commit {
            commit_payload: sign_commit(
                &UnsignedCommit {
                    height: current.blockchain().height(),
                    round: current.round(),
                    commit_seal: sign_hash(
                        &hash_block_for_commit_seal(&prop.proposed_block),
                        current,
                    ),
                    digest: digest(&prop.proposed_block),
                },
                current,
            ),
        });

        // Update state
        current.set_last_prepared_block_and_round(prop.proposed_block.clone(), current.round());
        current.push_message(commit.clone());

        return Some(multicast(validators(current.blockchain()), commit));
    }
    None
}

/// Try UponCommit transition
/// Algorithm 2 IBFT pseudocode for process pi: normal case operation
/// 8: upon receiving a quorum Qcommit of valid 〈COMMIT, λi, round, value〉 messages do
/// 9: set timeri to stopped
/// 10: Decide(λi, value, Qcommit)
pub fn upon_commit(current: &mut NodeState) -> Option<Vec<QbftMessageWithRecipient>> {
    let can_update_the_blockchain = current.is_the_proposer_for_current_round();

    if let Some(prop) = current.proposal_accepted_for_current_round() {
        trace!(
            target: "qbft",
            "upon_commit: can_update_the_blockchain={can_update_the_blockchain}",
        );
        if can_update_the_blockchain {
            let proposal_digest = digest(&prop.proposed_block);
            let valid_commits = valid_commits_for_height_round_and_digest(
                current.blockchain().height(),
                current.round(),
                &proposal_digest,
                current.messages_received(),
            );
            let quorum_size = quorum(validators(current.blockchain()).len());
            let mut commit_authors: Vec<_> = valid_commits
                .iter()
                .map(|commit| recover_signed_commit_author(commit))
                .collect();
            commit_authors.sort();
            commit_authors.dedup();

            let unique_commit_authors = commit_authors.len();
            if unique_commit_authors < quorum_size {
                trace!(
                    target: "qbft",
                    "upon_commit: below quorum commits {}/{} unique from {} total \
                     for height {} round {}",
                    unique_commit_authors,
                    quorum_size,
                    valid_commits.len(),
                    current.blockchain().height(),
                    current.round(),
                );
                return None;
            }

            trace!(
                target: "qbft",
                "upon_commit: committing block at height {} round {}",
                current.blockchain().height(),
                current.round(),
            );

            // Create new block with commit seals and add to blockchain
            let commit_seals = get_commit_seals_from_commit_messages(&valid_commits);
            let mut seal_authors: Vec<_> = commit_seals.iter().map(|seal| seal.author()).collect();
            seal_authors.sort();
            seal_authors.dedup();
            trace!(
                target: "qbft",
                "upon_commit: commit seals total {} unique {} authors {:?}",
                commit_seals.len(),
                seal_authors.len(),
                seal_authors
            );
            let mut new_block = prop.proposed_block.clone();
            new_block.header.commit_seals = commit_seals.into_iter().cloned().collect();
            let seal_hash = hash_block_for_commit_seal(&new_block);
            trace!(
                target: "qbft",
                "upon_commit: new block height {} round {} digest {} seal_hash {}",
                new_block.header.height,
                new_block.header.round_number,
                proposal_digest,
                seal_hash
            );

            // Set time_last_round_start to current local time to ensure nodes have a full
            // round timeout period to participate in round 0 of the new height.
            // Previously, using min(block_timestamp + block_time, local_time) could cause
            // immediate round timeouts if consensus took longer than block_time.

            current.blockchain_mut().set_head(new_block.clone());
            current.blockchain_mut().set_proposed_block(None);
            let time_last_round_start = current.local_time().max(current.next_block_timeout());
            current.new_round(0, time_last_round_start, None);

            // Create NewBlock message for follower nodes
            let new_block_msg = QbftMessage::NewBlock(sign_new_block(&new_block, current));

            // Make sure we get the newblock message directly.
            current.push_message(new_block_msg.clone());
            return Some(multicast(validators(current.blockchain()), new_block_msg));
        }
    }
    None
}

/// Try UponNewBlock transition
pub fn upon_new_block(current: &mut NodeState) -> Option<Vec<QbftMessageWithRecipient>> {
    if let Some(new_block_msg) = current
        // This includes future messages.
        .all_messages_received()
        .iter()
        .rev()
        .find_map(|msg| {
            if let QbftMessage::NewBlock(new_block) = msg {
                if valid_new_block(current.blockchain(), new_block) {
                    return Some(new_block.clone());
                }
            }
            None
        })
    {
        // Update state with new block
        let new_block = new_block_msg.block.clone();
        current.blockchain_mut().set_head(new_block);
        current.blockchain_mut().set_proposed_block(None);
        trace!(
            target: "qbft",
            "upon_new_block: added new block at height {}",
            current.blockchain().height(),
        );

        // Set time_last_round_start to current local time to ensure nodes have a full
        // round timeout period to participate in round 0 of the new height.
        // Also, make sure we don't get a round timeout before the next block is produced.
        let time_last_round_start = current.local_time().max(current.next_block_timeout());
        current.new_round(0, time_last_round_start, None);
        current.prune_messages_below_height();
        return Some(vec![]);
    }
    None
}

/// Try UponRoundTimeout transition
/// Algorithm 2 IBFT pseudocode for process pi: round change
/// 1: upon timeri is expired do
/// 2: ri ←ri + 1
/// 3: set timeri to running and expire after t(ri)
/// 4: broadcast 〈ROUND-CHANGE, λi, ri, pri, pvi〉
pub fn upon_round_timeout(current: &mut NodeState) -> Option<Vec<QbftMessageWithRecipient>> {
    // If round_change_on_first_block is false, prevent round change timeout on height 1
    if !current
        .configuration()
        .round_change_config
        .round_change_on_first_block
        && current.blockchain().height() == 1
    {
        return None;
    }

    if current.local_time() >= current.next_round_timeout() {
        let timed_out_round = current.round();
        if timed_out_round == 0 && defer_round_zero_timeout(current) {
            return None;
        }
        let expected_proposer = proposer(timed_out_round, current.blockchain());
        let proposal_seen = current.proposal_accepted_for_current_round().is_some();
        let timeout_deadline = current.next_round_timeout();
        let timeout_overdue = current.local_time().saturating_sub(timeout_deadline);
        if !proposal_seen {
            if expected_proposer == current.id() {
                warn!(
                    target: "qbft",
                    "round timeout at height {} round {} while we are proposer; \
                     no proposal seen (overdue {} ms)",
                    current.blockchain().height(),
                    timed_out_round,
                    timeout_overdue
                );
            } else {
                warn!(
                    target: "qbft",
                    "round timeout at height {} round {}; \
                     expected proposer {:?} did not propose (overdue {} ms)",
                    current.blockchain().height(),
                    timed_out_round,
                    expected_proposer,
                    timeout_overdue
                );
            }
        }

        let new_round = current.round() + 1;
        let round_change = create_round_change(current, new_round);

        // This is not in the spec, but we need to reset the round start time to avoid immediate
        // timeouts. The IBFT spec does this. Is there any reason why QBFT does not?
        current.set_time_last_round_start(current.local_time());

        // Update state
        current.set_round(new_round);
        current.set_proposal_accepted_for_current_round(None);
        current.push_message(QbftMessage::RoundChange(round_change.clone()));
        info!(
            target: "qbft",
            "{}: upon_round_timeout: moving to round {} at height {}.",
            current.label(),
            new_round,
            current.blockchain().height(),
        );

        return Some(multicast(
            validators(current.blockchain()),
            QbftMessage::RoundChange(round_change),
        ));
    }
    None
}

fn defer_round_zero_timeout(current: &mut NodeState) -> bool {
    let Some((prepare_count, commit_count)) = round_zero_progress_counts(current) else {
        return false;
    };

    let validators_len = validators(current.blockchain()).len();
    if validators_len == 0 {
        return false;
    }

    let progress_threshold = f(validators_len) + 1;
    let has_progress = prepare_count >= progress_threshold || commit_count > 0;
    if !has_progress {
        return false;
    }

    let progressed = current.update_round_zero_progress(prepare_count, commit_count);
    let grace_secs = current.configuration().block_time.max(1);
    let since_progress = current
        .local_time()
        .saturating_sub(current.round_zero_last_progress_time());

    if progressed || since_progress < grace_secs {
        current.set_time_last_round_start(current.local_time());
        debug!(
            target: "qbft",
            "{}: suppressing round change at height {} round 0; prepares={} commits={} \
             threshold={} since_progress={}s",
            current.label(),
            current.blockchain().height(),
            prepare_count,
            commit_count,
            progress_threshold,
            since_progress,
        );
        return true;
    }

    false
}

/// Try UponRoundChange transition
/// As I understand it:
///    If we have enough round change and prepare messages for a certain round, we send a proposal
/// with    the messages attached.
/// Only the leader of the new round should do this.
pub fn upon_round_change(current: &mut NodeState) -> Option<Vec<QbftMessageWithRecipient>> {
    // First branch: Check if we have received proposal justification for leading round
    if let Some(justification) = has_received_proposal_justification(current) {
        // 11: upon receiving a quorum Qrc of valid 〈ROUND-CHANGE, λi, ri, −, −〉 messages such
        //     that leader(λi, ri) = pi ∧ JustifyRoundChange(Qrc) do
        // 12: if HighestPrepared(Qrc) != ⊥ then
        // 13:     let v such that (−, v) = HighestPrepared(Qrc))
        // 14: else
        // 15:     let v such that v = inputValue
        // 16: broadcast 〈PRE-PREPARE, λi, ri, v〉
        let new_round = justification.round;
        let mut proposal_block = replace_round_in_block(&justification.block, new_round);
        proposal_block.header.proposer = proposer(new_round, current.blockchain());
        debug!(
            target: "qbft",
            "{}: upon_round_change has_received_proposal_justification \
             for round {new_round}.",
            current.label()
        );

        // Create proposal with justifications
        let proposal = QbftMessage::Proposal(Proposal {
            proposal_payload: sign_proposal(
                &UnsignedProposal {
                    height: current.blockchain().height(),
                    round: new_round,
                    digest: digest(&proposal_block),
                },
                current,
            ),
            proposed_block: proposal_block,
            proposal_justification: extract_signed_round_changes(&justification.round_changes)
                .into_iter()
                .cloned()
                .collect(),
            round_change_justification: extract_signed_prepares(&justification.prepares)
                .into_iter()
                .cloned()
                .collect(),
        });

        // Update state
        if new_round > current.round() {
            current.set_time_last_round_start(current.local_time())
        };
        current.set_round(new_round);
        current.set_proposal_accepted_for_current_round(None);
        current.push_message(proposal.clone());

        Some(multicast(validators(current.blockchain()), proposal))
    } else {
        // Second branch: Check for f+1 round changes for future rounds
        //
        // IBFT spec:
        // 5: upon receiving a set Frc of f + 1 valid 〈ROUND-CHANGE, λi, rj , −, −〉 messages such
        //    that ∀〈ROUND-CHANGE, λi, rj , −, −〉 ∈Frc : rj > ri do
        // 6: let 〈ROUND-CHANGE, hi, rmin, −, −〉 ∈Frc such that:
        // 7:      ∀〈ROUND-CHANGE, λi, rj , −, −〉 ∈Frc : rmin ≤rj
        // 8: ri ←rmin
        // 9: set timeri to running and expire after t(ri)
        // 10: broadcast 〈ROUND-CHANGE, λi, ri, pri, pvi〉

        // QBFT diverges from IBFT here.
        let use_ibft_method = false;
        if use_ibft_method {
            // IBFT checks each future round individually.
            let future_rounds = current.get_future_rounds_from_round_changes();

            // Note: This follows the IBFT method more closely than the Dafny spec,
            for new_round in future_rounds {
                let round_changes = current.received_signed_round_changes_for_round(new_round);
                let senders = get_set_of_round_change_senders(&round_changes);
                if senders.len() > f(validators(current.blockchain()).len()) {
                    info!(
                        target: "qbft",
                        "{}: upon_round_change: received {} round changes >= f+1 \
                         for future round {new_round}.",
                        current.label(),
                        senders.len(),
                    );

                    // Find the minimum round from the round changes (equivalent to minSet in Dafny)
                    let round_change = create_round_change(current, new_round);

                    // Update state
                    current.set_round(new_round);
                    // QBFT does not do this.
                    current.set_time_last_round_start(current.local_time());
                    current.set_proposal_accepted_for_current_round(None);
                    current.push_message(QbftMessage::RoundChange(round_change.clone()));

                    return Some(multicast(
                        validators(current.blockchain()),
                        QbftMessage::RoundChange(round_change),
                    ));
                }
            }
        } else {
            // QBFT checks all the round changes together.
            let round_changes =
                received_signed_round_changes_for_current_height_and_future_rounds(current);
            let senders = get_set_of_round_change_senders(&round_changes);

            if !senders.is_empty() {
                trace!(
                    target: "qbft",
                    "{}: upon_round_change: received {}/{} senders for future rounds.",
                    current.label(),
                    senders.len(),
                    f(validators(current.blockchain()).len()) + 1
                );
            }

            // More senders than maximum adversaries.
            if senders.len() > f(validators(current.blockchain()).len()) {
                // Find the minimum round from the round changes (equivalent to minSet in Dafny)
                let new_round = min_round(&round_changes);
                let round_change = create_round_change(current, new_round);

                // Update state
                current.set_round(new_round);
                current.set_proposal_accepted_for_current_round(None);
                current.push_message(QbftMessage::RoundChange(round_change.clone()));

                info!(
                    target: "qbft",
                    "{}: upon_round_change: received {} round changes >= f+1 \
                     for future round {new_round}.",
                    current.label(),
                    senders.len(),
                );

                return Some(multicast(
                    validators(current.blockchain()),
                    QbftMessage::RoundChange(round_change),
                ));
            }
        }

        None
    }
}

// =======================================================================
// NODE STATE TRANSITION FUNCTIONS
// =======================================================================

/// Main node state transition function.
/// Updates local time and executes sub-steps.
pub fn node_next(current: &mut NodeState, time: u64) -> Vec<QbftMessageWithRecipient> {
    // Update local time and add incoming messages
    current.set_local_time(time);
    node_next_sub_step(current)
}

/// Executes the next sub-step for the node.
/// Follows the NodeNextSubStep predicate from the Dafny specification.
pub fn node_next_sub_step(current: &mut NodeState) -> Vec<QbftMessageWithRecipient> {
    // Note that it is possible for multiple transitions to be possible.
    if validators(current.blockchain()).contains(&current.id()) {
        // First check for new blocks as commits do not advance the chain.
        if let Some(messages) = upon_new_block(current) {
            return messages;
        };
        // If we are a validator we execute this block.
        // Try each possible transition from NodeNextSubStep
        if let Some(messages) = upon_block_timeout(current) {
            messages
        } else if let Some(messages) = upon_proposal(current) {
            messages
        } else if let Some(messages) = upon_prepare(current) {
            messages
        } else if let Some(messages) = upon_commit(current) {
            messages
        } else if let Some(messages) = upon_round_change(current) {
            messages
        } else {
            upon_round_timeout(current).unwrap_or_default()
        }
    } else {
        // Non-validator nodes can only process new blocks
        upon_new_block(current).unwrap_or_default()
    }
}

// =======================================================================
// HELPER FUNCTIONS
// =======================================================================

/// Gets commit seals from commit messages.
/// Translation of getCommitSealsFromCommitMessages function from Dafny.
pub fn get_commit_seals_from_commit_messages<'a>(
    commits: &'a [&SignedCommit],
) -> Vec<&'a Signature> {
    commits
        .iter()
        .map(|commit| &commit.unsigned_payload.commit_seal)
        .collect()
}
