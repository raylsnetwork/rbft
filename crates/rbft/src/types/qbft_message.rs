// SPDX-License-Identifier: Apache-2.0
use alloy_rlp::{BufMut, Decodable, Encodable};
use itertools::Itertools;
use tracing::debug;

use super::{
    Address, BlockRequest, BlockResponse, Blockchain, Commit, Prepare, Proposal, RoundChange,
    SignedNewBlock,
};
use crate::node_auxilliary_functions::{
    proposer, recover_signed_commit_author, recover_signed_new_block_author,
    recover_signed_prepare_author, recover_signed_proposal_author,
    recover_signed_round_change_author,
};

// pub type QbftMessage = rbft_net::QbftMessage;
#[derive(Clone, Debug, PartialEq)]
pub enum QbftMessage {
    Proposal(Proposal),
    Prepare(Prepare),
    Commit(Commit),
    RoundChange(RoundChange),
    NewBlock(SignedNewBlock),
    /// Request blocks from a peer (for block backfill/sync)
    BlockRequest(BlockRequest),
    /// Response containing requested blocks
    BlockResponse(BlockResponse),
}

impl QbftMessage {
    // Check that any embedded block's body hash matches its header digest.
    pub(crate) fn check_digest(&self) -> bool {
        match self {
            QbftMessage::Proposal(proposal) => proposal.proposed_block.validate_digest(),
            QbftMessage::Prepare(_prepare) => true,
            QbftMessage::Commit(_commit) => true,
            QbftMessage::RoundChange(round_change) => round_change
                .proposed_block_for_next_round
                .as_ref()
                .is_none_or(|block| block.validate_digest()),
            QbftMessage::NewBlock(new_block) => new_block.block.validate_digest(),
            // Sync messages: validate all embedded blocks
            QbftMessage::BlockRequest(_) => true,
            QbftMessage::BlockResponse(response) => {
                response.blocks.iter().all(|b| b.block.validate_digest())
            }
        }
    }
}

impl Encodable for QbftMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        // Not strictly correct as we should wrap this in a list.
        match self {
            QbftMessage::Proposal(proposal) => {
                0_u8.encode(out);
                proposal.encode(out);
            }
            QbftMessage::Prepare(prepare) => {
                1_u8.encode(out);
                prepare.encode(out);
            }
            QbftMessage::Commit(commit) => {
                2_u8.encode(out);
                commit.encode(out);
            }
            QbftMessage::RoundChange(round_change) => {
                3_u8.encode(out);
                round_change.encode(out);
            }
            QbftMessage::NewBlock(new_block) => {
                4_u8.encode(out);
                new_block.encode(out);
            }
            QbftMessage::BlockRequest(request) => {
                5_u8.encode(out);
                request.encode(out);
            }
            QbftMessage::BlockResponse(response) => {
                6_u8.encode(out);
                response.encode(out);
            }
        }
    }
}

impl Decodable for QbftMessage {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let message_type: u8 = u8::decode(buf)?;
        match message_type {
            0 => Ok(QbftMessage::Proposal(Decodable::decode(buf)?)),
            1 => Ok(QbftMessage::Prepare(Decodable::decode(buf)?)),
            2 => Ok(QbftMessage::Commit(Decodable::decode(buf)?)),
            3 => Ok(QbftMessage::RoundChange(Decodable::decode(buf)?)),
            4 => Ok(QbftMessage::NewBlock(Decodable::decode(buf)?)),
            5 => Ok(QbftMessage::BlockRequest(Decodable::decode(buf)?)),
            6 => Ok(QbftMessage::BlockResponse(Decodable::decode(buf)?)),
            _ => Err(alloy_rlp::Error::Custom("Unknown QbftMessage type")),
        }
    }
}

