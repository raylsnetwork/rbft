// SPDX-License-Identifier: Apache-2.0
//! The wire protcol implementation for RBFT/QBFT messages.
//!
//! This module defines the message formats and encoding/decoding logic for
//! the RBFT/QBFT consensus protocol.
//!
//! It uses the reth_ethereum networking stack to handle peer connections and
//! message transmission.
//! The main components are:
//! - `Message`: Represents a QBFT message with encoding/decoding methods.
//! - `Connection`: Manages a protocol connection to a peer, handling incoming and outgoing
//!   messages.
//! - `RbftConnectionHandler`: Creates connections for incoming/outgoing peers.
//! - `RbftProtocolHandler`: Manages the overall protocol handler for RBFT/QBFT.
//! - `ProtocolState`: Maintains state related to protocol events and received messages.
//!
//! The protocol supports sending and receiving QBFT messages, relaying them
//! to other peers, and preventing message loops by tracking received messages.

/// Capacity of the per-connection outbound command channel.
/// In practice the number of pending outbound messages per peer should be low, but we set a high
/// capacity to avoid dropping messages under high load.
///
/// The open question is what will happen if we fail to send a command to a connection.
/// Will this break the state machine or will we recover?
const CONN_COMMAND_CHANNEL_CAPACITY: usize = 10_000;

use alloy_primitives::bytes::{Buf, BufMut, BytesMut};
use alloy_rlp::{Decodable, Encodable};
use futures::{ready, Stream, StreamExt};
use reth_ethereum::network::{
    api::{Direction, PeerId},
    eth_wire::{
        capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
        Capability,
    },
    protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler},
};
use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, trace, warn};

use rbft::QbftMessage;

/// Global message cache: sorted list of (height, RLP bytes) tuples
type MessageRecords = Vec<(u64, Box<[u8]>)>;

/// These events are sent to rbft_consensus when connections are established or disconnected.
#[derive(Debug)]
pub enum ProtocolEvent {
    /// Connection established.
    Established {
        /// Connection direction.
        direction: Direction,
        /// Peer ID.
        peer_id: PeerId,
        /// Remote socket address.
        remote_addr: SocketAddr,
        /// Sender part for forwarding commands.
        to_connection: mpsc::Sender<Command>,
    },
    /// Connection disconnected.
    Disconnected {
        /// Peer ID.
        peer_id: PeerId,
        /// Remote socket address for reconnection.
        remote_addr: SocketAddr,
        /// Reason for the disconnect event.
        reason: ProtocolDisconnectReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolDisconnectReason {
    IdleTimeout,
    StreamEnded,
    Dropped,
}

impl std::fmt::Display for ProtocolDisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolDisconnectReason::IdleTimeout => write!(f, "idle_timeout"),
            ProtocolDisconnectReason::StreamEnded => write!(f, "stream_ended"),
            ProtocolDisconnectReason::Dropped => write!(f, "dropped"),
        }
    }
}

/// Protocol state is a helper struct to store the protocol events.
#[derive(Clone, Debug)]
pub struct ProtocolState {
    /// Protocol event sender.
    pub events_sender: mpsc::Sender<ProtocolEvent>,

    /// Set of received message hashes to prevent loops.
    /// This is a vector sorted by height.
    pub messages_received: Arc<Mutex<MessageRecords>>,

    /// Per-peer inbound buffer high-water mark before pruning.
    pub _max_buffer_per_peer: usize,

    /// Per-peer inbound buffer size after pruning.
    pub _trim_to_per_peer: usize,

    /// Idle connection timeout; if None, idle timeout is disabled.
    pub idle_timeout: Option<Duration>,
}

impl ProtocolState {
    /// Create new protocol state.
    pub fn new(
        events_sender: mpsc::Sender<ProtocolEvent>,
        max_buffer_per_peer: usize,
        trim_to_per_peer: usize,
        idle_timeout: Option<Duration>,
    ) -> Self {
        let trim_to_per_peer = trim_to_per_peer.min(max_buffer_per_peer);
        Self {
            events_sender,
            messages_received: Arc::new(Mutex::new(Vec::new())),
            _max_buffer_per_peer: max_buffer_per_peer,
            _trim_to_per_peer: trim_to_per_peer,
            idle_timeout,
        }
    }
}

