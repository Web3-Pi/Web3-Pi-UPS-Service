use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::host_metrics::{HostMetricsSample, NetTotals};
use crate::proto::payloads::{HostStatusV1, NetStatusV1, PowerStatusV1, SysHelloV1};

/// Snapshot of the most recent telemetry observed from each peer.
#[derive(Debug, Default, Clone)]
pub struct AgentState {
    pub last_power: Option<PowerStatusV1>,
    pub last_power_at: Option<Instant>,
    pub last_power_event: Option<u8>,
    pub last_power_event_at: Option<Instant>,
    pub last_net: Option<NetStatusV1>,
    pub last_net_at: Option<Instant>,
    pub peers: HashMap<u8, SysHelloV1>,
    pub shutdown_pending_since: Option<Instant>,

    // Host metrics — populated by `host_metrics_loop`. Only `last_host` is
    // emitted on the wire as `host.status`; the rest is local-only (IPC).
    pub last_host: Option<HostStatusV1>,
    pub last_host_at: Option<Instant>,
    pub cpu_usage_pct: Option<u8>,
    pub net_totals: Option<NetTotals>,
    pub net_rx_bytes_per_s: Option<u64>,
    pub net_tx_bytes_per_s: Option<u64>,
}

/// Per-destination outbound sequence counter (matches "scoped per (SRC, DST)"
/// in the wire protocol spec).
#[derive(Debug, Default)]
pub struct TxSeq {
    next: HashMap<u8, u8>,
}

impl TxSeq {
    pub fn next_for(&mut self, dst: u8) -> u8 {
        let entry = self.next.entry(dst).or_insert(0);
        let v = *entry;
        *entry = entry.wrapping_add(1);
        v
    }
}

/// Shared, mutable agent state. Wrap in `Arc<...>` for tasks.
#[derive(Default)]
pub struct State {
    inner: RwLock<AgentState>,
    tx_seq: RwLock<TxSeq>,
}

impl State {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn update_power(&self, status: PowerStatusV1) {
        let mut s = self.inner.write().await;
        s.last_power = Some(status);
        s.last_power_at = Some(Instant::now());
    }

    pub async fn update_power_event(&self, event: u8) {
        let mut s = self.inner.write().await;
        s.last_power_event = Some(event);
        s.last_power_event_at = Some(Instant::now());
    }

    pub async fn update_net(&self, status: NetStatusV1) {
        let mut s = self.inner.write().await;
        s.last_net = Some(status);
        s.last_net_at = Some(Instant::now());
    }

    pub async fn record_hello(&self, src: u8, hello: SysHelloV1) {
        let mut s = self.inner.write().await;
        s.peers.insert(src, hello);
    }

    pub async fn update_host_sample(&self, sample: HostMetricsSample) {
        let mut s = self.inner.write().await;
        s.last_host = Some(sample.status);
        s.last_host_at = Some(Instant::now());
        s.cpu_usage_pct = sample.cpu_usage_pct;
        s.net_totals = sample.net;
        s.net_rx_bytes_per_s = sample.net_rx_bytes_per_s;
        s.net_tx_bytes_per_s = sample.net_tx_bytes_per_s;
    }

    pub async fn snapshot(&self) -> AgentState {
        self.inner.read().await.clone()
    }

    pub async fn set_shutdown_pending(&self, since: Option<Instant>) {
        self.inner.write().await.shutdown_pending_since = since;
    }

    pub async fn next_seq(&self, dst: u8) -> u8 {
        self.tx_seq.write().await.next_for(dst)
    }
}
