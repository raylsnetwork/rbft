// SPDX-License-Identifier: Apache-2.0
//! Types defined in doc/qbft-spec/types.dfy
//!
//! Do not put impls here, put them in node.rs

use alloy_rlp::{BufMut, Decodable, Encodable, Header, RlpDecodable, RlpEncodable};

// =======================================================================
// UNDEFINED TYPES
// =======================================================================

// TODO: Make a types trait with these types.

pub type Address = alloy_primitives::Address;
pub type BlockBody = alloy_primitives::Bytes;
pub type Transaction = alloy_primitives::Bytes;
pub type Hash = alloy_primitives::B256;
pub type PrivateKey = alloy_primitives::B256;

// Signature submodule
pub mod signature;
pub use signature::Signature;

// =======================================================================
// BLOCKCHAIN TYPES
// =======================================================================

pub mod blockchain;
pub use blockchain::Blockchain;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Default, RlpEncodable, RlpDecodable)]
pub struct BlockHeader {
    pub proposer: Address,
    pub round_number: u64,
    pub commit_seals: Vec<Signature>,
    pub height: u64,
    pub timestamp: u64,

    /// Not part of the Dafny spec, but needed to track validators.
    pub validators: Vec<Address>,

    /// Hash of the block body. *not* the same as the ethereum header hash.
    pub digest: Hash,
}

#[derive(Clone, Default, RlpEncodable, RlpDecodable)]
pub struct Block {
    /// Model block header
    pub header: BlockHeader,

    /// RLP encoded block. In practice this could be any data.
    body: BlockBody,

    /// Included for the spec only, but we don't use this.
    pub transactions: Vec<Transaction>,
}

