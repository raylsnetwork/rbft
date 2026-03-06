// SPDX-License-Identifier: Apache-2.0
//! Kubernetes deployment utilities for RBFT testnet

use std::io::Write;
use std::path::Path;

pub fn ensure_namespace(namespace: &str) -> eyre::Result<()> {
    let namespace = namespace.trim();
    if namespace.is_empty() {
        return Err(eyre::eyre!("Kubernetes namespace is empty"));
    }

    let exists = std::process::Command::new("kubectl")
        .args(["get", "namespace", namespace])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if exists {
        return Ok(());
    }

    eprintln!("Creating Kubernetes namespace {}...", namespace);

    let output = std::process::Command::new("kubectl")
        .args(["create", "namespace", namespace])
        .output()?;

    if !output.status.success() {
        return Err(eyre::eyre!(
            "Failed to create namespace {}: {}",
            namespace,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    eprintln!("✓ Namespace created");
    Ok(())
}

/// Create Kubernetes ConfigMap with genesis and validator keys
pub fn create_configmap(assets_dir: &Path, num_nodes: u32, namespace: &str) -> eyre::Result<()> {
    ensure_namespace(namespace)?;
    eprintln!("Creating Kubernetes ConfigMap...");

    // Read genesis file
    let genesis_content = std::fs::read_to_string(assets_dir.join("genesis.json"))?;
    let reth_config_content = r#"# Custom RBFT node configuration
# Tuned for high-throughput transaction processing

[sessions]
# Default is 32, increased to handle extreme transaction bursts
session_command_buffer = 393216
# Default is 260, increased for better event handling
session_event_buffer = 393216
"#;

    // Build ConfigMap YAML
    let mut configmap = String::from("apiVersion: v1\nkind: ConfigMap\nmetadata:\n");
    configmap.push_str("  name: rbft-testnet-config\n");
    configmap.push_str(&format!("  namespace: {}\n", namespace));
    configmap.push_str("data:\n");

    // Add genesis.json
    configmap.push_str("  genesis.json: |\n");
    for line in genesis_content.lines() {
        configmap.push_str(&format!("    {}\n", line));
    }

    // Add reth-config.toml
    configmap.push_str("  reth-config.toml: |\n");
    for line in reth_config_content.lines() {
        configmap.push_str(&format!("    {}\n", line));
    }

    // Read enodes from CSV file
    let csv_path = assets_dir.join("nodes.csv");
    let csv_content = std::fs::read_to_string(&csv_path)?;
    let mut enodes = Vec::new();

    for (i, line) in csv_content.lines().enumerate() {
        if i == 0 {
            continue; // Skip header
        }
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() >= 5 {
            enodes.push(fields[4]); // enode is the 5th field (index 4)
        }
    }

    let enodes_content = enodes.join(",");
    configmap.push_str(&format!("  enodes.txt: \"{}\"\n", enodes_content));

    // Add validator keys and p2p keys
    for i in 0..num_nodes {
        let validator_key =
            std::fs::read_to_string(assets_dir.join(format!("validator-key{}.txt", i)))?;
        let p2p_key = std::fs::read_to_string(assets_dir.join(format!("p2p-secret-key{}.txt", i)))?;

        configmap.push_str(&format!(
            "  validator-key{}.txt: \"{}\"\n",
            i,
            validator_key.trim()
        ));
        configmap.push_str(&format!(
            "  p2p-secret-key{}.txt: \"{}\"\n",
            i,
            p2p_key.trim()
        ));
    }

    // Apply ConfigMap
    let mut apply_cmd = std::process::Command::new("kubectl")
        .args(["apply", "-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(stdin) = apply_cmd.stdin.as_mut() {
        stdin.write_all(configmap.as_bytes())?;
    }

    let output = apply_cmd.wait_with_output()?;

    if !output.status.success() {
        return Err(eyre::eyre!(
            "Failed to create ConfigMap: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    eprintln!("✓ ConfigMap created");
    Ok(())
}

/// Deploy Kubernetes StatefulSet for validators
pub fn deploy_statefulset(
    num_nodes: u32,
    extra_args: Option<&[String]>,
    namespace: &str,
    image: &str,
) -> eyre::Result<()> {
    eprintln!("Creating headless Service for StatefulSet...");

    // Create headless Service for StatefulSet pod DNS
    let service_yaml = format!(
        r#"apiVersion: v1
kind: Service
metadata:
  name: rbft-node
  namespace: {namespace}
spec:
  clusterIP: None
  selector:
    app: rbft-node
  ports:
  - name: p2p
    port: 30303
    targetPort: 30303
  - name: http
    port: 8545
    targetPort: 8545
"#,
        namespace = namespace
    );

    let mut svc_cmd = std::process::Command::new("kubectl")
        .args(["apply", "-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(stdin) = svc_cmd.stdin.as_mut() {
        stdin.write_all(service_yaml.as_bytes())?;
    }

    let svc_output = svc_cmd.wait_with_output()?;

    if !svc_output.status.success() {
        return Err(eyre::eyre!(
            "Failed to create Service: {}",
            String::from_utf8_lossy(&svc_output.stderr)
        ));
    }

    eprintln!("✓ Service created");
    eprintln!("Deploying Kubernetes StatefulSet...");

    // Build command arguments string (without trusted-peers, which will be added at runtime)
    let mut cmd_args = vec![
        "node".to_string(),
        "--http".to_string(),
        "--http.port".to_string(),
        "8545".to_string(),
        "--http.addr".to_string(),
        "0.0.0.0".to_string(),
        "--http.corsdomain".to_string(),
        "*".to_string(),
        "--http.api".to_string(),
        "eth,txpool".to_string(),
        "--chain".to_string(),
        "/config/genesis.json".to_string(),
        "--config".to_string(),
        "/config/reth-config.toml".to_string(),
        "--validator-key".to_string(),
        "/config/validator-key.txt".to_string(),
        "--p2p-secret-key".to_string(),
        "/config/p2p-secret-key.txt".to_string(),
        "--datadir".to_string(),
        "/data".to_string(),
        "--ipcpath".to_string(),
        "/data/reth.ipc".to_string(),
        "--authrpc.port".to_string(),
        "8551".to_string(),
        "--port".to_string(),
        "30303".to_string(),
        "--disable-discovery".to_string(),
    ];

    // Add extra arguments if provided (split on whitespace)
    if let Some(extra) = extra_args {
        for arg in extra {
            // Split each argument on whitespace to handle cases like "--flag value"
            cmd_args.extend(arg.split_whitespace().map(|s| s.to_string()));
        }
    }

    // Build command string with trusted-peers from file.
    // Use bash for pipefail so rbft-node exits propagate when tee is used.
    let mut rbft_cmd = String::from("set -euo pipefail; ");
    rbft_cmd.push_str("mkdir -p /data/logs; ");
    rbft_cmd.push_str("/app/rbft-node");
    for arg in &cmd_args {
        rbft_cmd.push_str(&format!(" '{}'", arg.replace("'", "'\"'\"'")));
    }
    // Add trusted-peers from file
    rbft_cmd.push_str(" --trusted-peers \"$(cat /config/enodes.txt)\"");
    rbft_cmd.push_str(" 2>&1 | tee -a /data/logs/node.log");

    // Build StatefulSet YAML
    let mut statefulset = format!(
        r#"apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: rbft-node
  namespace: {namespace}
spec:
  replicas: {}
  serviceName: rbft-node
  selector:
    matchLabels:
      app: rbft-node
  template:
    metadata:
      labels:
        app: rbft-node
    spec:
      securityContext:
        fsGroup: 1000
      containers:
      - name: rbft-node
        image: {}
        command: ["/bin/bash", "-c"]
        args:
        - |
          {}
"#,
        num_nodes,
        image,
        rbft_cmd,
        namespace = namespace
    );

    statefulset.push_str(
        r#"        ports:
        - name: http
          containerPort: 8545
        - name: authrpc
          containerPort: 8551
        - name: p2p
          containerPort: 30303
        volumeMounts:
        - name: config-raw
          mountPath: /config-raw
        - name: config
          mountPath: /config
        - name: data
          mountPath: /data
        env:
        - name: RUST_LOG
          value: "info"
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
      initContainers:
      - name: setup-keys
        image: busybox
        command:
        - sh
        - -c
        - |
          # Extract ordinal from pod name (rbft-node-0 -> 0)
          ORDINAL=$(echo $POD_NAME | grep -o '[0-9]*$')
          echo "Setting up keys for validator $ORDINAL"
          cp /config-raw/genesis.json /config/genesis.json
          cp /config-raw/enodes.txt /config/enodes.txt
          cp /config-raw/reth-config.toml /config/reth-config.toml
          cp /config-raw/validator-key${ORDINAL}.txt /config/validator-key.txt
          cp /config-raw/p2p-secret-key${ORDINAL}.txt /config/p2p-secret-key.txt
          mkdir -p /data/db /data/logs
          chown -R 1000:1000 /data
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        volumeMounts:
        - name: config-raw
          mountPath: /config-raw
        - name: config
          mountPath: /config
        - name: data
          mountPath: /data
      volumes:
      - name: config-raw
        configMap:
          name: rbft-testnet-config
      - name: config
        emptyDir: {}
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 7Gi
"#,
    );

    // Apply StatefulSet
    let mut apply_cmd = std::process::Command::new("kubectl")
        .args(["apply", "-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(stdin) = apply_cmd.stdin.as_mut() {
        stdin.write_all(statefulset.as_bytes())?;
    }

    let output = apply_cmd.wait_with_output()?;

    if !output.status.success() {
        return Err(eyre::eyre!(
            "Failed to deploy StatefulSet: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    eprintln!("✓ StatefulSet deployed, waiting for pods to be ready...");

    // Wait for all pods to be ready using kubectl wait
    let wait_output = std::process::Command::new("kubectl")
        .args([
            "wait",
            "--for=condition=ready",
            "pod",
            "-l",
            "app=rbft-node",
            "-n",
            namespace,
            "--timeout=180s",
        ])
        .output()?;

    if !wait_output.status.success() {
        eprintln!(
            "Warning: Some pods may not be ready: {}",
            String::from_utf8_lossy(&wait_output.stderr)
        );
    } else {
        eprintln!("✓ All pods are ready");
    }

    eprintln!("✓ Kubernetes deployment complete");
    Ok(())
}

/// Clean up Kubernetes resources
pub fn cleanup_testnet(namespace: &str) -> eyre::Result<()> {
    eprintln!("Cleaning up Kubernetes resources...");

    // Delete StatefulSet
    let _ = std::process::Command::new("kubectl")
        .args(["delete", "statefulset", "rbft-node", "-n", namespace])
        .output();

    // Delete Service
    let _ = std::process::Command::new("kubectl")
        .args(["delete", "service", "rbft-node", "-n", namespace])
        .output();

    // Delete ConfigMap
    let _ = std::process::Command::new("kubectl")
        .args([
            "delete",
            "configmap",
            "rbft-testnet-config",
            "-n",
            namespace,
        ])
        .output();

    // Delete PVCs
    let _ = std::process::Command::new("kubectl")
        .args(["delete", "pvc", "-l", "app=rbft-node", "-n", namespace])
        .output();

    eprintln!("✓ Kubernetes resources cleaned up");
    Ok(())
}
