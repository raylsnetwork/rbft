// SPDX-License-Identifier: Apache-2.0
//! Log file aggregator and chronological viewer
//!
//! This module implements the logjam command which tails multiple log files
//! and outputs them in chronological order based on timestamps.
//!
//!
//! State summary strings have a format like this
//!
//! `[val0 h=20 bt=0 rt=-2 chain=19/1769512106 r=0 prva=111 in=20.0:Ppppp] o=20.0:c`
//!
//! The chain may have a comma separated list of values.
//!
//! Each message is like this
//!
//! struct Message { height: u64, round: u64, kind: char }
//!
//! In the example above:
//!
//! 20.0:Ppppp represents five input messages all with height=20 round=0 but with kind 'P' and 'p'.
//!
//! The o=20.0:c represents one output message with height=20 round=0 and kind 'c'.

use chrono::{DateTime, Utc};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::Duration;

/// A message identifier with cardinality tracking
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MessageKey {
    height: u64,
    round: u64,
    kind: char,
}

/// Information about a broadcast message
#[derive(Debug, Clone)]
struct MessageInfo {
    /// Validators who broadcast this message with their send timestamps
    senders: HashMap<String, DateTime<Utc>>,
    /// Map of validator -> timestamp when they received this message
    receivers_by_node: HashMap<String, DateTime<Utc>>,
    /// Timestamp when this message was first broadcast
    timestamp: DateTime<Utc>,
    /// First sender of this message (for debugging/future use)
    #[allow(dead_code)]
    first_sender: String,
}

/// Network state for tracking broadcasts and metrics
struct NetworkState {
    /// Track broadcast messages across all nodes
    broadcast_tracker: HashMap<MessageKey, MessageInfo>,
    /// Delivery delay histogram
    histogram: DeliveryHistogram,
    /// Last timestamp seen (for time-based separators)
    last_timestamp: Option<DateTime<Utc>>,
    /// First timestamp seen (used as reference point for relative times)
    first_timestamp: Option<DateTime<Utc>>,
    /// Maximum message delay in milliseconds
    max_message_delay_ms: u64,
    /// Quiet mode flag
    quiet: bool,
    /// Track which messages have been reported as not received for each node
    /// Key: (node_name, MessageKey), Value: ()
    reported_unreceived: HashSet<(String, MessageKey)>,
    /// Wire-level trace mode
    trace: bool,
    /// Wire-level message tracker (only active when trace=true)
    wire_tracker: Option<WireTracker>,
}

impl NetworkState {
    fn new(max_message_delay_ms: u64, bucket_size_ms: u64, quiet: bool, trace: bool) -> Self {
        Self {
            broadcast_tracker: HashMap::new(),
            histogram: DeliveryHistogram::new(max_message_delay_ms, bucket_size_ms),
            last_timestamp: None,
            first_timestamp: None,
            max_message_delay_ms,
            quiet,
            reported_unreceived: HashSet::new(),
            trace,
            wire_tracker: if trace {
                Some(WireTracker::new())
            } else {
                None
            },
        }
    }
}

/// A wire-level trace event parsed from msg_trace log lines
#[derive(Debug)]
enum TraceEvent {
    Send {
        h: u64,
        r: u64,
        t: char,
        to_peer: String,
    },
    Recv {
        h: u64,
        r: u64,
        t: char,
        from_peer: String,
    },
    RecvDup {
        h: u64,
        r: u64,
        t: char,
        from_peer: String,
    },
    Drain {
        count: u64,
        h_range: String,
        triggered_by: String,
    },
    Relay {
        h: u64,
        r: u64,
        t: char,
        from_peer: String,
        to_peers: u64,
        failed: u64,
    },
}

/// Wire-level delivery state for a message type at a given height/round
struct WireDelivery {
    /// Nodes that SENT (broadcast) this message type, with earliest timestamp
    senders: HashMap<String, DateTime<Utc>>,
    /// Nodes that RECV'd this message type, with earliest timestamp
    receivers: HashMap<String, DateTime<Utc>>,
    /// Nodes that received duplicates
    dup_receivers: HashSet<String>,
    /// Earliest SEND timestamp across all senders
    first_send: Option<DateTime<Utc>>,
}

impl WireDelivery {
    fn new() -> Self {
        Self {
            senders: HashMap::new(),
            receivers: HashMap::new(),
            dup_receivers: HashSet::new(),
            first_send: None,
        }
    }
}

/// Tracks wire-level message delivery across all nodes
struct WireTracker {
    /// Per-message delivery state
    messages: HashMap<MessageKey, WireDelivery>,
    /// All known node names (discovered from log file sources)
    known_nodes: HashSet<String>,
    /// Period counters (reset after each dump)
    send_count: u64,
    recv_count: u64,
    dup_count: u64,
    relay_count: u64,
    relay_fail_count: u64,
    drain_event_count: u64,
    drain_msg_count: u64,
    /// Wire delivery latencies in milliseconds (period, reset after dump)
    wire_latencies: Vec<u64>,
    /// Lifetime counters (never reset, used in final summary)
    lifetime_sends: u64,
    lifetime_recvs: u64,
    lifetime_dups: u64,
    lifetime_relays: u64,
    lifetime_relay_fails: u64,
    lifetime_drain_events: u64,
    lifetime_drain_msgs: u64,
    lifetime_latency_count: u64,
    lifetime_latency_sum: u64,
    lifetime_latency_max: u64,
    /// Last time periodic stats were dumped
    last_dump: DateTime<Utc>,
}

impl WireTracker {
    fn new() -> Self {
        Self {
            messages: HashMap::new(),
            known_nodes: HashSet::new(),
            send_count: 0,
            recv_count: 0,
            dup_count: 0,
            relay_count: 0,
            relay_fail_count: 0,
            drain_event_count: 0,
            drain_msg_count: 0,
            wire_latencies: Vec::new(),
            lifetime_sends: 0,
            lifetime_recvs: 0,
            lifetime_dups: 0,
            lifetime_relays: 0,
            lifetime_relay_fails: 0,
            lifetime_drain_events: 0,
            lifetime_drain_msgs: 0,
            lifetime_latency_count: 0,
            lifetime_latency_sum: 0,
            lifetime_latency_max: 0,
            last_dump: Utc::now(),
        }
    }

    /// Record a node as known (discovered from log file source)
    fn note_node(&mut self, node: &str) {
        self.known_nodes.insert(node.to_string());
    }

    /// Record a SEND event
    fn record_send(&mut self, key: MessageKey, node: &str, timestamp: DateTime<Utc>) {
        self.send_count += 1;
        self.lifetime_sends += 1;
        let delivery = self.messages.entry(key).or_insert_with(WireDelivery::new);
        delivery
            .senders
            .entry(node.to_string())
            .or_insert(timestamp);
        if delivery.first_send.is_none() || Some(timestamp) < delivery.first_send {
            delivery.first_send = Some(timestamp);
        }
    }

