//! `w3p-ups status` and `w3p-ups watch` — connect to the daemon's IPC socket
//! and print human-readable snapshots.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::config::IpcConfig;

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Snapshot,
    Subscribe,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    Snapshot(SnapshotMsg),
    Version { version: String },
    Error { message: String },
}

#[derive(Deserialize, Debug)]
struct SnapshotMsg {
    unix_ts_ms: u64,
    power: Option<PowerSnap>,
    net: Option<NetSnap>,
    host: Option<HostSnap>,
    last_power_event: Option<u8>,
    shutdown_pending_for_s: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct PowerSnap {
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

#[derive(Deserialize, Debug)]
struct NetSnap {
    age_ms: Option<u64>,
    state: u8,
    rssi_dbm: i8,
    rsrp_dbm: i8,
    rsrq_db: i8,
    bytes_tx: u32,
    bytes_rx: u32,
}

#[derive(Deserialize, Debug)]
struct HostSnap {
    age_ms: Option<u64>,
    cpu_temp_dc: i16,
    cpu_usage_pct: Option<u8>,
    load_avg_x100: u16,
    mem_used_pct: u8,
    disk_used_pct: u8,
    uptime_s: u32,
    net_bytes_rx_total: Option<u64>,
    net_bytes_tx_total: Option<u64>,
    net_rx_bytes_per_s: Option<u64>,
    net_tx_bytes_per_s: Option<u64>,
    eth_client_state: u8,
}

pub async fn run_status(ipc: &IpcConfig) -> Result<()> {
    let mut stream = connect(ipc).await?;
    write_request(&mut stream, &Request::Snapshot).await?;
    let (rd, _wr) = stream.split();
    let mut lines = BufReader::new(rd).lines();
    if let Some(line) = lines.next_line().await? {
        print_reply(&line)?;
    }
    Ok(())
}

pub async fn run_watch(ipc: &IpcConfig) -> Result<()> {
    let mut stream = connect(ipc).await?;
    write_request(&mut stream, &Request::Subscribe).await?;
    let (rd, _wr) = stream.split();
    let mut lines = BufReader::new(rd).lines();
    loop {
        tokio::select! {
            res = lines.next_line() => match res? {
                Some(line) => print_reply(&line)?,
                None => break,
            },
            _ = tokio::signal::ctrl_c() => {
                println!();
                break;
            }
        }
    }
    Ok(())
}

async fn connect(ipc: &IpcConfig) -> Result<UnixStream> {
    UnixStream::connect(&ipc.socket_path)
        .await
        .with_context(|| {
            format!(
                "connect IPC socket {} (is the daemon running?)",
                ipc.socket_path
            )
        })
}

async fn write_request(stream: &mut UnixStream, req: &Request) -> Result<()> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn print_reply(line: &str) -> Result<()> {
    let reply: Reply =
        serde_json::from_str(line).with_context(|| format!("parse IPC reply: {line}"))?;
    match reply {
        Reply::Snapshot(s) => print_snapshot(&s),
        Reply::Version { version } => println!("daemon version: {version}"),
        Reply::Error { message } => eprintln!("daemon error: {message}"),
    }
    Ok(())
}

fn print_snapshot(s: &SnapshotMsg) {
    let ts_secs = s.unix_ts_ms / 1000;
    print!("[{ts_secs}] ");
    if let Some(p) = &s.power {
        let age = p
            .age_ms
            .map(|m| format!("{}ms", m))
            .unwrap_or_else(|| "n/a".into());
        let src = if p.on_battery { "BATTERY" } else { "GRID" };
        let charge = charge_state_name(p.charge_state);
        let temp_c = p.temp_dc as f32 / 10.0;
        print!(
            "power[age={age}] SOC={soc}% src={src} charge={charge} VI={vi}mV VOUT={vo}mV IOUT={io}mA VBAT={vb}mV IBAT={ib}mA T={temp:.1}°C PD={pd_v}mV/{pd_a}mA faults=0x{f:04x}",
            soc = p.soc_pct,
            vi = p.vbus_in_mv,
            vo = p.vbus_out_mv,
            io = p.ibus_out_ma,
            vb = p.vbat_mv,
            ib = p.ibat_ma,
            temp = temp_c,
            pd_v = p.pd_contract_mv,
            pd_a = p.pd_contract_ma,
            f = p.faults,
        );
    } else {
        print!("power=<no data yet>");
    }
    if let Some(ev) = s.last_power_event {
        print!(" last_event={}", power_event_name(ev));
    }
    if let Some(secs) = s.shutdown_pending_for_s {
        print!(" SHUTDOWN_PENDING_{}s", secs);
    }
    if let Some(n) = &s.net {
        let age = n
            .age_ms
            .map(|m| format!("{}ms", m))
            .unwrap_or_else(|| "n/a".into());
        print!(
            " | net[age={age}] state={st} rssi={rs}dBm rsrp={rp}dBm rsrq={rq}dB tx={tx}B rx={rx}B",
            st = net_state_name(n.state),
            rs = n.rssi_dbm,
            rp = n.rsrp_dbm,
            rq = n.rsrq_db,
            tx = n.bytes_tx,
            rx = n.bytes_rx,
        );
    }
    if let Some(h) = &s.host {
        let age = h
            .age_ms
            .map(|m| format!("{}ms", m))
            .unwrap_or_else(|| "n/a".into());
        let temp_c = h.cpu_temp_dc as f32 / 10.0;
        let load = h.load_avg_x100 as f32 / 100.0;
        let cpu_usage = h
            .cpu_usage_pct
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| "n/a".into());
        let net_rate = match (h.net_rx_bytes_per_s, h.net_tx_bytes_per_s) {
            (Some(r), Some(t)) => format!("{}/s↓ {}/s↑", fmt_bytes(r), fmt_bytes(t)),
            _ => "n/a".into(),
        };
        let net_total = match (h.net_bytes_rx_total, h.net_bytes_tx_total) {
            (Some(r), Some(t)) => format!("{}↓ {}↑", fmt_bytes(r), fmt_bytes(t)),
            _ => "n/a".into(),
        };
        let eth = eth_client_state_name(h.eth_client_state);
        print!(
            " | host[age={age}] T={temp:.1}°C cpu={cpu_usage} load={load:.2} mem={mem}% disk={disk}% up={up}s net={net_rate} (total={net_total}) eth={eth}",
            temp = temp_c,
            mem = h.mem_used_pct,
            disk = h.disk_used_pct,
            up = h.uptime_s,
        );
    }
    println!();
}

fn fmt_bytes(b: u64) -> String {
    const K: u64 = 1024;
    const M: u64 = K * 1024;
    const G: u64 = M * 1024;
    if b >= G {
        format!("{:.1}GB", b as f64 / G as f64)
    } else if b >= M {
        format!("{:.1}MB", b as f64 / M as f64)
    } else if b >= K {
        format!("{:.1}kB", b as f64 / K as f64)
    } else {
        format!("{}B", b)
    }
}

fn eth_client_state_name(s: u8) -> &'static str {
    match s {
        0 => "stopped",
        1 => "syncing",
        2 => "synced",
        3 => "error",
        _ => "?",
    }
}

fn charge_state_name(s: u8) -> &'static str {
    match s {
        0 => "idle",
        1 => "charging",
        2 => "charged",
        3 => "fault",
        _ => "?",
    }
}

fn net_state_name(s: u8) -> &'static str {
    match s {
        0 => "off",
        1 => "init",
        2 => "net_attach",
        3 => "ppp_up",
        4 => "mqtt_up",
        5 => "err",
        _ => "?",
    }
}

fn power_event_name(e: u8) -> &'static str {
    match e {
        1 => "MAINS_LOST",
        2 => "MAINS_RESTORED",
        3 => "CHARGE_LOW",
        4 => "CHARGE_FULL",
        5 => "FAULT",
        _ => "?",
    }
}
