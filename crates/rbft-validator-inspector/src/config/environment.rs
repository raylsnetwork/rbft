// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;

use super::paths::assets_dir;

pub fn preload_environment() {
    if std::env::var_os("VALIDATOR_INSPECTOR_SKIP_ENV").is_some() {
        return;
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(explicit) = std::env::var("VALIDATOR_INSPECTOR_ENV_FILE") {
        candidates.push(PathBuf::from(explicit));
    }

    let assets_dir = assets_dir();
    candidates.push(assets_dir.join("validator-inspector.env"));
    candidates.push(assets_dir.join("validators.env"));
    candidates.push(assets_dir.join("rpc-urls.env"));
    candidates.push(assets_dir.join(".env"));

    for path in candidates {
        if path.exists() {
            if let Err(err) = dotenvy::from_path(&path) {
                eprintln!(
                    "Failed to load environment from {}: {}",
                    path.display(),
                    err
                );
            }
            break;
        }
    }
}

pub fn load_validator_key(label: &str) -> Option<String> {
    if let Some(idx_str) = label.strip_prefix('v') {
        if let Ok(idx) = idx_str.parse::<usize>() {
            let path = assets_dir().join(format!("validator-key{idx}.txt"));
            if let Ok(contents) = std::fs::read_to_string(path) {
                let trimmed = contents.trim();
                if !trimmed.is_empty() {
                    // Return shortened version for display
                    if trimmed.starts_with("0x") && trimmed.len() > 10 {
                        let short = format!(
                            "{}…{}",
                            &trimmed[..6],
                            &trimmed[trimmed.len().saturating_sub(4)..]
                        );
                        return Some(short);
                    }
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn load_trusted_peers() -> Option<String> {
    let path = assets_dir().join("enodes.txt");
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.replace('\n', "").trim().to_string())
        .filter(|s| !s.is_empty())
}
