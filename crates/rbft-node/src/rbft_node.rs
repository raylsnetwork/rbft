// SPDX-License-Identifier: Apache-2.0
//! RBFT Node Type
//!
//! This module defines the RBFT node type that uses the custom payload validator.

use crate::rbft_consensus::{RbftConsensusBuilder, RbftPayloadValidatorBuilder};
use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{
            components::{BasicPayloadServiceBuilder, ComponentsBuilder},
            rpc::RpcAddOns,
            Node, NodeAdapter,
        },
        node::{EthereumExecutorBuilder, EthereumNetworkBuilder, EthereumPoolBuilder},
        EthEngineTypes, EthereumEthApiBuilder, EthereumPayloadBuilder,
    },
    storage::EthStorage,
    EthPrimitives,
};

/// RBFT node type using custom payload validator
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct RbftNode;

/// Configure the node types for RBFT
impl NodeTypes for RbftNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = EthStorage;
    type Payload = EthEngineTypes;
}

/// Custom RBFT node addons configuring RPC types with RBFT payload validator
pub type RbftNodeAddOns<N> = RpcAddOns<N, EthereumEthApiBuilder, RbftPayloadValidatorBuilder>;

/// Implement the Node trait for the RBFT node
impl<N> Node<N> for RbftNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        RbftConsensusBuilder,
    >;
    type AddOns = RbftNodeAddOns<NodeAdapter<N>>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(EthereumPoolBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::new(
                EthereumPayloadBuilder::default(),
            ))
            .network(EthereumNetworkBuilder::default())
            .consensus(RbftConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {
        RbftNodeAddOns::default()
    }
}