    /// Record a RECV event, returns wire latency in ms if calculable
    fn record_recv(
        &mut self,
        key: MessageKey,
        node: &str,
        timestamp: DateTime<Utc>,
    ) -> Option<u64> {
        self.recv_count += 1;
        self.lifetime_recvs += 1;
        let delivery = self.messages.entry(key).or_insert_with(WireDelivery::new);
        delivery
            .receivers
            .entry(node.to_string())
            .or_insert(timestamp);

        // Calculate wire latency from first send
        if let Some(first_send) = delivery.first_send {
            let latency_ms = timestamp
                .signed_duration_since(first_send)
                .num_milliseconds()
                .max(0) as u64;
            self.wire_latencies.push(latency_ms);
            self.lifetime_latency_count += 1;
            self.lifetime_latency_sum += latency_ms;
            if latency_ms > self.lifetime_latency_max {
                self.lifetime_latency_max = latency_ms;
            }
            Some(latency_ms)
        } else {
            None
        }
    }

    /// Record a RECV_DUP event
    fn record_dup(&mut self, key: MessageKey, node: &str) {
        self.dup_count += 1;
        self.lifetime_dups += 1;
        let delivery = self.messages.entry(key).or_insert_with(WireDelivery::new);
        delivery.dup_receivers.insert(node.to_string());
    }

    /// Record a RELAY event
    fn record_relay(&mut self, _to_peers: u64, failed: u64) {
        self.relay_count += 1;
        self.lifetime_relays += 1;
        self.relay_fail_count += failed;
        self.lifetime_relay_fails += failed;
    }

    /// Record a DRAIN event
    fn record_drain(&mut self, count: u64) {
        self.drain_event_count += 1;
        self.lifetime_drain_events += 1;
        self.drain_msg_count += count;
        self.lifetime_drain_msgs += count;
    }

    /// Check if it's time to dump periodic stats (every 5 seconds)
    fn should_dump(&self, current_time: &DateTime<Utc>) -> bool {
        current_time
            .signed_duration_since(self.last_dump)
            .num_seconds()
            >= 5
    }

    /// Dump periodic wire stats and reset period counters.
    /// Always produces output so the user sees wire trace status every cycle.
    fn dump_and_reset(&mut self, current_time: DateTime<Utc>) -> String {
        // Period stats line
        let mut out = format!(
            "Wire trace: [SEND={} RECV={} DUP={} RELAY={}({} fail) DRAIN={}({} msgs)]",
            self.send_count,
            self.recv_count,
            self.dup_count,
            self.relay_count,
            self.relay_fail_count,
            self.drain_event_count,
            self.drain_msg_count,
        );

        if !self.wire_latencies.is_empty() {
            let mut sorted = self.wire_latencies.clone();
            sorted.sort();
            let len = sorted.len();
            let avg = sorted.iter().sum::<u64>() as f64 / len as f64;
            let p50 = sorted[len / 2];
            let p95 = sorted[std::cmp::min((len as f64 * 0.95) as usize, len - 1)];
            let max = sorted[len - 1];
            out.push_str(&format!(
                " latency: avg={:.1}ms p50={}ms p95={}ms max={}ms",
                avg, p50, p95, max
            ));
        }

        // Lifetime totals
        out.push_str(&format!(
            " | Lifetime: SEND={} RECV={} DUP={}",
            self.lifetime_sends, self.lifetime_recvs, self.lifetime_dups,
        ));

        if self.lifetime_latency_count > 0 {
            let avg = self.lifetime_latency_sum as f64 / self.lifetime_latency_count as f64;
            out.push_str(&format!(
                " avg={:.1}ms max={}ms",
                avg, self.lifetime_latency_max,
            ));
        }

        // Count messages with incomplete wire delivery
        let missing_count = self.count_incomplete_deliveries();
        out.push_str(&format!(" missing_delivery={}", missing_count));

        // Reset period counters
        self.send_count = 0;
        self.recv_count = 0;
        self.dup_count = 0;
        self.relay_count = 0;
        self.relay_fail_count = 0;
        self.drain_event_count = 0;
        self.drain_msg_count = 0;
        self.wire_latencies.clear();
        self.last_dump = current_time;

        out
    }

    /// Count messages where at least one known node neither sent nor received
    fn count_incomplete_deliveries(&self) -> usize {
        self.messages
            .iter()
            .filter(|(_, delivery)| {
                if delivery.senders.is_empty() {
                    return false;
                }
                self.known_nodes.iter().any(|n| {
                    !delivery.senders.contains_key(n.as_str())
                        && !delivery.receivers.contains_key(n.as_str())
                })
            })
            .count()
    }

    /// Generate final wire trace summary string (uses lifetime counters)
    fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str("=== Wire Trace Summary ===\n");
        out.push_str(&format!(
            "Lifetime: SEND={} RECV={} DUP={} RELAY={}({} failed) DRAIN={}({} msgs)\n",
            self.lifetime_sends,
            self.lifetime_recvs,
            self.lifetime_dups,
            self.lifetime_relays,
            self.lifetime_relay_fails,
            self.lifetime_drain_events,
            self.lifetime_drain_msgs,
        ));

        // Wire latency stats from lifetime counters
        if self.lifetime_latency_count > 0 {
            let avg = self.lifetime_latency_sum as f64 / self.lifetime_latency_count as f64;
            out.push_str(&format!(
                "Wire latency: avg={:.1}ms max={}ms ({} samples)\n",
                avg, self.lifetime_latency_max, self.lifetime_latency_count,
            ));
        }

        // Find messages with incomplete wire delivery
        let mut missing_entries: Vec<String> = Vec::new();
        let mut keys: Vec<&MessageKey> = self.messages.keys().collect();
        keys.sort_by(|a, b| {
            a.height
                .cmp(&b.height)
                .then(a.round.cmp(&b.round))
                .then(a.kind.cmp(&b.kind))
        });

        for key in keys {
            let delivery = &self.messages[key];
            if delivery.senders.is_empty() {
                continue;
            }
            // Nodes that neither sent nor received this message
            let mut missing: Vec<&String> = self
                .known_nodes
                .iter()
                .filter(|n| {
                    !delivery.senders.contains_key(n.as_str())
                        && !delivery.receivers.contains_key(n.as_str())
                })
                .collect();
            if !missing.is_empty() {
                missing.sort();
                let mut senders: Vec<&String> = delivery.senders.keys().collect();
                senders.sort();
                missing_entries.push(format!(
                    "  {}.{}:{} sent_by=[{}] missing=[{}]",
                    key.height,
                    key.round,
                    key.kind,
                    senders
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ));
            }
        }

        if !missing_entries.is_empty() {
            out.push_str(&format!(
                "Messages with incomplete wire delivery ({}):\n",
                missing_entries.len()
            ));
            for entry in missing_entries.iter().take(50) {
                out.push_str(entry);
                out.push('\n');
            }
            if missing_entries.len() > 50 {
                out.push_str(&format!("  ... and {} more\n", missing_entries.len() - 50));
            }
        } else {
            out.push_str("All messages delivered at wire level.\n");
        }

