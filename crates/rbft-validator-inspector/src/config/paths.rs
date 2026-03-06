// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;

pub fn default_test_dirs() -> (PathBuf, PathBuf) {
    let db_env = std::env::var("RBFT_CLI_DB_DIR")
        .or_else(|_| std::env::var("RBFT_DB_DIR"))
        .ok();
    let logs_env = std::env::var("RBFT_CLI_LOGS_DIR")
        .or_else(|_| std::env::var("RBFT_LOGS_DIR"))
        .ok();
    if let (Some(db), Some(logs)) = (db_env, logs_env) {
        return (PathBuf::from(db), PathBuf::from(logs));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let base = PathBuf::from(home).join(".rbft").join("testnet");
    (base.join("db"), base.join("logs"))
}

pub fn assets_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.join("../crates/rbft-node2/assets"),
        manifest_dir.join("../rbft-node2/assets"),
        manifest_dir.join("../../crates/rbft-node2/assets"),
    ];

    for candidate in candidates.iter() {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    // Fall back to the first candidate even if it does not exist, so callers get
    // a consistent path to display in error messages.
    candidates[0].clone()
}
