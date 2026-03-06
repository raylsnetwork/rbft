// SPDX-License-Identifier: Apache-2.0
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use reth_ethereum::{
    cli::{chainspec::EthereumChainSpecParser, Cli},
    engine::local::LocalPayloadAttributesBuilder,
    network::{types::PeerKind, NetworkHandle, Peers},
    node::builder::NodeHandle,
};
use reth_network_peers::{PeerId, TrustedPeer};
use std::{collections::HashMap, net::SocketAddr, time::Duration};
use tracing::{debug, info, warn};

use crate::rbft_consensus::RbftConsensus;

mod metrics;
// QBFT is now a separate crate
mod rbft_consensus;
mod rbft_node;

use clap::Parser;
use metrics::QbftMetrics;

// === Node args ===
/// RBFT node configuration
///
/// Environment Variables:
///   RBFT_BLOCK_INTERVAL           Override block interval (seconds, float)
///   RBFT_GAS_LIMIT                Override gas limit (unsigned integer)
///   RBFT_DEBUG_CATCHUP_BLOCK      Only enable chain catchup after this
///                                 block height (unsigned integer)
///   RBFT_RESEND_AFTER             Resend cached messages after this many
///                                 seconds without block commits (default: 0=disabled)
///   RBFT_FULL_LOGS                Emit state logs on every advance (default: false)
///   RBFT_TRUSTED_PEERS_REFRESH_SECS  Peer refresh interval (default: 10)
#[derive(Debug, Parser, Clone)]
pub struct RbftNodeArgs {
    /// Path to the validator private key file
    #[arg(long)]
    pub validator_key: Option<String>,

    /// Directory for log files. Defaults to ~/.rbft/testnet/logs
    #[arg(long)]
    pub logs_dir: Option<String>,

    /// Directory for database files. Defaults to ~/.rbft/testnet/db
    #[arg(long)]
    pub db_dir: Option<String>,

    /// How often (in seconds) to refresh trusted peer DNS entries and reconnect on changes.
    ///
    /// Set to 0 to disable the refresher.
    #[arg(
        long,
        default_value_t = 10,
        value_name = "SECS",
        env = "RBFT_TRUSTED_PEERS_REFRESH_SECS"
    )]
    pub trusted_peers_refresh_secs: u64,

    /// Emit full state logs on every advance cycle.
    ///
    /// By default (false), the node only logs "b" (before) and "a" (after) messages
    /// when the state summary changes. Enable this flag to log on every advance.
    #[arg(long, env = "RBFT_FULL_LOGS")]
    pub full_logs: bool,

    /// Resend timeout in seconds. If no blocks are committed for this duration,
    /// resend all cached messages for current height and height-1.
    #[arg(long, value_name = "SECS", env = "RBFT_RESEND_AFTER")]
    pub resend_after_secs: Option<u64>,

    /// Disable express transaction delivery (direct forwarding of local pending
    /// transactions to the next block proposer).
    #[arg(long, env = "RBFT_DISABLE_EXPRESS")]
    pub disable_express: bool,
}