        out
    }
}

/// Histogram tracking message delivery delays in 100ms buckets
#[derive(Debug, Clone)]
struct DeliveryHistogram {
    /// Buckets: index 0 = 0-100ms, index 1 = 100-200ms, etc.
    /// Last bucket contains everything >= max_delay_ms
    buckets: Vec<u64>,
    /// Total messages tracked
    total_messages: u64,
    /// Messages that were never received before height changed
    never_received: u64,
    /// Last time histogram was dumped
    last_dump: DateTime<Utc>,
    /// Bucket size in milliseconds
    bucket_size_ms: u64,
    /// Maximum delay to track
    max_delay_ms: u64,
    /// Count of WARN level logs
    warn_count: u64,
    /// Count of ERROR level logs
    error_count: u64,
}

impl DeliveryHistogram {
    fn new(max_delay_ms: u64, bucket_size_ms: u64) -> Self {
        let num_buckets = (max_delay_ms / bucket_size_ms) as usize + 1; // +1 for overflow bucket
        Self {
            buckets: vec![0; num_buckets],
            total_messages: 0,
            never_received: 0,
            last_dump: Utc::now(),
            bucket_size_ms,
            max_delay_ms,
            warn_count: 0,
            error_count: 0,
        }
    }

    /// Record a message delivery with its delay in milliseconds
    fn record(&mut self, delay_ms: u64) {
        self.total_messages += 1;
        let bucket_index = std::cmp::min(
            (delay_ms / self.bucket_size_ms) as usize,
            self.buckets.len() - 1,
        );
        self.buckets[bucket_index] += 1;
    }

    /// Record a WARN level log
    fn record_warn(&mut self) {
        self.warn_count += 1;
    }

    /// Record an ERROR level log
    fn record_error(&mut self) {
        self.error_count += 1;
    }

    /// Check if it's time to dump (every 5 seconds)
    fn should_dump(&self, current_time: &DateTime<Utc>) -> bool {
        current_time
            .signed_duration_since(self.last_dump)
            .num_seconds()
            >= 5
    }

    /// Dump histogram as a single line and reset
    fn dump_and_reset(&mut self, current_time: DateTime<Utc>) -> String {
        if self.total_messages == 0 && self.never_received == 0 {
            self.last_dump = current_time;
            return String::new();
        }

        let mut output = format!(
            "Message stats: [total={} never_received={}",
            self.total_messages, self.never_received
        );

        // Add WARN and ERROR counts if present
        if self.warn_count > 0 {
            output.push_str(&format!(" WARN={}", self.warn_count));
        }
        if self.error_count > 0 {
            output.push_str(&format!(" ERROR={}", self.error_count));
        }
        output.push_str("] ");

        for (i, &count) in self.buckets.iter().enumerate() {
            if count > 0 {
                let percentage = (count as f64 / self.total_messages as f64) * 100.0;
                let range_start = i as u64 * self.bucket_size_ms;
                let range_end = if i == self.buckets.len() - 1 {
                    format!("{}+", self.max_delay_ms)
                } else {
                    format!("{}", range_start + self.bucket_size_ms)
                };
                output.push_str(&format!(
                    "{}-{}ms: {:.1}% ",
                    range_start, range_end, percentage
                ));
            }
        }

        // Reset for next period
        self.buckets.fill(0);
        self.total_messages = 0;
        self.never_received = 0;
        self.warn_count = 0;
        self.error_count = 0;
        self.last_dump = current_time;

        output
    }
}

/// A log line with its source file and parsed timestamp
#[derive(Debug, Clone)]
struct LogLine {
    source: String,
    timestamp: Option<DateTime<Utc>>,
    line: String,
}

impl Eq for LogLine {}

impl PartialEq for LogLine {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp && self.line == other.line
    }
}

impl Ord for LogLine {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse ordering for BinaryHeap (it's a max heap, we want min)
        // Lines without timestamps come first (None < Some)
        match (&self.timestamp, &other.timestamp) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => b.cmp(a), // Reverse for min-heap
        }
    }
}

impl PartialOrd for LogLine {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Parse a timestamp from a log line
/// Expected format: 2026-01-27T11:08:07.070327Z
fn parse_timestamp(line: &str) -> Option<DateTime<Utc>> {
    // Find the first space to isolate the timestamp
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    if parts.is_empty() {
        return None;
    }

    let timestamp_str = parts[0];
    DateTime::parse_from_rfc3339(timestamp_str)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Parse messages from a message string like "20.0:Ppppp"
/// Returns a vec of MessageKeys
fn parse_messages(msg_str: &str) -> Vec<MessageKey> {
    let mut messages = Vec::new();

    if msg_str.is_empty() {
        return messages;
    }

    // Split by comma in case there are multiple message groups
    for part in msg_str.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // Parse format: height.round:kinds
        if let Some(colon_pos) = part.find(':') {
            let hr_part = &part[..colon_pos];
            let kinds_part = &part[colon_pos + 1..];

            // Parse height.round
            if let Some(dot_pos) = hr_part.find('.') {
                if let (Ok(height), Ok(round)) = (
                    hr_part[..dot_pos].parse::<u64>(),
                    hr_part[dot_pos + 1..].parse::<u64>(),
                ) {
                    // Parse each kind character
                    for kind in kinds_part.chars() {
                        messages.push(MessageKey {
                            height,
                            round,
                            kind,
                        });
                    }
                }
            }
        }
    }

    messages
}

/// Parse the validator name from the log source
/// Source format: "node0.log" -> "node0"
fn parse_validator_name(source: &str) -> Option<String> {
    // Remove the .log extension if present
    if let Some(name) = source.strip_suffix(".log") {
        return Some(name.to_string());
    }
    Some(source.to_string())
}

/// Parse output messages from a log line
/// Looks for "o=height.round:kinds" pattern
fn parse_output_messages(line: &str) -> Vec<MessageKey> {
    if let Some(o_pos) = line.find(" o=") {
        let after_o = &line[o_pos + 3..];
        // Find the end (either space or end of line)
        let msg_str = after_o.split_whitespace().next().unwrap_or("");
        return parse_messages(msg_str);
    }
    Vec::new()
}

/// Parse input messages from a log line
/// Looks for "in=height.round:kinds" pattern
fn parse_input_messages(line: &str) -> Vec<MessageKey> {
    if let Some(in_pos) = line.find(" in=") {
        let after_in = &line[in_pos + 4..];
        // Find the closing bracket
        if let Some(bracket_pos) = after_in.find(']') {
            let msg_str = &after_in[..bracket_pos];
            return parse_messages(msg_str);
        }
    }
    Vec::new()
}

/// Check if a line is a broadcast message (b [...])
fn is_before_line(line: &str) -> bool {
    line.contains(" b [")
}

/// Check if a line is an accept message (a [...])
fn is_after_line(line: &str) -> bool {
    line.contains(" a [")
}

/// Parse key=value pairs from a list of whitespace-separated tokens
fn parse_trace_kvs<'a>(parts: &[&'a str]) -> HashMap<&'a str, &'a str> {
    let mut kvs = HashMap::new();
    for part in parts {
        if let Some(eq_pos) = part.find('=') {
            let key = &part[..eq_pos];
            let value = &part[eq_pos + 1..];
            kvs.insert(key, value);
        }
    }
    kvs
}