/// Commands that can be sent to a QbftProtocol connection.
#[derive(Debug)]
pub enum Command {
    /// Send a QBFT message to the peer.
    Message(Message),
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageId {
    Message = 0x00,
    Transactions = 0x01,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MessageKind {
    Message(Box<rbft::QbftMessage>),
    /// A batch of RLP-encoded Ethereum transactions.
    Transactions(Vec<alloy_primitives::Bytes>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub message_type: MessageId,
    pub message: MessageKind,
}

impl Message {
    pub fn capability() -> Capability {
        Capability::new_static("rbft", 1)
    }

    pub fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 2)
    }

    pub fn new_message(qbft_message: rbft::QbftMessage) -> Self {
        Self {
            message_type: MessageId::Message,
            message: MessageKind::Message(Box::new(qbft_message)),
        }
    }

    pub fn new_transactions(txs: Vec<alloy_primitives::Bytes>) -> Self {
        Self {
            message_type: MessageId::Transactions,
            message: MessageKind::Transactions(txs),
        }
    }

    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            MessageKind::Message(qbft_msg) => {
                qbft_msg.encode(&mut buf);
            }
            MessageKind::Transactions(txs) => {
                txs.encode(&mut buf);
            }
        }
        buf
    }

    pub fn decode_message(buf: &mut &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }
        let id = buf[0];
        buf.advance(1);
        let message_type = match id {
            0x00 => MessageId::Message,
            0x01 => MessageId::Transactions,
            _ => return None,
        };
        let message = match message_type {
            MessageId::Message => {
                let qbft_msg = QbftMessage::decode(buf).ok()?;
                MessageKind::Message(Box::new(qbft_msg))
            }
            MessageId::Transactions => {
                let txs = Vec::<alloy_primitives::Bytes>::decode(buf).ok()?;
                MessageKind::Transactions(txs)
            }
        };
        Some(Self {
            message_type,
            message,
        })
    }
}

/// A connection to a peer for the RBFT/QBFT protocol.
///
/// The connection represents a single peer connection and handles sending
/// and receiving messages.
///
/// Messages are relayed to other peers via the relay_tx channel.
pub struct Connection {
    conn: ProtocolConnection,
    peer_id: PeerId,
    /// Remote socket address for reconnection.
    remote_addr: SocketAddr,
    commands: ReceiverStream<Command>,
    message_tx: mpsc::Sender<rbft::QbftMessage>,
    transactions_tx: mpsc::Sender<Vec<alloy_primitives::Bytes>>,
    state: ProtocolState,
    _relay_tx: mpsc::Sender<(Message, PeerId)>,
    /// When the connection was established
    connected_at: Instant,
    /// Count of messages received from this peer
    messages_received_count: AtomicU64,
    /// Count of messages sent to this peer
    messages_sent_count: AtomicU64,
    /// Time of last activity (send or receive)
    last_activity: Arc<Mutex<Instant>>,
    /// Flag to track if disconnect event was already sent (prevents duplicate from Drop)
    disconnect_sent: Arc<std::sync::atomic::AtomicBool>,
    /// Last QBFT message height received from this peer.
    last_qbft_height: Option<u64>,
    /// Last QBFT message kind received from this peer.
    last_qbft_kind: Option<&'static str>,
    /// When the last QBFT message was received from this peer.
    last_qbft_at: Option<Instant>,
}

fn qbft_kind_str(message: &rbft::QbftMessage) -> &'static str {
    match message {
        rbft::QbftMessage::Proposal(_) => "proposal",
        rbft::QbftMessage::Prepare(_) => "prepare",
        rbft::QbftMessage::Commit(_) => "commit",
        rbft::QbftMessage::RoundChange(_) => "round_change",
        rbft::QbftMessage::NewBlock(_) => "new_block",
        rbft::QbftMessage::BlockRequest(_) => "block_request",
        rbft::QbftMessage::BlockResponse(_) => "block_response",
    }
}

