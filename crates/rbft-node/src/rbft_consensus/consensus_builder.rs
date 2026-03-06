// SPDX-License-Identifier: Apache-2.0
//! RBFT Consensus Builder
//!
//! This builder creates a consensus instance that allows RBFT-specific validation rules,
//! particularly for timestamp handling between blocks.

use alloy_primitives::{Bloom, FixedBytes};
use reth_consensus_common::validation::{
    validate_against_parent_4844, validate_against_parent_eip1559_base_fee,
    validate_against_parent_gas_limit, validate_against_parent_hash_number,
};
use reth_ethereum::{
    chainspec::{ChainSpec, EthChainSpec},
    consensus::{Consensus, ConsensusError, EthBeaconConsensus, FullConsensus, HeaderValidator},
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{components::ConsensusBuilder, BuilderContext},
    },
    primitives::{BlockHeader, Header, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader},
    provider::BlockExecutionResult,
    EthPrimitives,
};
use reth_ethereum_primitives::{Block, BlockBody};
use std::sync::Arc;

/// RBFT beacon consensus that wraps Ethereum beacon consensus
#[derive(Debug, Clone)]
pub struct RbftBeaconConsensus {
    /// Inner Ethereum beacon consensus
    inner: Arc<EthBeaconConsensus<ChainSpec>>,
}

impl RbftBeaconConsensus {
    /// Create a new RBFT beacon consensus
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            inner: Arc::new(EthBeaconConsensus::new(chain_spec)),
        }
    }
}

/// Custom timestamp validation to allow sub-second blocks in RBFT.
#[inline]
pub fn custom_validate_against_parent_timestamp<H: BlockHeader>(
    header: &H,
    parent: &H,
) -> Result<(), ConsensusError> {
    if header.timestamp() < parent.timestamp() {
        return Err(ConsensusError::TimestampIsInPast {
            parent_timestamp: parent.timestamp(),
            timestamp: header.timestamp(),
        });
    }
    Ok(())
}

impl HeaderValidator<Header> for RbftBeaconConsensus {
    fn validate_header(&self, header: &SealedHeader<Header>) -> Result<(), ConsensusError> {
        self.inner.validate_header(header)
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader<Header>,
        parent: &SealedHeader<Header>,
    ) -> Result<(), ConsensusError> {
        // Validate hash and number relationship
        validate_against_parent_hash_number(header.header(), parent)?;

        // RBFT: Custom timestamp validation to allow same timestamps.
        custom_validate_against_parent_timestamp(header.header(), parent.header())?;

        // Validate gas limit
        validate_against_parent_gas_limit(header, parent, self.inner.chain_spec())?;

        // Validate EIP-1559 base fee
        validate_against_parent_eip1559_base_fee(
            header.header(),
            parent.header(),
            self.inner.chain_spec(),
        )?;

        // Ensure that the blob gas fields for this block are valid
        if let Some(blob_params) = self
            .inner
            .chain_spec()
            .blob_params_at_timestamp(header.header().timestamp)
        {
            validate_against_parent_4844(header.header(), parent.header(), blob_params)?;
        }

        Ok(())
    }
}

impl Consensus<Block> for RbftBeaconConsensus {
    fn validate_body_against_header(
        &self,
        body: &BlockBody,
        header: &SealedHeader<Header>,
    ) -> Result<(), ConsensusError> {
        <Arc<EthBeaconConsensus<ChainSpec>> as Consensus<Block>>::validate_body_against_header(
            &self.inner,
            body,
            header,
        )
    }

    fn validate_block_pre_execution(
        &self,
        block: &SealedBlock<Block>,
    ) -> Result<(), ConsensusError> {
        self.inner.validate_block_pre_execution(block)
    }
}

impl FullConsensus<EthPrimitives> for RbftBeaconConsensus {
    fn validate_block_post_execution(
        &self,
        block: &RecoveredBlock<Block>,
        result: &BlockExecutionResult<<EthPrimitives as NodePrimitives>::Receipt>,
        beacon_root: Option<(FixedBytes<32>, Bloom)>,
    ) -> Result<(), ConsensusError> {
        <Arc<EthBeaconConsensus<ChainSpec>> as FullConsensus<
            EthPrimitives,
        >>::validate_block_post_execution(&self.inner, block, result, beacon_root)
    }
}

/// RBFT consensus builder that wraps Ethereum consensus
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct RbftConsensusBuilder;

impl<N> ConsensusBuilder<N> for RbftConsensusBuilder
where
    N: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
{
    type Consensus = Arc<RbftBeaconConsensus>;

    async fn build_consensus(self, ctx: &BuilderContext<N>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(RbftBeaconConsensus::new(ctx.chain_spec())))
    }
}
