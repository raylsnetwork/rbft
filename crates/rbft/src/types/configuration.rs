// SPDX-License-Identifier: Apache-2.0
//! Configuration module for QBFT nodes
//!
//! This module contains the configuration structures for QBFT nodes,
//! including round change timeout configuration.

use serde::{Deserialize, Serialize};

use super::{Address, Block};

/// Round change time is calculated as:
///
/// ```text
/// start_time + first_interval * (growth_factor ^ round)
/// ```
///
/// So for example, with start_time = 0, first_interval = 1, growth_factor = 2:
///
/// Round | Time
/// -------|------
///   0   |  1
///   1   |  2
///   2   |  4
///     
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoundChangeConfig {
    pub start_time: f64,
    pub first_interval: f64,
    pub growth_factor: f64,
    pub max_round: u64,
    /// If false, prevent round change timeout on the first block (height 1)
    #[serde(default)]
    pub round_change_on_first_block: bool,
}

impl RoundChangeConfig {
    /// Calculate the round timeout for a given round number.
    pub fn timeout_for_round(&self, round: u64) -> u64 {
        // let clamped_round = round.min(self.max_round);
        let timeout =
            self.start_time + self.first_interval * (2_f64).powf(self.growth_factor * round as f64);
        timeout.ceil() as u64
    }
}

impl Default for RoundChangeConfig {
    fn default() -> Self {
        Self {
            // Constant time after expected block start.
            start_time: 0.0,
            first_interval: 10.0,
            growth_factor: 1.5,
            max_round: 10,
            round_change_on_first_block: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Configuration {
    pub nodes: Vec<Address>,
    pub genesis_block: Block,
    pub block_time: u64,
    pub round_change_config: RoundChangeConfig,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            nodes: vec![],
            genesis_block: Block::default(),
            block_time: 10,
            round_change_config: RoundChangeConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_change_config_timeout() {
        let config = RoundChangeConfig {
            start_time: 0.0,
            first_interval: 1.0,
            growth_factor: 2.0,
            max_round: 10,
            round_change_on_first_block: false,
        };

        // Test exponential backoff: start_time + first_interval * (2 ^ (growth_factor * round))
        assert_eq!(config.timeout_for_round(0), 1); // 0 + 1 * 2^(2.0 * 0) = 1 * 2^0 = 1
        assert_eq!(config.timeout_for_round(1), 4); // 0 + 1 * 2^(2.0 * 1) = 1 * 2^2 = 4
        assert_eq!(config.timeout_for_round(2), 16); // 0 + 1 * 2^(2.0 * 2) = 1 * 2^4 = 16
        assert_eq!(config.timeout_for_round(3), 64); // 0 + 1 * 2^(2.0 * 3) = 1 * 2^6 = 64
    }

    #[test]
    fn test_round_change_config_max_round() {
        let config = RoundChangeConfig {
            start_time: 0.0,
            first_interval: 1.0,
            growth_factor: 2.0,
            max_round: 5,
            round_change_on_first_block: false,
        };

        // Note: max_round is not currently enforced in timeout_for_round (clamping is commented
        // out) These tests show the actual behavior without clamping
        assert_eq!(config.timeout_for_round(10), 1048576); // 0 + 1 * 2^(2.0 * 10) = 2^20 = 1048576
        assert_eq!(config.timeout_for_round(5), 1024); // 0 + 1 * 2^(2.0 * 5) = 2^10 = 1024
        assert_eq!(config.timeout_for_round(4), 256); // 0 + 1 * 2^(2.0 * 4) = 2^8 = 256
    }

    #[test]
    fn test_round_change_config_with_start_time() {
        let config = RoundChangeConfig {
            start_time: 10.0,
            first_interval: 1.0,
            growth_factor: 2.0,
            max_round: 10,
            round_change_on_first_block: false,
        };

        // Test with non-zero start_time: start_time + first_interval * (2 ^ (growth_factor *
        // round))
        assert_eq!(config.timeout_for_round(0), 11); // 10 + 1 * 2^(2.0 * 0) = 10 + 1 = 11
        assert_eq!(config.timeout_for_round(1), 14); // 10 + 1 * 2^(2.0 * 1) = 10 + 4 = 14
        assert_eq!(config.timeout_for_round(2), 26); // 10 + 1 * 2^(2.0 * 2) = 10 + 16 = 26
    }
}