impl Stream for Connection {
    type Item = BytesMut;

    /// This is called to request another message to send to the peer.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Idle connection timeout: drop the connection if no activity is observed for too long.
        if let Some(timeout) = this.state.idle_timeout {
            if let Ok(last) = this.last_activity.lock() {
                if last.elapsed() >= timeout {
                    let last_qbft_age_secs = this.last_qbft_at.map(|t| t.elapsed().as_secs());
                    warn!(
                        target: "qbft_protocol",
                        "Connection to peer {} at {} idle for {:?}, closing. \
                         last_qbft: kind={:?} height={:?} age_secs={:?}",
                        this.peer_id,
                        this.remote_addr,
                        last.elapsed(),
                        this.last_qbft_kind,
                        this.last_qbft_height,
                        last_qbft_age_secs
                    );
                    // Mark disconnect as sent to prevent duplicate from Drop handler
                    this.disconnect_sent
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    if let Err(e) = this
                        .state
                        .events_sender
                        .try_send(ProtocolEvent::Disconnected {
                            peer_id: this.peer_id,
                            remote_addr: this.remote_addr,
                            reason: ProtocolDisconnectReason::IdleTimeout,
                        })
                    {
                        warn!(
                            target: "qbft_protocol",
                            "Failed to send idle disconnect event for peer {}: {:?}",
                            this.peer_id,
                            e
                        );
                    }
                    return Poll::Ready(None);
                }
            }
        }

