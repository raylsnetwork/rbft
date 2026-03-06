// SPDX-License-Identifier: Apache-2.0
//! Shared constants for RBFT tooling.

/// Well-known admin private key used for local testnets and load generation.
///
/// This key is baked into genesis assets so tooling can assume a funded account.
pub const DEFAULT_ADMIN_KEY: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000001";

/// Ethereum address derived from [`DEFAULT_ADMIN_KEY`].
///
/// `cast wallet address --private-key 0x000...0001` → `0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf`
pub const DEFAULT_ADMIN_ADDRESS: &str = "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf";
