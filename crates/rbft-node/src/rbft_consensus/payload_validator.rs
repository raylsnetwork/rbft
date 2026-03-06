// SPDX-License-Identifier: Apache-2.0
//! Custom PayloadValidator for RBFT consensus engine.
//!
//! This module implements a custom payload validator that extends the default
//! Ethereum payload validation with RBFT-specific checks.

use alloy_rpc_types::engine::{ExecutionData, PayloadAttributes};
use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
        api::{
            payload::PayloadOrAttributes, validate_version_specific_fields,
            EngineApiMessageVersion, EngineApiValidator, EngineObjectValidationError,
            InvalidPayloadAttributesError, NewPayloadError,
            PayloadAttributes as PayloadAttributesTrait, PayloadValidator,
        },
        EthEngineTypes,
    },
    primitives::{RecoveredBlock, SealedBlock},
};
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use reth_ethereum_primitives::Block;
use std::sync::Arc;
use tracing::{debug, error, info};

/// Custom RBFT engine validator that extends the default Ethereum validator
/// with RBFT-specific validation logic.
#[derive(Debug, Clone)]
pub struct RbftPayloadValidator {
    /// Inner Ethereum payload validator
    inner: EthereumExecutionPayloadValidator<ChainSpec>,
}

impl RbftPayloadValidator {
    /// Instantiates a new RBFT payload validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            inner: EthereumExecutionPayloadValidator::new(chain_spec),
        }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        self.inner.chain_spec()
    }
}

impl PayloadValidator<EthEngineTypes> for RbftPayloadValidator {
    type Block = Block;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        // Delegate to the inner Ethereum validator
        let sealed_block = self.inner.ensure_well_formed_payload(payload)?;
        sealed_block
            .try_recover()
            .map_err(|e| NewPayloadError::Other(e.into()))
    }

    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Self::Block>, NewPayloadError> {
        // Convert ExecutionData to SealedBlock by ensuring it's well formed
        self.inner
            .ensure_well_formed_payload(payload)
            .map_err(Into::into)
    }

    fn validate_payload_attributes_against_header(
        &self,
        attr: &PayloadAttributes,
        header: &<Self::Block as reth_ethereum::primitives::Block>::Header,
    ) -> Result<(), InvalidPayloadAttributesError> {
        debug!("Validating payload attributes against header for RBFT");
        // RBFT: Allow zero time between blocks.
        if attr.timestamp() < header.timestamp {
            error!("FAIL!");
            return Err(InvalidPayloadAttributesError::InvalidTimestamp);
        }
        info!("Validated payload attributes against header for RBFT");
        Ok(())
    }
}

impl EngineApiValidator<EthEngineTypes> for RbftPayloadValidator {
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, ExecutionData, PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        // We don't have any special version-specific fields to validate for RBFT, but do this
        // anyway.
        validate_version_specific_fields(self.chain_spec(), version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        // No special RBFT-specific attribute checks, but delegate to default validation.
        validate_version_specific_fields(
            self.chain_spec(),
            version,
            PayloadOrAttributes::<ExecutionData, PayloadAttributes>::PayloadAttributes(attributes),
        )
    }
}
