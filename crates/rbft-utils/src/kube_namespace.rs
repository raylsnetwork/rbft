// SPDX-License-Identifier: Apache-2.0
pub fn default_kube_namespace() -> String {
    if let Ok(namespace) = std::env::var("RBFT_KUBE_NAMESPACE") {
        if !namespace.trim().is_empty() {
            return namespace;
        }
    }

    "rbft".to_string()
}
