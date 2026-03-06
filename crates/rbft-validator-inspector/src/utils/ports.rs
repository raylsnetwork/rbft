// SPDX-License-Identifier: Apache-2.0
use anyhow::Result;
use std::path::Path;
use url::Url;

use crate::config::load_validator_key;
use crate::models::ValidatorTarget;

pub fn build_targets_from_ports(ports: &[u16], logs_root: &Path) -> Result<Vec<ValidatorTarget>> {
    let mut targets = Vec::with_capacity(ports.len());
    for (idx, port) in ports.iter().enumerate() {
        let url = Url::parse(&format!("http://127.0.0.1:{port}"))?;
        targets.push(ValidatorTarget {
            label: format!("v{idx}"),
            url,
            port: Some(*port),
            key: load_validator_key(&format!("v{idx}")),
            log_path: Some(logs_root.join(format!("node{idx}.log"))),
        });
    }
    Ok(targets)
}