impl QbftMessage {
    const PROPOSAL: u8 = 0;
    const PREPARE: u8 = 1;
    const COMMIT: u8 = 2;
    const ROUND_CHANGE: u8 = 3;
    const NEW_BLOCK: u8 = 4;
    const BLOCK_REQUEST: u8 = 5;
    const BLOCK_RESPONSE: u8 = 6;

    /// Establish a sort order, so we can find messages of a certain kind quickly and deduplicate
    /// messages. Height, round, message type, author
    ///
    /// Note: BlockRequest/BlockResponse are sync messages without height/round/author,
    /// so they use the requested height range and Address::ZERO for sorting.
    pub fn components(&self) -> (u64, u64, u8, Address) {
        match self {
            QbftMessage::Proposal(prop) => (
                prop.proposal_payload.unsigned_payload.height,
                prop.proposal_payload.unsigned_payload.round,
                QbftMessage::PROPOSAL,
                self.author(),
            ),
            QbftMessage::Prepare(prep) => (
                prep.prepare_payload.unsigned_payload.height,
                prep.prepare_payload.unsigned_payload.round,
                QbftMessage::PREPARE,
                self.author(),
            ),
            QbftMessage::Commit(comm) => (
                comm.commit_payload.unsigned_payload.height,
                comm.commit_payload.unsigned_payload.round,
                QbftMessage::COMMIT,
                self.author(),
            ),
            QbftMessage::RoundChange(rc) => (
                rc.round_change_payload.unsigned_payload.height,
                rc.round_change_payload.unsigned_payload.round,
                QbftMessage::ROUND_CHANGE,
                self.author(),
            ),
            QbftMessage::NewBlock(new_block) => (
                new_block.unsigned_payload.height,
                new_block.unsigned_payload.round,
                QbftMessage::NEW_BLOCK,
                self.author(),
            ),
            // Sync messages use requested height range for sorting
            QbftMessage::BlockRequest(req) => (
                req.from_height,
                req.to_height,
                QbftMessage::BLOCK_REQUEST,
                Address::ZERO,
            ),
            QbftMessage::BlockResponse(resp) => {
                let height = resp
                    .blocks
                    .first()
                    .map(|b| b.unsigned_payload.height)
                    .unwrap_or(0);
                (height, 0, QbftMessage::BLOCK_RESPONSE, Address::ZERO)
            }
        }
    }

    /// Return the author of the message (which validator).
    /// Note: BlockRequest/BlockResponse are unsigned sync messages and return Address::ZERO.
    pub fn author(&self) -> Address {
        match self {
            QbftMessage::Proposal(prop) => recover_signed_proposal_author(&prop.proposal_payload),
            QbftMessage::Prepare(prep) => recover_signed_prepare_author(&prep.prepare_payload),
            QbftMessage::Commit(comm) => recover_signed_commit_author(&comm.commit_payload),
            QbftMessage::RoundChange(rc) => {
                recover_signed_round_change_author(&rc.round_change_payload)
            }
            QbftMessage::NewBlock(block) => recover_signed_new_block_author(&block.signature),
            // Sync messages are unsigned, no author
            QbftMessage::BlockRequest(_) | QbftMessage::BlockResponse(_) => Address::ZERO,
        }
    }

    /// Return the height of the message.
    /// Note: BlockRequest returns from_height, BlockResponse returns first block's height or 0.
    pub fn height(&self) -> u64 {
        match self {
            QbftMessage::Proposal(prop) => prop.proposal_payload.unsigned_payload.height,
            QbftMessage::Prepare(prep) => prep.prepare_payload.unsigned_payload.height,
            QbftMessage::Commit(comm) => comm.commit_payload.unsigned_payload.height,
            QbftMessage::RoundChange(rc) => rc.round_change_payload.unsigned_payload.height,
            QbftMessage::NewBlock(block) => block.unsigned_payload.height,
            QbftMessage::BlockRequest(req) => req.from_height,
            QbftMessage::BlockResponse(resp) => resp
                .blocks
                .first()
                .map(|b| b.unsigned_payload.height)
                .unwrap_or(0),
        }
    }

