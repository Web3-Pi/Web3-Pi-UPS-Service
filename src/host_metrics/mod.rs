//! Linux host metrics: CPU (temp / load / usage), RAM, root disk, network,
//! uptime. Emits a `host.status` EVENT to RP2040 on a configurable cadence
//! (default 30 s, sized for the ~500 MB/mo LTE data plan; AGENT-2 in
//! implementation-plan).

mod cpu;
mod disk;
pub mod eth;
mod mem;
mod network;
mod uptime;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::config::{EthClientsConfig, HostMetricsConfig};
use crate::proto::payloads::HostStatusV1;
use crate::proto::{addr, class, flag, op, Frame};
use crate::state::State;
use crate::transport::OutboundFrame;

pub use network::NetTotals;

/// Aggregate metrics for one tick. Used both for the host.status emission
/// (only the wire-protocol fields go out) and for the IPC snapshot
/// (everything is exposed locally).
#[derive(Debug, Clone, Copy, Default)]
pub struct HostMetricsSample {
    pub status: HostStatusV1,
    pub cpu_usage_pct: Option<u8>,
    pub net: Option<NetTotals>,
    pub net_rx_bytes_per_s: Option<u64>,
    pub net_tx_bytes_per_s: Option<u64>,
}

pub async fn host_metrics_loop(
    state: Arc<State>,
    cfg: HostMetricsConfig,
    eth_cfg: EthClientsConfig,
    out_tx: mpsc::Sender<OutboundFrame>,
) {
    if cfg.interval_seconds == 0 {
        info!("host_metrics: disabled (interval_seconds = 0)");
        // Park the task so the supervisor doesn't immediately treat us as dead.
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
    info!(interval_s = cfg.interval_seconds, "host_metrics running");
    let mut tick = interval(Duration::from_secs(cfg.interval_seconds));
    let mut prev_cpu = cpu::read_cpu_snap();
    let mut prev_net = network::read_totals();

    loop {
        tick.tick().await;
        let curr_cpu = cpu::read_cpu_snap();
        let cpu_usage = match (&prev_cpu, &curr_cpu) {
            (Some(p), Some(c)) => cpu::compute_usage_pct(p, c),
            _ => None,
        };
        prev_cpu = curr_cpu;

        let curr_net = network::read_totals();
        let (rx_per_s, tx_per_s) = match (&prev_net, &curr_net) {
            (Some(p), Some(c)) => (
                Some(c.bytes_rx.saturating_sub(p.bytes_rx) / cfg.interval_seconds),
                Some(c.bytes_tx.saturating_sub(p.bytes_tx) / cfg.interval_seconds),
            ),
            _ => (None, None),
        };
        prev_net = curr_net;

        let load = cpu::read_load_avg_x100().unwrap_or(0);
        let temp = cpu::read_temp_dc().unwrap_or(0);
        let mem_pct = mem::read_used_pct().unwrap_or(0);
        let disk_pct = disk::read_used_pct("/").unwrap_or(0);
        let uptime_s = uptime::read_uptime_s().unwrap_or(0);

        // Per-client systemd service state (execution / consensus / validator)
        // packed into the single eth_client_state byte (2 bits each) — no wire
        // change, firmware keeps forwarding the same frame. See host_metrics::eth.
        let eth_client_state = eth::read_packed_state(&eth_cfg).await;

        let status = HostStatusV1 {
            eth_client_state,
            cpu_temp_dc: temp,
            mem_used_pct: mem_pct,
            disk_used_pct: disk_pct,
            load_avg_x100: load,
            uptime_s,
        };

        let sample = HostMetricsSample {
            status,
            cpu_usage_pct: cpu_usage,
            net: curr_net,
            net_rx_bytes_per_s: rx_per_s,
            net_tx_bytes_per_s: tx_per_s,
        };

        debug!(
            cpu_temp_dc = status.cpu_temp_dc,
            cpu_usage_pct = ?cpu_usage,
            mem_used_pct = status.mem_used_pct,
            disk_used_pct = status.disk_used_pct,
            load_avg_x100 = status.load_avg_x100,
            uptime_s = status.uptime_s,
            net_rx_per_s = ?rx_per_s,
            net_tx_per_s = ?tx_per_s,
            "host_metrics tick"
        );

        state.update_host_sample(sample).await;

        let frame = Frame {
            dst: addr::RP2040,
            src: addr::RPI,
            class: class::HOST,
            op: op::host::STATUS,
            flags: flag::EVENT,
            seq: state.next_seq(addr::RP2040).await,
            payload: status.encode().to_vec(),
        };
        if out_tx.send(OutboundFrame { frame }).await.is_err() {
            warn!("host_metrics: outbound closed; exiting");
            return;
        }
    }
}
