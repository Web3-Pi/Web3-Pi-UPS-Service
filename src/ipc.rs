//! Unix-socket IPC server. Read-only state queries for the CLI and (later) the
//! LCD plugin.
//!
//! Wire format: line-delimited JSON. One JSON object per line; client closes
//! the socket to disconnect. Ops:
//!   - `{"op":"snapshot"}`  → one `snapshot` reply, then connection stays open
//!   - `{"op":"subscribe"}` → `snapshot` reply, then a `snapshot` every second until disconnect
//!   - `{"op":"version"}`   → `{"type":"version","version":"<x.y.z>"}` then connection stays open

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::proto::payloads::{HostStatusV1, NetStatusV1, PowerStatusV1};
use crate::soc::pack_mv_to_soc_pct;
use crate::state::{AgentState, State};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Snapshot,
    Subscribe,
    Version,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    Snapshot(SnapshotMsg),
    Version { version: &'static str },
    Error { message: String },
}

#[derive(Debug, Serialize)]
struct SnapshotMsg {
    /// Unix timestamp of when this snapshot was generated.
    unix_ts_ms: u64,
    power: Option<PowerSnapshot>,
    net: Option<NetSnapshot>,
    host: Option<HostSnapshot>,
    last_power_event: Option<u8>,
    shutdown_pending_for_s: Option<u64>,
}

#[derive(Debug, Serialize)]
struct PowerSnapshot {
    age_ms: Option<u64>,
    charge_state: u8,
    vbus_in_mv: u16,
    vbus_out_mv: u16,
    ibus_out_ma: i16,
    vbat_mv: u16,
    ibat_ma: i16,
    soc_pct: u8,
    on_battery: bool,
    temp_dc: i16,
    pd_contract_mv: u16,
    pd_contract_ma: u16,
    faults: u16,
}

#[derive(Debug, Serialize)]
struct NetSnapshot {
    age_ms: Option<u64>,
    state: u8,
    rssi_dbm: i8,
    rsrp_dbm: i8,
    rsrq_db: i8,
    bytes_tx: u32,
    bytes_rx: u32,
}

#[derive(Debug, Serialize)]
struct HostSnapshot {
    age_ms: Option<u64>,
    cpu_temp_dc: i16,
    cpu_usage_pct: Option<u8>,
    load_avg_x100: u16,
    mem_used_pct: u8,
    disk_used_pct: u8,
    uptime_s: u32,
    /// Cumulative bytes received across non-loopback interfaces, since boot.
    net_bytes_rx_total: Option<u64>,
    net_bytes_tx_total: Option<u64>,
    net_rx_bytes_per_s: Option<u64>,
    net_tx_bytes_per_s: Option<u64>,
    eth_client_state: u8,
}

/// Spawn the IPC listener on `socket_path`. Returns the listener task handle.
pub async fn spawn_ipc(
    socket_path: String,
    state: Arc<State>,
    on_batt_min_mv: u16,
    on_batt_max_mv: u16,
) -> Result<tokio::task::JoinHandle<()>> {
    if let Some(parent) = Path::new(&socket_path).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create IPC dir {}", parent.display()))?;
        }
    }
    // Best-effort cleanup of a stale socket from a previous run.
    let _ = tokio::fs::remove_file(&socket_path).await;

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind IPC socket {socket_path}"))?;
    info!("IPC listening on {socket_path}");

    let handle = tokio::spawn(accept_loop(
        listener,
        state,
        OnBattCfg {
            min_mv: on_batt_min_mv,
            max_mv: on_batt_max_mv,
        },
    ));
    Ok(handle)
}

#[derive(Clone, Copy)]
struct OnBattCfg {
    min_mv: u16,
    max_mv: u16,
}

