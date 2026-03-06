// SPDX-License-Identifier: Apache-2.0
use alloy_primitives::Address;
use rbft::types::RoundChangeConfig;
use serde::{Deserialize, Serialize};

/// Validator information in the RBFT configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ValidatorInfo {
    pub address: Address,
}

/// RBFT configuration structure matching genesis.json format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RbftConfig {
    /// Round change timeout configuration
    #[serde(rename = "roundChangeConfig", default)]
    pub round_change_config: RoundChangeConfig,

    /// Optional address of the validator set contract (proxy address)
    #[serde(
        rename = "validatorSetContract",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub validator_set_contract: Option<Address>,

    /// Per-peer inbound message buffer high-water mark before pruning.
    #[serde(rename = "messageBufferMax", default = "default_message_buffer_max")]
    pub message_buffer_max: usize,

    /// Per-peer inbound message buffer size after pruning.
    #[serde(
        rename = "messageBufferTrimTo",
        default = "default_message_buffer_trim_to"
    )]
    pub message_buffer_trim_to: usize,

    /// Idle connection timeout in seconds. If zero, idle timeout is disabled.
    #[serde(
        rename = "idleConnectionTimeoutSeconds",
        default = "default_idle_connection_timeout_seconds"
    )]
    pub idle_connection_timeout_seconds: u64,

    /// Maximum number of reconnection attempts when a peer disconnects.
    #[serde(
        rename = "reconnectMaxAttempts",
        default = "default_reconnect_max_attempts"
    )]
    pub reconnect_max_attempts: u32,

    /// Base delay in milliseconds for reconnection backoff (doubles each attempt).
    #[serde(
        rename = "reconnectBaseDelayMs",
        default = "default_reconnect_base_delay_ms"
    )]
    pub reconnect_base_delay_ms: u64,

    /// Maximum delay in milliseconds between reconnection attempts.
    #[serde(
        rename = "reconnectMaxDelayMs",
        default = "default_reconnect_max_delay_ms"
    )]
    pub reconnect_max_delay_ms: u64,

    /// Whether to relay messages to other peers. Disable in full mesh topologies.
    #[serde(rename = "relayEnabled", default = "default_relay_enabled")]
    pub relay_enabled: bool,
}

const fn default_message_buffer_max() -> usize {
    8192
}

const fn default_message_buffer_trim_to() -> usize {
    6144
}

const fn default_idle_connection_timeout_seconds() -> u64 {
    60
}

const fn default_reconnect_max_attempts() -> u32 {
    10
}

const fn default_reconnect_base_delay_ms() -> u64 {
    1000
}

const fn default_reconnect_max_delay_ms() -> u64 {
    30000
}

const fn default_relay_enabled() -> bool {
    false
}

impl Default for RbftConfig {
    fn default() -> Self {
        Self {
            round_change_config: RoundChangeConfig::default(),
            validator_set_contract: None,
            message_buffer_max: default_message_buffer_max(),
            message_buffer_trim_to: default_message_buffer_trim_to(),
            idle_connection_timeout_seconds: default_idle_connection_timeout_seconds(),
            reconnect_max_attempts: default_reconnect_max_attempts(),
            reconnect_base_delay_ms: default_reconnect_base_delay_ms(),
            reconnect_max_delay_ms: default_reconnect_max_delay_ms(),
            relay_enabled: default_relay_enabled(),
        }
    }
}