pub type RawBlockchain = Vec<RawBlock>;

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct RawBlockHeader {
    pub proposer: Address,
    pub height: u64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct RawBlock {
    pub header: RawBlockHeader,
    pub body: BlockBody,
    pub transactions: Vec<Transaction>,
}

// =======================================================================
// MESSAGE TYPES
// =======================================================================

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct UnsignedProposal {
    pub height: u64,
    pub round: u64,
    pub digest: Hash,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct UnsignedPrepare {
    pub height: u64,
    pub round: u64,
    pub digest: Hash,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct UnsignedCommit {
    pub height: u64,
    pub round: u64,
    pub commit_seal: Signature,
    pub digest: Hash,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct UnsignedNewBlock {
    pub height: u64,
    pub round: u64,
    pub digest: Hash,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct UnsignedRoundChange {
    pub height: u64,
    pub round: u64,
    pub prepared_value: Option<Hash>,
    pub prepared_round: Option<u64>,
}

impl Encodable for UnsignedRoundChange {
    fn encode(&self, out: &mut dyn BufMut) {
        // Encode prepared_round as round+1 so Some(0) doesn't collide with None in RLP.
        let prepared_round_encoded = self
            .prepared_round
            .map(|round| round.checked_add(1).expect("prepared_round overflow"));
        let prepared_value_length = match self.prepared_value.as_ref() {
            Some(value) => value.length(),
            None => {
                if prepared_round_encoded.is_some() {
                    1
                } else {
                    0
                }
            }
        };
        let prepared_round_length = prepared_round_encoded
            .as_ref()
            .map(|value| value.length())
            .unwrap_or(0);
        let payload_length = self.height.length()
            + self.round.length()
            + prepared_value_length
            + prepared_round_length;
        Header {
            list: true,
            payload_length,
        }
        .encode(out);
        self.height.encode(out);
        self.round.encode(out);
        if let Some(value) = self.prepared_value.as_ref() {
            value.encode(out);
        } else if prepared_round_encoded.is_some() {
            out.put_u8(alloy_rlp::EMPTY_STRING_CODE);
        }
        if let Some(value) = prepared_round_encoded.as_ref() {
            value.encode(out);
        }
    }

    fn length(&self) -> usize {
        let prepared_round_encoded = self
            .prepared_round
            .map(|round| round.checked_add(1).expect("prepared_round overflow"));
        let prepared_value_length = match self.prepared_value.as_ref() {
            Some(value) => value.length(),
            None => {
                if prepared_round_encoded.is_some() {
                    1
                } else {
                    0
                }
            }
        };
        let prepared_round_length = prepared_round_encoded
            .as_ref()
            .map(|value| value.length())
            .unwrap_or(0);
        let payload_length = self.height.length()
            + self.round.length()
            + prepared_value_length
            + prepared_round_length;
        Header {
            list: true,
            payload_length,
        }
        .length_with_payload()
    }
}

impl Decodable for UnsignedRoundChange {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::Custom(
                "UnsignedRoundChange must be an RLP list",
            ));
        }

        let mut inner = &buf[..header.payload_length];
        let height = u64::decode(&mut inner)?;
        let round = u64::decode(&mut inner)?;
        let prepared_value = if inner.is_empty() {
            None
        } else if inner[0] == alloy_rlp::EMPTY_STRING_CODE {
            inner = &inner[1..];
            None
        } else {
            Some(Hash::decode(&mut inner)?)
        };
        let prepared_round_encoded = if inner.is_empty() {
            None
        } else if inner[0] == alloy_rlp::EMPTY_STRING_CODE {
            inner = &inner[1..];
            None
        } else {
            Some(u64::decode(&mut inner)?)
        };
        if !inner.is_empty() {
            return Err(alloy_rlp::Error::Custom(
                "UnsignedRoundChange has trailing bytes",
            ));
        }
        *buf = &buf[header.payload_length..];

        let prepared_round = match prepared_round_encoded {
            None => None,
            Some(0) => None,
            Some(value) => Some(value - 1),
        };

        Ok(Self {
            height,
            round,
            prepared_value,
            prepared_round,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct SignedProposal {
    pub unsigned_payload: UnsignedProposal,
    pub signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct SignedPrepare {
    pub unsigned_payload: UnsignedPrepare,
    pub signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct SignedCommit {
    pub unsigned_payload: UnsignedCommit,
    pub signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct SignedNewBlock {
    pub unsigned_payload: UnsignedNewBlock,
    pub signature: Signature,
    pub block: Block,
}

/// Request for blocks in a range (used for block backfill/sync)
#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct BlockRequest {
    /// Starting block height (inclusive)
    pub from_height: u64,
    /// Ending block height (inclusive)
    pub to_height: u64,
}

/// Response containing requested blocks
#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct BlockResponse {
    /// The requested blocks with their signatures
    pub blocks: Vec<SignedNewBlock>,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct SignedRoundChange {
    pub unsigned_payload: UnsignedRoundChange,
    pub signature: Signature,
}

pub mod qbft_message;
pub use qbft_message::QbftMessage;

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct Proposal {
    pub proposal_payload: SignedProposal,
    pub proposed_block: Block,
    pub proposal_justification: Vec<SignedRoundChange>,
    pub round_change_justification: Vec<SignedPrepare>,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct Prepare {
    pub prepare_payload: SignedPrepare,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
pub struct Commit {
    pub commit_payload: SignedCommit,
}

#[derive(Clone, Debug, PartialEq, Default, RlpEncodable, RlpDecodable)]
#[rlp(trailing)]
pub struct RoundChange {
    pub round_change_payload: SignedRoundChange,
    pub round_change_justification: Vec<SignedPrepare>,
    pub proposed_block_for_next_round: Option<Block>,
}

#[derive(Clone, Debug, PartialEq, RlpEncodable, RlpDecodable)]
pub struct QbftMessageWithRecipient {
    pub message: QbftMessage,
    pub recipient: Address,
}

// =======================================================================
// STATE TYPES
// =======================================================================

pub mod configuration;
pub use configuration::{Configuration, RoundChangeConfig};

pub mod node_state;
pub use node_state::NodeState;

// =======================================================================
// GENERAL TYPES
// =======================================================================

pub type Optional<T> = Option<T>;

pub type ISeq<T> = Vec<T>;

// =======================================================================
// BEHAVIOUR TYPES
// =======================================================================

#[derive(Clone, Debug, PartialEq, Default)]
pub struct QbftSpecificationStep {
    pub messages_received: Vec<QbftMessage>,
    pub new_blockchain: Blockchain,
    pub messages_to_send: Vec<QbftMessageWithRecipient>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct QbftNodeBehaviour {
    pub initial_blockchain: Blockchain,
    pub steps: ISeq<QbftSpecificationStep>,
}

/// Proposal justification structure
///
/// Not in the spec, but useful for implementation.
#[derive(Clone, Debug)]
pub struct ProposalJustification<'a> {
    pub round: u64,
    pub round_changes: Vec<&'a RoundChange>,
    pub prepares: Vec<&'a Prepare>,
    pub block: Block,
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header
            && self.body == other.body
            && self.transactions == other.transactions
    }
}

impl Eq for Block {}

impl PartialOrd for Block {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Block {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.header, &self.body, &self.transactions).cmp(&(
            &other.header,
            &other.body,
            &other.transactions,
        ))
    }
}

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.summarise().fmt(f)
    }
}

impl Block {
    pub fn new(mut header: BlockHeader, body: BlockBody) -> Self {
        header.digest = alloy_primitives::keccak256(&body);
        Self {
            header,
            body,
            transactions: vec![],
        }
    }

    pub fn new_with_transactions(
        mut header: BlockHeader,
        body: BlockBody,
        transactions: Vec<Transaction>,
    ) -> Self {
        header.digest = alloy_primitives::keccak256(&body);
        Self {
            header,
            body,
            transactions,
        }
    }

    pub fn body(&self) -> &BlockBody {
        &self.body
    }

    pub fn summarise(&self) -> String {
        let bytes = self.header.digest.as_slice();
        format!(
            "0x{:02x}{:02x}/{}",
            bytes[0], bytes[1], self.header.round_number
        )
    }

    pub fn validate_digest(&self) -> bool {
        let computed_digest = alloy_primitives::keccak256(&self.body);
        self.header.digest == computed_digest
    }

    pub fn round_sort_key(&self) -> (u64, Hash) {
        (
            self.header.round_number,
            crate::node_auxilliary_functions::digest(self),
        )
    }
}