    /// Return the round number of the message.
    /// Note: BlockRequest/BlockResponse return 0 as they are not round-specific.
    pub fn round(&self) -> u64 {
        match self {
            QbftMessage::Proposal(prop) => prop.proposal_payload.unsigned_payload.round,
            QbftMessage::Prepare(prep) => prep.prepare_payload.unsigned_payload.round,
            QbftMessage::Commit(comm) => comm.commit_payload.unsigned_payload.round,
            QbftMessage::RoundChange(rc) => rc.round_change_payload.unsigned_payload.round,
            QbftMessage::NewBlock(block) => block.unsigned_payload.round,
            QbftMessage::BlockRequest(_) | QbftMessage::BlockResponse(_) => 0,
        }
    }

    /// Return the index of the author in the given validator set, if present.
    pub fn author_index(&self, validators: &[Address]) -> Option<usize> {
        let author = self.author();
        validators.iter().position(|id| *id == author)
    }

    /// Check all signatures in a QbftMessage by calling verify_message on each signature
    /// with the RLP payload of the corresponding unsigned message.
    ///
    /// This method performs comprehensive signature verification for QBFT messages:
    /// - For Proposal messages: verifies the proposal signature plus all justification signatures
    /// - For Prepare messages: verifies the prepare signature
    /// - For Commit messages: verifies the commit signature
    /// - For RoundChange messages: verifies the round change signature plus justifications
    /// - For NewBlock messages: verifies the NewBlock signature and expected proposer
    ///
    /// Returns Ok(true) if all signatures are valid, Ok(false) if any signature is invalid,
    /// or Err if there's an error during verification.
    ///
    /// # Usage with existing validation
    /// This method can be used alongside the `check_authors` method for complete
    /// message validation:
    ///
    /// ```rust,ignore
    /// // Complete message validation
    /// fn validate_qbft_message(
    ///     msg: &QbftMessage,
    ///     validators: &[Address],
    ///     blockchain: &Blockchain,
    /// ) -> bool {
    ///     // First check if authors are valid validators
    ///     if !msg.check_authors(validators) {
    ///         return false;
    ///     }
    ///
    ///     // Then verify all cryptographic signatures
    ///     match msg.check_signatures(blockchain) {
    ///         Ok(true) => true,
    ///         Ok(false) | Err(_) => false,
    ///     }
    /// }
    /// ```
    pub fn check_signatures(
        &self,
        blockchain: &Blockchain,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        match self {
            QbftMessage::Proposal(proposal) => {
                // Check the main proposal signature
                let unsigned_proposal_rlp =
                    alloy_rlp::encode(&proposal.proposal_payload.unsigned_payload);
                if !proposal
                    .proposal_payload
                    .signature
                    .verify_message(&unsigned_proposal_rlp)?
                {
                    debug!(
                        target: "qbft",
                        "Invalid proposal signature from author {}",
                        self.author()
                    );
                    return Ok(false);
                }

                // Check all proposal justification signatures (if any)
                for signed_round_change in &proposal.proposal_justification {
                    let unsigned_rc_rlp = alloy_rlp::encode(&signed_round_change.unsigned_payload);
                    if !signed_round_change
                        .signature
                        .verify_message(&unsigned_rc_rlp)?
                    {
                        debug!(
                            target: "qbft",
                            "Invalid round change justification signature from author {}",
                            signed_round_change.signature.author()
                        );
                        return Ok(false);
                    }
                }

                // Check all round change justification signatures (if any)
                for signed_prepare in &proposal.round_change_justification {
                    let unsigned_prep_rlp = alloy_rlp::encode(&signed_prepare.unsigned_payload);
                    if !signed_prepare
                        .signature
                        .verify_message(&unsigned_prep_rlp)?
                    {
                        debug!(
                            target: "qbft",
                            "Invalid prepare justification signature from author {}",
                            signed_prepare.signature.author()
                        );
                        return Ok(false);
                    }
                }
            }

            QbftMessage::Prepare(prepare) => {
                let unsigned_prepare_rlp =
                    alloy_rlp::encode(&prepare.prepare_payload.unsigned_payload);
                if !prepare
                    .prepare_payload
                    .signature
                    .verify_message(&unsigned_prepare_rlp)?
                {
                    debug!(
                        target: "qbft",
                        "Invalid prepare signature from author {}",
                        self.author()
                    );
                    return Ok(false);
                }
            }

            QbftMessage::Commit(commit) => {
                let unsigned_commit_rlp =
                    alloy_rlp::encode(&commit.commit_payload.unsigned_payload);
                if !commit
                    .commit_payload
                    .signature
                    .verify_message(&unsigned_commit_rlp)?
                {
                    debug!(
                        target: "qbft",
                        "Invalid commit signature from author {}",
                        self.author()
                    );
                    return Ok(false);
                }
            }

            QbftMessage::RoundChange(round_change) => {
                // Check the main round change signature
                let unsigned_rc_rlp =
                    alloy_rlp::encode(&round_change.round_change_payload.unsigned_payload);
                if !round_change
                    .round_change_payload
                    .signature
                    .verify_message(&unsigned_rc_rlp)?
                {
                    debug!(
                        target: "qbft",
                        "Invalid round change signature from author {}",
                        self.author()
                    );
                    return Ok(false);
                }

                // Check all round change justification signatures (if any)
                for signed_prepare in &round_change.round_change_justification {
                    let unsigned_prep_rlp = alloy_rlp::encode(&signed_prepare.unsigned_payload);
                    if !signed_prepare
                        .signature
                        .verify_message(&unsigned_prep_rlp)?
                    {
                        debug!(
                            target: "qbft",
                            "Invalid prepare justification signature from author {}",
                            signed_prepare.signature.author()
                        );
                        return Ok(false);
                    }
                }
            }

            QbftMessage::NewBlock(block) => {
                let unsigned_payload = &block.unsigned_payload;

                // Check the NewBlock signature itself.
                let unsigned_new_block_rlp = alloy_rlp::encode(unsigned_payload);
                if !block.signature.verify_message(&unsigned_new_block_rlp)? {
                    debug!(
                        target: "qbft",
                        "Invalid NewBlock signature from author {}",
                        block.signature.author()
                    );
                    return Ok(false);
                }

                let expected_proposer = proposer(block.block.header.round_number, blockchain);
                if block.signature.author() != expected_proposer
                    || block.block.header.proposer != expected_proposer
                {
                    return Ok(false);
                }
            }

            // Sync messages: BlockRequest has no signatures, BlockResponse contains
            // signed blocks that should be verified individually by the handler
            QbftMessage::BlockRequest(_) => {}
            QbftMessage::BlockResponse(response) => {
                // Verify signatures on all blocks in the response
                for signed_block in &response.blocks {
                    let unsigned_new_block_rlp = alloy_rlp::encode(&signed_block.unsigned_payload);
                    if !signed_block
                        .signature
                        .verify_message(&unsigned_new_block_rlp)?
                    {
                        debug!(
                            target: "qbft",
                            "Invalid BlockResponse block signature from author {}",
                            signed_block.signature.author()
                        );
                        return Ok(false);
                    }

                    let expected_proposer =
                        proposer(signed_block.block.header.round_number, blockchain);
                    if signed_block.signature.author() != expected_proposer
                        || signed_block.block.header.proposer != expected_proposer
                    {
                        debug!(
                            target: "qbft",
                            "BlockResponse block proposer mismatch"
                        );
                        return Ok(false);
                    }
                }
            }
        }

        Ok(true)
    }