        // Poll for incoming commands
        if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
            trace!(target: "qbft_protocol", "Sending command to peer {}: {:?}", this.peer_id, cmd);
            // Update activity tracking
            this.messages_sent_count.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut last) = this.last_activity.lock() {
                *last = Instant::now();
            }
            return match cmd {
                Command::Message(msg) => Poll::Ready(Some(msg.encoded())),
            };
        }

        // Poll for incoming protocol messages
        let Some(msg_rlp) = ready!(this.conn.poll_next_unpin(cx)) else {
            // Connection closed - send disconnect event with detailed logging
            let connection_duration = this.connected_at.elapsed();
            let msgs_received = this.messages_received_count.load(Ordering::Relaxed);
            let msgs_sent = this.messages_sent_count.load(Ordering::Relaxed);
            let last_qbft_age_secs = this.last_qbft_at.map(|t| t.elapsed().as_secs());
            debug!(
                target: "qbft_protocol",
                "Connection to peer {} at {} closed (stream ended). \
                 Duration: {:?}, msgs_recv: {}, msgs_sent: {}, \
                 last_qbft: kind={:?} height={:?} age_secs={:?}",
                this.peer_id,
                this.remote_addr,
                connection_duration,
                msgs_received,
                msgs_sent,
                this.last_qbft_kind,
                this.last_qbft_height,
                last_qbft_age_secs
            );
            // Mark disconnect as sent to prevent duplicate from Drop handler
            this.disconnect_sent
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Err(e) = this
                .state
                .events_sender
                .try_send(ProtocolEvent::Disconnected {
                    peer_id: this.peer_id,
                    remote_addr: this.remote_addr,
                    reason: ProtocolDisconnectReason::StreamEnded,
                })
            {
                warn!(
                    target: "qbft_protocol",
                    "Failed to send disconnect event for peer {}: {:?}",
                    this.peer_id,
                    e
                );
            }
            return Poll::Ready(None);
        };

        if let Some(msg) = Message::decode_message(&mut &msg_rlp[..]) {
            match msg.message {
                MessageKind::Transactions(txs) => {
                    this.messages_received_count.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut last) = this.last_activity.lock() {
                        *last = Instant::now();
                    }
                    if let Err(e) = this.transactions_tx.try_send(txs) {
                        warn!(
                            target: "qbft_protocol",
                            "Failed to forward transactions from peer {}: {:?}",
                            this.peer_id,
                            e
                        );
                    }
                }
                MessageKind::Message(ref qbft_message) => {
                    // Update activity tracking
                    this.messages_received_count.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut last) = this.last_activity.lock() {
                        *last = Instant::now();
                    }
                    this.last_qbft_height = Some(qbft_message.height());
                    this.last_qbft_kind = Some(qbft_kind_str(qbft_message));
                    this.last_qbft_at = Some(Instant::now());

                    if let Err(e) = this.message_tx.try_send((**qbft_message).clone()) {
                        warn!(
                            target: "qbft_protocol",
                            "Failed to forward message to consensus engine from peer {}: {:?}",
                            this.peer_id,
                            e
                        );
                    }

                    // Old gossip code. Now probably not needed.

                    // let height = qbft_message.height();
                    // let mut messages_received = this.state.messages_received.lock().unwrap();

                    // // Find insertion point sorted by (height, RLP content) for total ordering
                    // // This ensures duplicates are always adjacent and can be detected in O(1)
                    // let pos = messages_received.partition_point(|(h, rlp)| {
                    //     *h < height || (*h == height && rlp.as_ref() < msg_rlp.as_ref())
                    // });

                    // Check if the message at insertion point is an exact duplicate
                    // let is_duplicate = messages_received
                    //     .get(pos)
                    //     .is_some_and(|(h, rlp)| *h == height && rlp.as_ref() ==
                    // msg_rlp.as_ref());

                    // if !is_duplicate {
                    //     messages_received.insert(pos, (height, msg_rlp.to_vec().into()));

                    //     // Global buffer management
                    //     let len = messages_received.len();
                    //     if len > this.state.max_buffer_per_peer {
                    //         let drained = len.saturating_sub(this.state.trim_to_per_peer);
                    //         warn!(target: "qbft_protocol",
                    //             "Global buffer high-watermark: draining {} (was {}, keep {})",
                    //             drained, len, this.state.trim_to_per_peer);
                    //         messages_received.drain(0..drained);
                    //     }

                    //     debug!(
                    //         target: "qbft_protocol",
                    //         "New msg from {}, relaying. Now {} messages total",
                    //         this.peer_id,
                    //         messages_received.len()
                    //     );

                    //     // Forward the QBFT message to the consensus engine
                    //     if let Err(e) = this.message_tx.send(qbft_message.clone()) {
                    //         warn!(
                    //             target: "qbft_protocol",
                    //             "Failed to forward message to consensus engine from peer {}:
                    // {:?}",             this.peer_id,
                    //             e
                    //         );
                    //     }

                    //     // Relay the message to other peers (excluding the sender)
                    //     // Note: we may need to send to a subset of peers.
                    //     // if let Err(e) = this.relay_tx.send((msg, this.peer_id)) {
                    //     //     warn!(
                    //     //         target: "qbft_protocol",
                    //     //         "Failed to relay message from peer {}: {:?}",
                    //     //         this.peer_id,
                    //     //         e
                    //     //     );
                    //     // }
                    // }
                }
            }
        }

        Poll::Pending
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Only send disconnect event if not already sent (prevents duplicates)
        if self
            .disconnect_sent
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            // Disconnect was already sent, skip duplicate
            debug!(
                target: "qbft_protocol",
                "Connection to peer {} at {} dropped (disconnect already sent)",
                self.peer_id,
                self.remote_addr
            );
            return;
        }

        // Send disconnect event when connection is dropped with detailed logging
        let connection_duration = self.connected_at.elapsed();
        let msgs_received = self.messages_received_count.load(Ordering::Relaxed);
        let msgs_sent = self.messages_sent_count.load(Ordering::Relaxed);
        let last_activity_secs = self
            .last_activity
            .lock()
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        let last_qbft_age_secs = self.last_qbft_at.map(|t| t.elapsed().as_secs());

        debug!(
            target: "qbft_protocol",
            "Connection to peer {} at {} dropped. Duration: {:?}, msgs_recv: {}, \
             msgs_sent: {}, last_activity: {}s ago, \
             last_qbft: kind={:?} height={:?} age_secs={:?}",
            self.peer_id,
            self.remote_addr,
            connection_duration,
            msgs_received,
            msgs_sent,
            last_activity_secs,
            self.last_qbft_kind,
            self.last_qbft_height,
            last_qbft_age_secs
        );
        if let Err(e) = self
            .state
            .events_sender
            .try_send(ProtocolEvent::Disconnected {
                peer_id: self.peer_id,
                remote_addr: self.remote_addr,
                reason: ProtocolDisconnectReason::Dropped,
            })
        {
            warn!(
                target: "qbft_protocol",
                "Failed to send disconnect event for peer {} during drop: {:?}",
                self.peer_id,
                e
            );
        }
    }
}