/// Parse a msg_trace log line into a TraceEvent.
/// Returns None if the line is not a trace line or cannot be parsed.
///
/// Expected formats:
///   `... msg_trace: SEND h=20 r=0 t=c to=abc12345`
///   `... msg_trace: RECV h=20 r=0 t=c from=abc12345`
///   `... msg_trace: RECV_DUP h=20 r=0 t=c from=abc12345`
///   `... msg_trace: DRAIN count=5 h_range=18..20 triggered_by=abc12345`
///   `... msg_trace: RELAY h=20 r=0 t=c from=abc1 to_peers=3 failed=0`
fn parse_trace_line(line: &str) -> Option<TraceEvent> {
    let marker = "msg_trace: ";
    let trace_start = line.find(marker)?;
    let trace_content = &line[trace_start + marker.len()..];

    let parts: Vec<&str> = trace_content.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let event_type = parts[0];
    let kvs = parse_trace_kvs(&parts[1..]);

    match event_type {
        "SEND" => {
            let h = kvs.get("h")?.parse().ok()?;
            let r = kvs.get("r")?.parse().ok()?;
            let t = kvs.get("t")?.chars().next()?;
            let to_peer = kvs.get("to")?.to_string();
            Some(TraceEvent::Send { h, r, t, to_peer })
        }
        "RECV" => {
            let h = kvs.get("h")?.parse().ok()?;
            let r = kvs.get("r")?.parse().ok()?;
            let t = kvs.get("t")?.chars().next()?;
            let from_peer = kvs.get("from")?.to_string();
            Some(TraceEvent::Recv { h, r, t, from_peer })
        }
        "RECV_DUP" => {
            let h = kvs.get("h")?.parse().ok()?;
            let r = kvs.get("r")?.parse().ok()?;
            let t = kvs.get("t")?.chars().next()?;
            let from_peer = kvs.get("from")?.to_string();
            Some(TraceEvent::RecvDup { h, r, t, from_peer })
        }
        "DRAIN" => {
            let count = kvs.get("count")?.parse().ok()?;
            let h_range = kvs.get("h_range")?.to_string();
            let triggered_by = kvs.get("triggered_by")?.to_string();
            Some(TraceEvent::Drain {
                count,
                h_range,
                triggered_by,
            })
        }
        "RELAY" => {
            let h = kvs.get("h")?.parse().ok()?;
            let r = kvs.get("r")?.parse().ok()?;
            let t = kvs.get("t")?.chars().next()?;
            let from_peer = kvs.get("from")?.to_string();
            let to_peers = kvs.get("to_peers")?.parse().ok()?;
            let failed = kvs.get("failed")?.parse().ok()?;
            Some(TraceEvent::Relay {
                h,
                r,
                t,
                from_peer,
                to_peers,
                failed,
            })
        }
        _ => None,
    }
}

/// Record a trace event in the wire tracker without printing
fn record_trace_event(
    event: &TraceEvent,
    node: &str,
    timestamp: DateTime<Utc>,
    tracker: &mut WireTracker,
) {
    match event {
        TraceEvent::Send { h, r, t, .. } => {
            let key = MessageKey {
                height: *h,
                round: *r,
                kind: *t,
            };
            tracker.record_send(key, node, timestamp);
        }
        TraceEvent::Recv { h, r, t, .. } => {
            let key = MessageKey {
                height: *h,
                round: *r,
                kind: *t,
            };
            tracker.record_recv(key, node, timestamp);
        }
        TraceEvent::RecvDup { h, r, t, .. } => {
            let key = MessageKey {
                height: *h,
                round: *r,
                kind: *t,
            };
            tracker.record_dup(key, node);
        }
        TraceEvent::Drain { count, .. } => {
            tracker.record_drain(*count);
        }
        TraceEvent::Relay {
            to_peers, failed, ..
        } => {
            tracker.record_relay(*to_peers, *failed);
        }
    }
}

/// Format and print a trace event, updating the wire tracker
fn format_trace_event<W: std::io::Write>(
    event: &TraceEvent,
    node: &str,
    timestamp: DateTime<Utc>,
    tracker: &mut WireTracker,
    writer: &mut W,
    first_timestamp: Option<&DateTime<Utc>>,
) -> std::io::Result<()> {
    let time_str = first_timestamp
        .map(|ft| {
            let offset = (timestamp.timestamp_millis() - ft.timestamp_millis()) as f64 / 1000.0;
            format!("({:.3}s) ", offset)
        })
        .unwrap_or_default();

    match event {
        TraceEvent::Send { h, r, t, to_peer } => {
            let key = MessageKey {
                height: *h,
                round: *r,
                kind: *t,
            };
            tracker.record_send(key, node, timestamp);
            writeln!(
                writer,
                "  [TRACE] {}{} SEND {}.{}:{} -> {}",
                time_str, node, h, r, t, to_peer
            )?;
        }
        TraceEvent::Recv { h, r, t, from_peer } => {
            let key = MessageKey {
                height: *h,
                round: *r,
                kind: *t,
            };
            let latency = tracker.record_recv(key, node, timestamp);
            let lat_str = latency.map(|l| format!(" ({}ms)", l)).unwrap_or_default();
            writeln!(
                writer,
                "  [TRACE] {}{} RECV {}.{}:{} <- {}{}",
                time_str, node, h, r, t, from_peer, lat_str
            )?;
        }
        TraceEvent::RecvDup { h, r, t, from_peer } => {
            let key = MessageKey {
                height: *h,
                round: *r,
                kind: *t,
            };
            tracker.record_dup(key, node);
            writeln!(
                writer,
                "  [TRACE] {}{} RECV_DUP {}.{}:{} <- {}",
                time_str, node, h, r, t, from_peer
            )?;
        }
        TraceEvent::Drain {
            count,
            h_range,
            triggered_by,
        } => {
            tracker.record_drain(*count);
            writeln!(
                writer,
                "  [TRACE] {}{} DRAIN {} msgs h={} (triggered by {})",
                time_str, node, count, h_range, triggered_by
            )?;
        }
        TraceEvent::Relay {
            h,
            r,
            t,
            from_peer,
            to_peers,
            failed,
        } => {
            tracker.record_relay(*to_peers, *failed);
            let fail_str = if *failed > 0 {
                format!(", {} failed", failed)
            } else {
                String::new()
            };
            writeln!(
                writer,
                "  [TRACE] {}{} RELAY {}.{}:{} from {} -> {} peers{}",
                time_str, node, h, r, t, from_peer, to_peers, fail_str
            )?;
        }
    }
    Ok(())
}