async fn accept_loop(listener: UnixListener, state: Arc<State>, cfg: OnBattCfg) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let state = state.clone();
                tokio::spawn(handle_client(stream, state, cfg));
            }
            Err(e) => {
                warn!("IPC accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn handle_client(stream: UnixStream, state: Arc<State>, cfg: OnBattCfg) {
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd).lines();
    let (tick_tx, mut tick_rx) = mpsc::channel::<()>(4);
    let mut subscribed = false;
    let mut ticker_handle: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        tokio::select! {
            line = reader.next_line() => match line {
                Ok(Some(line)) => {
                    let req: Result<Request, _> = serde_json::from_str(line.trim());
                    match req {
                        Ok(Request::Snapshot) => {
                            send_snapshot(&mut wr, &state, &cfg).await;
                        }
                        Ok(Request::Subscribe) => {
                            send_snapshot(&mut wr, &state, &cfg).await;
                            if !subscribed {
                                subscribed = true;
                                let tx = tick_tx.clone();
                                ticker_handle = Some(tokio::spawn(async move {
                                    let mut interval = tokio::time::interval(Duration::from_secs(1));
                                    loop {
                                        interval.tick().await;
                                        if tx.send(()).await.is_err() { break; }
                                    }
                                }));
                            }
                        }
                        Ok(Request::Version) => {
                            send_reply(&mut wr, &Reply::Version { version: VERSION }).await;
                        }
                        Err(e) => {
                            send_reply(&mut wr, &Reply::Error { message: format!("bad request: {e}") }).await;
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    debug!("IPC client read error: {e}");
                    break;
                }
            },
            tick = tick_rx.recv() => {
                if tick.is_none() { break; }
                send_snapshot(&mut wr, &state, &cfg).await;
            }
        }
    }
    if let Some(h) = ticker_handle {
        h.abort();
    }
    debug!("IPC client disconnected");
}

async fn send_snapshot(wr: &mut tokio::net::unix::OwnedWriteHalf, state: &State, cfg: &OnBattCfg) {
    let snap = state.snapshot().await;
    let msg = build_snapshot(&snap, cfg);
    send_reply(wr, &Reply::Snapshot(msg)).await;
}

async fn send_reply(wr: &mut tokio::net::unix::OwnedWriteHalf, reply: &Reply) {
    let line = match serde_json::to_string(reply) {
        Ok(s) => s,
        Err(e) => {
            warn!("serialize IPC reply: {e}");
            return;
        }
    };
    if let Err(e) = wr.write_all(line.as_bytes()).await {
        debug!("IPC write failed: {e}");
        return;
    }
    let _ = wr.write_all(b"\n").await;
    let _ = wr.flush().await;
}

fn build_snapshot(snap: &AgentState, cfg: &OnBattCfg) -> SnapshotMsg {
    let now = Instant::now();
    let unix_ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let power = snap
        .last_power
        .map(|p| make_power(p, snap.last_power_at, now, cfg));
    let net = snap.last_net.map(|n| make_net(n, snap.last_net_at, now));
    let host = snap.last_host.map(|h| make_host(h, snap, now));

    SnapshotMsg {
        unix_ts_ms,
        power,
        net,
        host,
        last_power_event: snap.last_power_event,
        shutdown_pending_for_s: snap.shutdown_pending_since.map(|t| t.elapsed().as_secs()),
    }
}

fn make_power(
    p: PowerStatusV1,
    at: Option<Instant>,
    now: Instant,
    cfg: &OnBattCfg,
) -> PowerSnapshot {
    let soc_pct = pack_mv_to_soc_pct(p.vbat_mv);
    let on_battery = crate::shutdown_sm::is_on_battery(p.vbus_in_mv, cfg.min_mv, cfg.max_mv);
    PowerSnapshot {
        age_ms: at.map(|t| now.saturating_duration_since(t).as_millis() as u64),
        charge_state: p.charge_state,
        vbus_in_mv: p.vbus_in_mv,
        vbus_out_mv: p.vbus_out_mv,
        ibus_out_ma: p.ibus_out_ma,
        vbat_mv: p.vbat_mv,
        ibat_ma: p.ibat_ma,
        soc_pct,
        on_battery,
        temp_dc: p.temp_dc,
        pd_contract_mv: p.pd_contract_mv,
        pd_contract_ma: p.pd_contract_ma,
        faults: p.faults,
    }
}

fn make_host(h: HostStatusV1, snap: &AgentState, now: Instant) -> HostSnapshot {
    HostSnapshot {
        age_ms: snap
            .last_host_at
            .map(|t| now.saturating_duration_since(t).as_millis() as u64),
        cpu_temp_dc: h.cpu_temp_dc,
        cpu_usage_pct: snap.cpu_usage_pct,
        load_avg_x100: h.load_avg_x100,
        mem_used_pct: h.mem_used_pct,
        disk_used_pct: h.disk_used_pct,
        uptime_s: h.uptime_s,
        net_bytes_rx_total: snap.net_totals.map(|n| n.bytes_rx),
        net_bytes_tx_total: snap.net_totals.map(|n| n.bytes_tx),
        net_rx_bytes_per_s: snap.net_rx_bytes_per_s,
        net_tx_bytes_per_s: snap.net_tx_bytes_per_s,
        eth_client_state: h.eth_client_state,
    }
}

fn make_net(n: NetStatusV1, at: Option<Instant>, now: Instant) -> NetSnapshot {
    NetSnapshot {
        age_ms: at.map(|t| now.saturating_duration_since(t).as_millis() as u64),
        state: n.state,
        rssi_dbm: n.rssi_dbm,
        rsrp_dbm: n.rsrp_dbm,
        rsrq_db: n.rsrq_db,
        bytes_tx: n.bytes_tx,
        bytes_rx: n.bytes_rx,
    }
}