pub struct RbftConnectionHandler {
    pub state: ProtocolState,
    pub message_tx: mpsc::Sender<rbft::QbftMessage>,
    pub transactions_tx: mpsc::Sender<Vec<alloy_primitives::Bytes>>,
    pub relay_tx: mpsc::Sender<(Message, PeerId)>,
    /// Remote socket address for this connection.
    pub remote_addr: SocketAddr,
}

impl ConnectionHandler for RbftConnectionHandler {
    type Connection = Connection;

    fn protocol(&self) -> Protocol {
        Message::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        // Keep the network session alive even when the remote peer doesn't
        // support QBFT yet (e.g. during startup before it registers the
        // subprotocol).  Disconnecting here causes excessive connection churn
        // that destabilises the first consensus rounds.  Instead, a periodic
        // connection audit in the consensus loop detects network-level peers
        // missing a QBFT connection and forces a targeted reconnect.
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let (tx, rx) = mpsc::channel(CONN_COMMAND_CHANNEL_CAPACITY);

        debug!(
            target: "qbft_protocol",
            "Creating new connection to peer {} at {} (direction: {:?})",
            peer_id,
            self.remote_addr,
            direction
        );

        // Announce connection established
        if let Err(e) = self
            .state
            .events_sender
            .try_send(ProtocolEvent::Established {
                direction,
                peer_id,
                remote_addr: self.remote_addr,
                to_connection: tx,
            })
        {
            warn!(
                target: "qbft_protocol",
                "Failed to send connection established event for peer {}: {:?}",
                peer_id,
                e
            );
        }

        let now = Instant::now();
        Connection {
            conn,
            peer_id,
            remote_addr: self.remote_addr,
            commands: ReceiverStream::new(rx),
            message_tx: self.message_tx.clone(),
            transactions_tx: self.transactions_tx.clone(),
            state: self.state.clone(),
            _relay_tx: self.relay_tx.clone(),
            connected_at: now,
            messages_received_count: AtomicU64::new(0),
            messages_sent_count: AtomicU64::new(0),
            last_activity: Arc::new(Mutex::new(now)),
            disconnect_sent: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_qbft_height: None,
            last_qbft_kind: None,
            last_qbft_at: None,
        }
    }
}

#[derive(Debug)]
pub struct RbftProtocolHandler {
    pub state: ProtocolState,
    pub message_tx: mpsc::Sender<rbft::QbftMessage>,
    pub transactions_tx: mpsc::Sender<Vec<alloy_primitives::Bytes>>,
    pub relay_tx: mpsc::Sender<(Message, PeerId)>,
}

impl ProtocolHandler for RbftProtocolHandler {
    type ConnectionHandler = RbftConnectionHandler;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// Returns a connection handler.
    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(RbftConnectionHandler {
            state: self.state.clone(),
            message_tx: self.message_tx.clone(),
            transactions_tx: self.transactions_tx.clone(),
            relay_tx: self.relay_tx.clone(),
            remote_addr: socket_addr,
        })
    }

    /// Invoked when a new outgoing connection to the remote is requested.
    ///
    /// Returns a connection handler.
    fn on_outgoing(
        &self,
        socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(RbftConnectionHandler {
            state: self.state.clone(),
            message_tx: self.message_tx.clone(),
            transactions_tx: self.transactions_tx.clone(),
            relay_tx: self.relay_tx.clone(),
            remote_addr: socket_addr,
        })
    }
}