fn spawn_trusted_peer_refresh(
    network: NetworkHandle,
    trusted_peers: Vec<TrustedPeer>,
    interval: Duration,
    task_executor: reth_tasks::TaskExecutor,
) {
    if trusted_peers.is_empty() || interval.is_zero() {
        return;
    }

    info!(
        target: "rbft_node",
        peers = trusted_peers.len(),
        interval_secs = interval.as_secs(),
        "Starting trusted peer refresh task"
    );

    let local_peer_id = *network.peer_id();

    task_executor.spawn_critical("trusted_peer_refresh", async move {
        // Immediately disconnect from ourselves in case reth connected to our own enode
        // (can happen if local peer is included in trusted_peers list)
        debug!(
            target: "rbft_node",
            ?local_peer_id,
            "Disconnecting local peer (reason: self_disconnect)"
        );
        network.disconnect_peer(local_peer_id);

        let mut last_resolved: HashMap<PeerId, SocketAddr> = HashMap::new();
        let mut ticker = tokio::time::interval(interval);

        loop {
            ticker.tick().await;

            for peer in trusted_peers.iter() {
                let peer_id = peer.id;
                if peer_id == local_peer_id {
                    continue;
                }

                match peer.resolve().await {
                    Ok(record) => {
                        let tcp_addr = SocketAddr::new(record.address, record.tcp_port);
                        let udp_addr = SocketAddr::new(record.address, record.udp_port);
                        let previous_addr = last_resolved.insert(peer_id, tcp_addr);

                        let connected_to_same_addr = match network.get_peer_by_id(peer_id).await {
                            Ok(Some(info)) => info.remote_addr == tcp_addr,
                            _ => false,
                        };

                        if previous_addr != Some(tcp_addr) {
                            debug!(
                                target: "rbft_node",
                                ?peer_id,
                                %tcp_addr,
                                "Trusted peer resolved to new address, forcing reconnect"
                            );
                            debug!(
                                target: "rbft_node",
                                ?peer_id,
                                "Disconnecting peer (reason: trusted_peer_address_change)"
                            );
                            network.disconnect_peer(peer_id);
                        }

                        network.add_trusted_peer_with_udp(peer_id, tcp_addr, udp_addr);

                        if previous_addr != Some(tcp_addr) || !connected_to_same_addr {
                            network.connect_peer_kind(
                                peer_id,
                                PeerKind::Trusted,
                                tcp_addr,
                                Some(udp_addr),
                            );
                        }
                    }
                    Err(err) => {
                        warn!(
                            target: "rbft_node",
                            ?peer_id,
                            %err,
                            "Failed to resolve trusted peer address"
                        );
                    }
                }
            }
        }
    });
}

fn main() -> eyre::Result<()> {
    let cli = Cli::<EthereumChainSpecParser, RbftNodeArgs>::parse();

    cli.run(|builder, args| async move {
        info!(target: "rbft_node", "Launching node with RBFT payload validator");
        let handle = builder.launch_node(rbft_node::RbftNode::default()).await?;

        let NodeHandle {
            node,
            node_exit_future,
        } = handle;

        let network: NetworkHandle = node.network.clone();
        let task_executor = node.task_executor.clone();

        info!(
            target: "rbft_node",
            "Node P2P identity: {}",
            network.peer_id()
        );

        // TODO: Add a better way to choose metrics port
        let http_port = node.config.rpc.http_port;
        // Calculate unique metrics port based on offset from base HTTP port (8545)
        // This supports any number of nodes without port collisions
        let metrics_port = 9000 + http_port.saturating_sub(8545);

        let metrics = QbftMetrics::new();
        metrics.clone().serve(metrics_port);

        let refresh_interval = Duration::from_secs(args.trusted_peers_refresh_secs);
        spawn_trusted_peer_refresh(
            network.clone(),
            node.config.network.trusted_peers.clone(),
            refresh_interval,
            task_executor.clone(),
        );

        let task_executor_main = task_executor.clone();
        task_executor_main.spawn_critical("producer or follower", async move {
            let beacon_engine_handle = node.add_ons_handle.beacon_engine_handle.clone();
            let provider = node.provider.clone();
            let chain_spec: std::sync::Arc<reth_ethereum::chainspec::ChainSpec> =
                node.config.chain.clone();
            let payload_builder_handle = node.payload_builder_handle.clone();
            let local_payload_attributes_builder =
                LocalPayloadAttributesBuilder::new(std::sync::Arc::new(chain_spec.clone()));

            let task_executor_consensus = task_executor.clone();

            let consensus = RbftConsensus::new(
                provider,
                node.pool.clone(),
                local_payload_attributes_builder,
                beacon_engine_handle,
                payload_builder_handle,
                chain_spec,
                task_executor_consensus,
                network.clone(),
                &args,
                metrics.clone(),
            )
            .expect("Failed to initialise RbftConsensus");

            // Periodically update metrics inside consensus
            // consensus.run() should internally call metrics updates as needed.
            //consensus.run_with_metrics().await
            consensus.run().await;
        });

        node_exit_future.await
    })?;

    Ok(())
}
