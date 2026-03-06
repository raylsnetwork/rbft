// SPDX-License-Identifier: Apache-2.0
//! RBFT Payload Validator Builder
//!
//! This module provides the builder for the RBFT payload validator that integrates
//! with the Reth node builder infrastructure.

use super::RbftPayloadValidator;
use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
        api::AddOnsContext, builder::rpc::PayloadValidatorBuilder, EthEngineTypes, EthEvmConfig,
    },
};

/// Builder for the RBFT payload validator
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct RbftPayloadValidatorBuilder;

impl<N> PayloadValidatorBuilder<N> for RbftPayloadValidatorBuilder
where
    N: reth_ethereum::node::api::FullNodeComponents<Evm = EthEvmConfig>,
    <N as reth_ethereum::node::api::FullNodeTypes>::Types:
        reth_ethereum::node::api::NodeTypes<Payload = EthEngineTypes, ChainSpec = ChainSpec>,
{
    type Validator = RbftPayloadValidator;

    async fn build(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        Ok(RbftPayloadValidator::new(ctx.config.chain.clone()))
    }
}