    /// Validate that a message's author is a legitimate validator
    ///
    /// This method checks if the message's author(s) are valid validators:
    /// - For consensus messages (Proposal, Prepare, Commit, RoundChange): author must be a
    ///   validator
    /// - For NewBlock messages: signer and proposer must be validators, and all commit seal authors
    ///   must be validators
    ///
    /// # Arguments
    /// * `validators` - A slice of valid validator addresses
    ///
    /// # Returns
    /// * `true` if all authors in the message are valid validators
    /// * `false` if any author is not a valid validator
    pub fn check_authors(&self, validators: &[Address]) -> bool {
        let author = self.author();

        match self {
            QbftMessage::Proposal(_)
            | QbftMessage::Prepare(_)
            | QbftMessage::Commit(_)
            | QbftMessage::RoundChange(_) => {
                // For consensus messages, author must be a validator
                if !validators.contains(&author) {
                    debug!(target: "qbft", "Message author {} is not a validator.", author);
                    return false;
                }
            }
            QbftMessage::NewBlock(block) => {
                // For NewBlock messages, ensure the signer and proposer are validators,
                // and validate commit seals if present.
                if !validators.contains(&author) {
                    debug!(target: "qbft", "NewBlock signer {} is not a validator.", author);
                    return false;
                }

                if !validators.contains(&block.block.header.proposer) {
                    debug!(
                        target: "qbft",
                        "NewBlock proposer {} is not a validator.",
                        block.block.header.proposer
                    );
                    return false;
                }
            }

            // Sync messages: BlockRequest has no author to validate
            // BlockResponse blocks should have their proposers validated
            QbftMessage::BlockRequest(_) => {}
            QbftMessage::BlockResponse(response) => {
                for signed_block in &response.blocks {
                    if !validators.contains(&signed_block.block.header.proposer) {
                        debug!(
                            target: "qbft",
                            "BlockResponse block proposer {} is not a validator.",
                            signed_block.block.header.proposer
                        );
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// Enable sorting of QbftMessage by (height, round, message type, author)
///
/// This means that each validator can send at most one message of each type.
impl PartialOrd for QbftMessage {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.components().cmp(&other.components()))
    }
}

pub fn summarise_messages<'a>(mr: impl Iterator<Item = &'a QbftMessage>, quorum: usize) -> String {
    let mut m = String::with_capacity(100);
    let mut plus = "";
    for (h, msgs) in &mr.chunk_by(|a| a.height()) {
        for (r, msgs) in &msgs.chunk_by(|a| a.round()) {
            m.push_str(format!("{plus}{}.{}:", h, r).as_str());
            plus = "+";

            // Group consecutive messages of the same type
            let msgs_vec: Vec<_> = msgs.collect();
            let mut i = 0;
            while i < msgs_vec.len() {
                let msg_code = msg_error_code(msgs_vec[i]);
                m.push_str(msg_code);

                // Count consecutive messages of the same type
                let mut count = 1;
                while i + count < msgs_vec.len() && msg_error_code(msgs_vec[i + count]) == msg_code
                {
                    m.push_str(msg_code);
                    count += 1;
                }

                // Add "!" if we met or exceeded quorum
                if count >= quorum {
                    m.push('!');
                }

                i += count;
            }
        }
    }
    m
}

pub fn msg_error_code(msg: &QbftMessage) -> &str {
    match msg {
        QbftMessage::Proposal(_) => "P",
        QbftMessage::Prepare(_) => "p",
        QbftMessage::Commit(_) => "c",
        QbftMessage::RoundChange(r)
            if r.round_change_payload
                .unsigned_payload
                .prepared_round
                .is_some() =>
        {
            "R"
        }
        QbftMessage::RoundChange(_) => "r",
        QbftMessage::NewBlock(_) => "b",
        QbftMessage::BlockRequest(_) => "Q", // Q for Query/request
        QbftMessage::BlockResponse(_) => "A", // A for Answer/response
    }
}