/// A file reader that tracks its position
struct LogFileReader {
    reader: BufReader<File>,
    name: String,
}

impl LogFileReader {
    fn new(path: PathBuf) -> eyre::Result<Self> {
        let file = File::open(&path)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let reader = BufReader::new(file);
        Ok(Self { reader, name })
    }

    fn read_line(&mut self, buf: &mut String) -> eyre::Result<usize> {
        Ok(self.reader.read_line(buf)?)
    }
}

/// Run the logjam command to tail and merge log files
pub async fn logjam(
    logs_dir: Option<&str>,
    follow: bool,
    max_message_delay_ms: u64,
    bucket_size_ms: u64,
    quiet: bool,
    trace: bool,
) -> eyre::Result<()> {
    // Resolve the logs directory path
    let logs_path = if let Some(dir) = logs_dir {
        PathBuf::from(dir)
    } else {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".rbft/testnet/logs")
    };

    if !logs_path.exists() {
        return Err(eyre::eyre!(
            "Logs directory does not exist: {}",
            logs_path.display()
        ));
    }

    println!("Logjam monitoring logs in: {}", logs_path.display());
    if trace {
        println!("Wire-level trace mode enabled (requires RUST_LOG=msg_trace=debug on nodes)");
    }

    // Find all node*.log files
    let mut log_files = Vec::new();
    for entry in std::fs::read_dir(&logs_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("node") && name.ends_with(".log") {
                    log_files.push(path);
                }
            }
        }
    }

    if log_files.is_empty() {
        return Err(eyre::eyre!(
            "No node*.log files found in {}",
            logs_path.display()
        ));
    }

    log_files.sort();
    println!("Found {} log files", log_files.len());

    // Open all log files
    let mut readers: Vec<LogFileReader> = log_files
        .into_iter()
        .filter_map(|path| LogFileReader::new(path).ok())
        .collect();

    if readers.is_empty() {
        return Err(eyre::eyre!("Could not open any log files"));
    }

    // Initialize network state
    let mut state = NetworkState::new(max_message_delay_ms, bucket_size_ms, quiet, trace);

    // Read all existing lines into a heap
    let mut heap: BinaryHeap<LogLine> = BinaryHeap::new();

    for reader in &mut readers {
        let mut line = String::new();
        while reader.read_line(&mut line)? > 0 {
            let timestamp = parse_timestamp(&line);
            heap.push(LogLine {
                source: reader.name.clone(),
                timestamp,
                line: line.trim_end().to_string(),
            });
            line.clear();
        }
    }

    // Print all existing lines in order
    let mut sorted_lines: Vec<LogLine> = heap.into_iter().collect();
    sorted_lines.sort_by(|a, b| match (&a.timestamp, &b.timestamp) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => a.cmp(b),
    });

    let mut stdout = std::io::stdout();
    for log_line in sorted_lines {
        process_and_print_line(&log_line, &mut state, &mut stdout)?;
    }

    // Dump final histogram if there's data
    if state.histogram.total_messages > 0 {
        let final_histogram = state.histogram.dump_and_reset(Utc::now());
        if !final_histogram.is_empty() {
            println!("{}", final_histogram);
        }
    }

    // Dump remaining wire trace period stats then final summary
    if let Some(ref mut tracker) = state.wire_tracker {
        let remaining = tracker.dump_and_reset(Utc::now());
        if !remaining.is_empty() {
            println!("{}", remaining);
        }
        println!("{}", tracker.summary());
    }

    // If follow mode is enabled, tail the files for new lines
    if !follow {
        return Ok(());
    }

    // Now tail the files for new lines
    println!("\n--- Tailing for new lines ---\n");

    loop {
        let mut any_new_lines = false;
        let mut new_heap: BinaryHeap<LogLine> = BinaryHeap::new();

        for reader in &mut readers {
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                any_new_lines = true;
                let timestamp = parse_timestamp(&line);
                new_heap.push(LogLine {
                    source: reader.name.clone(),
                    timestamp,
                    line: line.trim_end().to_string(),
                });
                line.clear();
            }
        }

        if any_new_lines {
            // Print new lines in order
            let mut sorted_lines: Vec<LogLine> = new_heap.into_iter().collect();
            sorted_lines.sort_by(|a, b| match (&a.timestamp, &b.timestamp) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a), Some(b)) => a.cmp(b),
            });

            for log_line in sorted_lines {
                process_and_print_line(&log_line, &mut state, &mut stdout)?;
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Process a log line: update broadcast tracker and print with summary
fn process_and_print_line<W: std::io::Write>(
    log_line: &LogLine,
    state: &mut NetworkState,
    writer: &mut W,
) -> std::io::Result<()> {
    let line = &log_line.line;

    // Early exit: skip trace lines when trace mode is disabled
    let is_trace_line = line.contains("msg_trace: ");
    if is_trace_line && !state.trace {
        return Ok(());
    }

    // Set first timestamp if not yet set (rounded down to nearest second)
    if let Some(current_time) = log_line.timestamp.as_ref() {
        if state.first_timestamp.is_none() {
            // Round down to the nearest second
            let timestamp_secs = current_time.timestamp();
            let rounded_time = DateTime::from_timestamp(timestamp_secs, 0).unwrap_or(*current_time);
            state.first_timestamp = Some(rounded_time);
        }
    }

    // Check if timestamp moved ahead by 100ms or more, and print dashes
    if let Some(current_time) = log_line.timestamp.as_ref() {
        if let Some(last_time) = &state.last_timestamp {
            let current_slot = current_time.timestamp_millis() / 100;
            let prev_slot = last_time.timestamp_millis() / 100;

            if current_slot != prev_slot && !state.quiet {
                writeln!(
                    writer,
                    "------------------------------------------------------------"
                )?;
            }
        }
        state.last_timestamp = Some(*current_time);
    }

    // Register node in wire tracker for every line (discovers all nodes)
    if state.trace {
        if let Some(node) = parse_validator_name(&log_line.source) {
            if let Some(ref mut tracker) = state.wire_tracker {
                tracker.note_node(&node);
            }
        }
    }

    // Process wire-level trace events
    if is_trace_line {
        if let Some(event) = parse_trace_line(line) {
            if let Some(node) = parse_validator_name(&log_line.source) {
                let first_ts = state.first_timestamp;
                if let Some(ref mut tracker) = state.wire_tracker {
                    let timestamp = log_line.timestamp.unwrap_or_else(Utc::now);
                    if state.quiet {
                        record_trace_event(&event, &node, timestamp, tracker);
                    } else {
                        format_trace_event(
                            &event,
                            &node,
                            timestamp,
                            tracker,
                            writer,
                            first_ts.as_ref(),
                        )?;
                    }
                    // Check if we should dump periodic wire stats
                    if tracker.should_dump(&timestamp) {
                        let wire_output = tracker.dump_and_reset(timestamp);
                        if !wire_output.is_empty() {
                            writeln!(writer, "{}", wire_output)?;
                        }
                    }
                }
            }
        }
        return Ok(());
    }

    // Check for WARN and ERROR log levels
    if line.contains(" WARN ") {
        state.histogram.record_warn();
    } else if line.contains(" ERROR ") {
        state.histogram.record_error();
    }

    // Check if we should dump the histogram
    if let Some(current_time) = log_line.timestamp.as_ref() {
        if state.histogram.should_dump(current_time) {
            let histogram_output = state.histogram.dump_and_reset(*current_time);
            if !histogram_output.is_empty() {
                writeln!(writer, "{}", histogram_output)?;
            }
        }
    }

    // Check if we should dump wire trace stats
    if let Some(current_time) = log_line.timestamp.as_ref() {
        if let Some(ref mut tracker) = state.wire_tracker {
            if tracker.should_dump(current_time) {
                let wire_output = tracker.dump_and_reset(*current_time);
                if !wire_output.is_empty() {
                    writeln!(writer, "{}", wire_output)?;
                }
            }
        }
    }

    // Print the actual line (skip in quiet mode)
    if !state.quiet {
        // Calculate time offset from first timestamp in milliseconds
        let time_prefix = if let (Some(current_time), Some(first_time)) =
            (log_line.timestamp.as_ref(), state.first_timestamp.as_ref())
        {
            let offset_ms =
                (current_time.timestamp_millis() - first_time.timestamp_millis()) as f64 / 1000.0;
            format!("[{}]({:.3}s)", log_line.source, offset_ms)
        } else {
            format!("[{}]", log_line.source)
        };

        writeln!(writer, "{} {}", time_prefix, line)?;
    }

    // Update tracker based on line type and show unreceived for this node
    // Only "after" lines (a [...]) have output messages and we track those
    if let Some(validator) = parse_validator_name(&log_line.source) {
        // For "before" lines, first update what this node has received (from in= field)
        // so that unreceived check is accurate
        if is_before_line(line) {
            let input_msgs = parse_input_messages(line);
            for msg_key in input_msgs {
                if let Some(msg_info) = state.broadcast_tracker.get_mut(&msg_key) {
                    // Record receipt time and update histogram
                    let receive_time = log_line.timestamp.unwrap_or_else(Utc::now);
                    msg_info
                        .receivers_by_node
                        .insert(validator.clone(), receive_time);

                    // Calculate delay from earliest sender to this receipt
                    if let Some(earliest_send_time) = msg_info.senders.values().min() {
                        let delay = receive_time
                            .signed_duration_since(*earliest_send_time)
                            .num_milliseconds()
                            .max(0) as u64;
                        state.histogram.record(delay);
                    }
                }
            }

            // Now print unreceived messages AFTER updating received state
            let unreceived_msgs = get_unreceived_messages_for_node(
                &state.broadcast_tracker,
                &validator,
                log_line.timestamp.as_ref(),
                state.max_message_delay_ms,
                &state.reported_unreceived,
            );

            if !unreceived_msgs.is_empty() {
                // Record the count of unreceived messages in histogram
                state.histogram.never_received += unreceived_msgs.len() as u64;

                // Mark these messages as reported
                for msg_key in &unreceived_msgs {
                    state
                        .reported_unreceived
                        .insert((validator.clone(), msg_key.clone()));
                }

                // Format and print the unreceived messages
                let unreceived_str = format_unreceived_messages(&unreceived_msgs);
                writeln!(
                    writer,
                    "[{}] WARN message(s) not received {}",
                    log_line.source, unreceived_str
                )?;
            }
        }

        if is_after_line(line) {
            // Check for output messages - these are newly broadcast messages
            let output_msgs = parse_output_messages(line);
            let send_time = log_line.timestamp.unwrap_or_else(Utc::now);

            for msg_key in output_msgs {
                // A new message is broadcast - add this validator as a sender with timestamp
                state
                    .broadcast_tracker
                    .entry(msg_key)
                    .or_insert_with(|| MessageInfo {
                        senders: HashMap::new(),
                        receivers_by_node: HashMap::new(),
                        timestamp: send_time,
                        first_sender: validator.clone(),
                    })
                    .senders
                    .insert(validator.clone(), send_time);
            }

            // Check for input messages - these show which messages this node has received
            let input_msgs = parse_input_messages(line);
            for msg_key in input_msgs {
                // Mark this validator as having received this message
                if let Some(msg_info) = state.broadcast_tracker.get_mut(&msg_key) {
                    let receive_time = log_line.timestamp.unwrap_or_else(Utc::now);
                    msg_info
                        .receivers_by_node
                        .insert(validator.clone(), receive_time);

                    // Calculate delay from earliest sender to this receipt
                    if let Some(earliest_send_time) = msg_info.senders.values().min() {
                        let delay = receive_time
                            .signed_duration_since(*earliest_send_time)
                            .num_milliseconds()
                            .max(0) as u64;
                        state.histogram.record(delay);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Get unreceived messages for a specific node
/// Returns a vector of MessageKeys that haven't been received yet
/// Only shows messages that this specific node has NOT received yet
/// Excludes messages sent by this node itself (counts other senders only)
/// Only includes messages older than max_message_delay_ms
/// Excludes messages that have already been reported for this node
fn get_unreceived_messages_for_node(
    broadcast_tracker: &HashMap<MessageKey, MessageInfo>,
    node: &str,
    current_timestamp: Option<&DateTime<Utc>>,
    max_message_delay_ms: u64,
    already_reported: &std::collections::HashSet<(String, MessageKey)>,
) -> Vec<MessageKey> {
    let delay_threshold = Duration::from_millis(max_message_delay_ms);
    let mut unreceived = Vec::new();

    for (msg_key, msg_info) in broadcast_tracker {
        // Skip if already reported for this node
        if already_reported.contains(&(node.to_string(), msg_key.clone())) {
            continue;
        }

        // Only include messages that this specific node hasn't received
        if !msg_info.receivers_by_node.contains_key(node) {
            // Check if message is old enough
            if let Some(current_time) = current_timestamp {
                let message_age = current_time
                    .signed_duration_since(msg_info.timestamp)
                    .to_std()
                    .unwrap_or(Duration::ZERO);

                // Skip messages that are too recent
                if message_age < delay_threshold {
                    continue;
                }
            }

            // Count senders excluding this node itself
            let other_sender_count = msg_info
                .senders
                .keys()
                .filter(|sender_name| sender_name.as_str() != node)
                .count();

            // Only include if there are other senders
            if other_sender_count > 0 {
                unreceived.push(msg_key.clone());
            }
        }
    }

    unreceived
}

/// Format unreceived messages in "h.r:kkkk" format
/// Shows cardinality by repeating the kind character for each sender
fn format_unreceived_messages(messages: &[MessageKey]) -> String {
    // Group by height.round
    let mut grouped: HashMap<(u64, u64), Vec<char>> = HashMap::new();

    for msg_key in messages {
        grouped
            .entry((msg_key.height, msg_key.round))
            .or_default()
            .push(msg_key.kind);
    }

    // Format as "h.r:kkkk" strings, sorted by height then round
    let mut entries: Vec<((u64, u64), Vec<char>)> = grouped.into_iter().collect();
    entries.sort_by_key(|(hr, _)| *hr);

    entries
        .into_iter()
        .map(|((h, r), mut kinds)| {
            kinds.sort();
            format!("{}.{}:{}", h, r, kinds.iter().collect::<String>())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse log lines from logjam output format with [source] prefix
    /// Skips "unreceived" lines as those are output, not input
    fn parse_loglines(input: &str) -> Vec<LogLine> {
        let mut log_lines = Vec::new();

        for line in input.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Skip unreceived lines - these are expected output, not input
            if line.contains("unreceived") {
                continue;
            }

            // Extract source from [node0.log] prefix
            let source = if line.starts_with('[') {
                if let Some(end) = line.find(']') {
                    line[1..end].to_string()
                } else {
                    continue;
                }
            } else {
                continue;
            };

            // Extract the actual log content after the [source] prefix
            let log_content = if let Some(pos) = line.find(']') {
                line[pos + 1..].trim()
            } else {
                line
            };

            // Parse timestamp from the log content
            let timestamp = parse_timestamp(log_content);

            log_lines.push(LogLine {
                source,
                timestamp,
                line: log_content.to_string(),
            });
        }

        log_lines
    }

    #[test]
    fn test_parse_loglines() {
        let input = concat!(
            "[node0.log] 2026-01-27T11:08:09.021548Z  INFO val0: a ",
            "[val0 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        random\n",
            "        [node3.log] 2026-01-27T11:08:09.021855Z  INFO val3: b ",
            "[val3 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        text!\n",
            "        [node2.log] 2026-01-27T11:08:09.021872Z  INFO val2: b ",
            "[val2 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node1.log] 2026-01-27T11:08:09.021987Z  INFO val1: b ",
            "[val1 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node3.log] 2026-01-27T11:08:09.022000Z  INFO val3: a ",
            "[val3 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node2.log] 2026-01-27T11:08:09.022036Z  INFO val2: a ",
            "[val2 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node1.log] 2026-01-27T11:08:09.022210Z  INFO val1: a ",
            "[val1 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node0.log] 2026-01-27T11:08:09.031822Z  INFO val0: b ",
            "[val0 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Pppppcccc]\n",
        );

        let log_lines = parse_loglines(input);

        // Verify we parsed all 8 lines
        assert_eq!(log_lines.len(), 8);

        // Check first line
        assert_eq!(log_lines[0].source, "node0.log");
        assert!(log_lines[0].timestamp.is_some());
        assert!(log_lines[0].line.contains("val0: a"));

        // Check last line
        assert_eq!(log_lines[7].source, "node0.log");
        assert!(log_lines[7].timestamp.is_some());
        assert!(log_lines[7].line.contains("2.0:Pppppcccc"));

        // Verify timestamps are in order in the input (they should be mostly ascending)
        assert!(log_lines[0].timestamp.unwrap() < log_lines[7].timestamp.unwrap());
    }

    #[test]
    fn test_unreceived1() {
        let input = concat!(
            "[node0.log] 2026-01-27T11:08:09.021548Z  INFO val0: a ",
            "[val0 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node3.log] 2026-01-27T11:08:09.021855Z  INFO val3: b ",
            "[val3 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node2.log] 2026-01-27T11:08:09.021872Z  INFO val2: b ",
            "[val2 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node1.log] 2026-01-27T11:08:09.021987Z  INFO val1: b ",
            "[val1 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node3.log] 2026-01-27T11:08:09.022000Z  INFO val3: a ",
            "[val3 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node2.log] 2026-01-27T11:08:09.022036Z  INFO val2: a ",
            "[val2 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node1.log] 2026-01-27T11:08:09.022210Z  INFO val1: a ",
            "[val1 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node0.log] 2026-01-27T11:08:09.031822Z  INFO val0: b ",
            "[val0 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Pppppcccc]\n",
        );

        let expected_output = concat!(
            "[node0.log](0.021s) 2026-01-27T11:08:09.021548Z  INFO val0: a ",
            "[val0 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node3.log](0.021s) 2026-01-27T11:08:09.021855Z  INFO val3: b ",
            "[val3 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node3.log] WARN message(s) not received 2.0:c\n",
            "        [node2.log](0.021s) 2026-01-27T11:08:09.021872Z  INFO val2: b ",
            "[val2 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node2.log] WARN message(s) not received 2.0:c\n",
            "        [node1.log](0.021s) 2026-01-27T11:08:09.021987Z  INFO val1: b ",
            "[val1 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=001 in=2.0:Ppppp]\n",
            "        [node1.log] WARN message(s) not received 2.0:c\n",
            "        [node3.log](0.022s) 2026-01-27T11:08:09.022000Z  INFO val3: a ",
            "[val3 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node2.log](0.022s) 2026-01-27T11:08:09.022036Z  INFO val2: a ",
            "[val2 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node1.log](0.022s) 2026-01-27T11:08:09.022210Z  INFO val1: a ",
            "[val1 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Ppppp] o=2.0:c\n",
            "        [node0.log](0.031s) 2026-01-27T11:08:09.031822Z  INFO val0: b ",
            "[val0 h=2 bt=0 rt=-2 chain=1/1769512088 r=0 prva=111 in=2.0:Pppppcccc]\n",
        );

        let log_lines = parse_loglines(input);

        // Process log lines and track unreceived messages
        // 0ms delay for test, quiet = false, trace = false
        let mut state = NetworkState::new(0, 100, false, false);
        let mut output = Vec::new();

        for log_line in &log_lines {
            process_and_print_line(log_line, &mut state, &mut output)
                .expect("Writing to Vec should not fail");
        }

        // Convert output to string
        let output_str = String::from_utf8(output).expect("Output should be valid UTF-8");

        // Normalize the expected output for comparison (remove leading/trailing whitespace from
        // each line)
        let expected_lines: Vec<String> = expected_output
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();

        let output_lines: Vec<String> = output_str
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();

        // Compare line by line
        assert_eq!(
            output_lines.len(),
            expected_lines.len(),
            "Output should have same number of lines as input.\nExpected:\n{}\n\nGot:\n{}",
            expected_lines.join("\n"),
            output_lines.join("\n")
        );

        for (i, (expected, actual)) in expected_lines.iter().zip(output_lines.iter()).enumerate() {
            assert_eq!(
                actual,
                expected,
                "Line {} mismatch:\nExpected: {}\nActual:   {}",
                i + 1,
                expected,
                actual
            );
        }
    }

    #[test]
    fn test_parse_trace_lines() {
        // SEND
        let line = "2026-01-27T11:08:09.021Z DEBUG msg_trace: SEND h=20 r=0 t=c to=abc12345";
        let event = parse_trace_line(line).expect("should parse SEND");
        match event {
            TraceEvent::Send { h, r, t, to_peer } => {
                assert_eq!(h, 20);
                assert_eq!(r, 0);
                assert_eq!(t, 'c');
                assert_eq!(to_peer, "abc12345");
            }
            _ => panic!("expected Send"),
        }

        // RECV
        let line = "2026-01-27T11:08:09.023Z DEBUG msg_trace: RECV h=20 r=0 t=P from=def67890";
        let event = parse_trace_line(line).expect("should parse RECV");
        match event {
            TraceEvent::Recv { h, r, t, from_peer } => {
                assert_eq!(h, 20);
                assert_eq!(r, 0);
                assert_eq!(t, 'P');
                assert_eq!(from_peer, "def67890");
            }
            _ => panic!("expected Recv"),
        }

        // RECV_DUP
        let line = "2026-01-27T11:08:09.025Z DEBUG msg_trace: RECV_DUP h=20 r=0 t=p from=aaa11111";
        let event = parse_trace_line(line).expect("should parse RECV_DUP");
        match event {
            TraceEvent::RecvDup { h, r, t, from_peer } => {
                assert_eq!(h, 20);
                assert_eq!(r, 0);
                assert_eq!(t, 'p');
                assert_eq!(from_peer, "aaa11111");
            }
            _ => panic!("expected RecvDup"),
        }

        // DRAIN
        let line = "2026-01-27T11:08:09.030Z DEBUG msg_trace: DRAIN count=5 h_range=18..20 \
                    triggered_by=bbb22222";
        let event = parse_trace_line(line).expect("should parse DRAIN");
        match event {
            TraceEvent::Drain {
                count,
                h_range,
                triggered_by,
            } => {
                assert_eq!(count, 5);
                assert_eq!(h_range, "18..20");
                assert_eq!(triggered_by, "bbb22222");
            }
            _ => panic!("expected Drain"),
        }

        // RELAY
        let line = "2026-01-27T11:08:09.035Z DEBUG msg_trace: RELAY h=20 r=0 t=c from=ccc33333 \
                    to_peers=3 failed=1";
        let event = parse_trace_line(line).expect("should parse RELAY");
        match event {
            TraceEvent::Relay {
                h,
                r,
                t,
                from_peer,
                to_peers,
                failed,
            } => {
                assert_eq!(h, 20);
                assert_eq!(r, 0);
                assert_eq!(t, 'c');
                assert_eq!(from_peer, "ccc33333");
                assert_eq!(to_peers, 3);
                assert_eq!(failed, 1);
            }
            _ => panic!("expected Relay"),
        }

        // Non-trace line should return None
        assert!(parse_trace_line("2026-01-27T11:08:09.021Z INFO val0: a [...]").is_none());
    }

    #[test]
    fn test_wire_tracker() {
        let mut tracker = WireTracker::new();

        tracker.note_node("node0");
        tracker.note_node("node1");
        tracker.note_node("node2");
        tracker.note_node("node3");

        let t0 = DateTime::parse_from_rfc3339("2026-01-27T11:08:09.000Z")
            .unwrap()
            .with_timezone(&Utc);
        let t1 = DateTime::parse_from_rfc3339("2026-01-27T11:08:09.002Z")
            .unwrap()
            .with_timezone(&Utc);
        let t2 = DateTime::parse_from_rfc3339("2026-01-27T11:08:09.003Z")
            .unwrap()
            .with_timezone(&Utc);

        let key = MessageKey {
            height: 20,
            round: 0,
            kind: 'c',
        };

        // node0 sends
        tracker.record_send(key.clone(), "node0", t0);
        assert_eq!(tracker.send_count, 1);

        // node1 receives (2ms latency)
        let latency = tracker.record_recv(key.clone(), "node1", t1);
        assert_eq!(latency, Some(2));
        assert_eq!(tracker.recv_count, 1);

        // node2 receives (3ms latency)
        let latency = tracker.record_recv(key.clone(), "node2", t2);
        assert_eq!(latency, Some(3));
        assert_eq!(tracker.recv_count, 2);

        // Verify period counters
        assert_eq!(tracker.send_count, 1);
        assert_eq!(tracker.recv_count, 2);

        // Verify lifetime counters
        assert_eq!(tracker.lifetime_sends, 1);
        assert_eq!(tracker.lifetime_recvs, 2);
        assert_eq!(tracker.lifetime_latency_count, 2);
        assert_eq!(tracker.lifetime_latency_max, 3);

        // node3 never received - check summary mentions it
        let summary = tracker.summary();
        assert!(summary.contains("SEND=1"), "summary: {}", summary);
        assert!(summary.contains("RECV=2"), "summary: {}", summary);
        assert!(summary.contains("node3"), "summary: {}", summary);
        assert!(
            summary.contains("incomplete wire delivery"),
            "summary: {}",
            summary
        );

        // Verify periodic dump resets period counters but not lifetime
        let dump = tracker.dump_and_reset(Utc::now());
        assert!(dump.contains("SEND=1"), "dump: {}", dump);
        assert!(
            dump.contains("missing_delivery=1"),
            "dump should show node3 as missing: {}",
            dump
        );
        assert!(
            dump.contains("Lifetime: SEND=1 RECV=2"),
            "dump should include lifetime totals: {}",
            dump
        );
        assert_eq!(tracker.send_count, 0);
        assert_eq!(tracker.recv_count, 0);
        assert_eq!(tracker.lifetime_sends, 1); // lifetime unchanged
        assert_eq!(tracker.lifetime_recvs, 2);
    }

    #[test]
    fn test_trace_lines_skipped_without_trace_flag() {
        let trace_line = LogLine {
            source: "node0.log".to_string(),
            timestamp: Some(
                DateTime::parse_from_rfc3339("2026-01-27T11:08:09.021Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            line: "2026-01-27T11:08:09.021Z DEBUG msg_trace: SEND h=20 r=0 t=c to=abc12345"
                .to_string(),
        };

        // With trace=false, trace lines should be silently skipped
        let mut state = NetworkState::new(1000, 100, false, false);
        let mut output = Vec::new();
        process_and_print_line(&trace_line, &mut state, &mut output).expect("should not fail");
        assert!(
            output.is_empty(),
            "trace lines should be skipped when trace=false"
        );

        // With trace=true, trace lines should produce output
        let mut state = NetworkState::new(1000, 100, false, true);
        let mut output = Vec::new();
        process_and_print_line(&trace_line, &mut state, &mut output).expect("should not fail");
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("[TRACE]"),
            "trace lines should produce output when trace=true"
        );
        assert!(output_str.contains("SEND"));
        assert!(output_str.contains("20.0:c"));
    }
}
