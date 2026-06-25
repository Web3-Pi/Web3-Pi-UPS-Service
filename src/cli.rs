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
    faults: u16,
    // pd_contract_mv / pd_contract_ma are present in the IPC JSON for
    // diagnostics but not surfaced in this CLI — values reported by CH32X
    // are currently misleading (track CH32X firmware fix).
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
        print_reply(&line, /* refresh = */ false)?;
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
                Some(line) => print_reply(&line, /* refresh = */ true)?,
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

fn print_reply(line: &str, refresh: bool) -> Result<()> {
    let reply: Reply =
        serde_json::from_str(line).with_context(|| format!("parse IPC reply: {line}"))?;
    match reply {
        Reply::Snapshot(s) => {
            if refresh {
                // Clear screen + cursor home — for `watch` mode so each
                // new snapshot replaces the previous block in place.
                print!("\x1b[2J\x1b[H");
            }
            print_snapshot(&s);
        }
        Reply::Version { version } => println!("daemon version: {version}"),
        Reply::Error { message } => eprintln!("daemon error: {message}"),
    }
    Ok(())
}

const LBL: usize = 11; // label column width

fn print_snapshot(s: &SnapshotMsg) {
    println!("Web3 Pi UPS — {}", format_clock_utc(s.unix_ts_ms));
    println!();

    print_power_block(s);
    if s.net.is_some() {
        println!();
        print_net_block(s);
    }
    println!();
    print_host_block(s);
}

fn print_power_block(s: &SnapshotMsg) {
    let header = match &s.power {
        Some(p) => {
            let age = p
                .age_ms
                .map(|m| format!("{}ms ago", m))
                .unwrap_or_else(|| "no data".into());
            format!("power  ({age})")
        }
        None => "power  (no data yet)".into(),
    };
    println!("{header}");

    let Some(p) = &s.power else { return };
    let src = if p.on_battery { "BATTERY" } else { "GRID" };
    let charge = charge_state_name(p.charge_state);
    let temp_c = p.temp_dc as f32 / 10.0;

    row("source", &format!("{src:<8}  charge: {charge}"));
    row(
        "input",
        &format!("VI   = {} V", fmt_mv(p.vbus_in_mv as i32)),
    );
    row(
        "output",
        &format!(
            "VOUT = {} V    IOUT = {} A",
            fmt_mv(p.vbus_out_mv as i32),
            fmt_ma(p.ibus_out_ma as i32),
        ),
    );
    row(
        "battery",
        &format!(
            "VBAT = {} V    IBAT = {} mA    SOC  = {}%",
            fmt_mv(p.vbat_mv as i32),
            p.ibat_ma,
            p.soc_pct,
        ),
    );
    row("thermal", &format!("T = {temp_c:.1} °C"));
    row("faults", &format!("0x{:04x}", p.faults));

    if let Some(ev) = s.last_power_event {
        row("last event", power_event_name(ev));
    }
    if let Some(secs) = s.shutdown_pending_for_s {
        row("ALERT", &format!("shutdown pending: {secs} s elapsed"));
    }
}

fn print_net_block(s: &SnapshotMsg) {
    let n = s.net.as_ref().unwrap();
    let age = n
        .age_ms
        .map(|m| format!("{}ms ago", m))
        .unwrap_or_else(|| "no data".into());
    println!("net    ({age})");

    row("state", net_state_name(n.state));
    row(
        "signal",
        &format!(
            "RSSI = {} dBm    RSRP = {} dBm    RSRQ = {} dB",
            n.rssi_dbm, n.rsrp_dbm, n.rsrq_db
        ),
    );
    row(
        "traffic",
        &format!(
            "RX = {}    TX = {}",
            fmt_bytes(n.bytes_rx as u64),
            fmt_bytes(n.bytes_tx as u64)
        ),
    );
}

fn print_host_block(s: &SnapshotMsg) {
    let header = match &s.host {
        Some(h) => {
            let age = h
                .age_ms
                .map(|m| format!("{}ms ago", m))
                .unwrap_or_else(|| "no data".into());
            format!("host   ({age})")
        }
        None => "host   (no data yet)".into(),
    };
    println!("{header}");

    let Some(h) = &s.host else { return };
    let temp_c = h.cpu_temp_dc as f32 / 10.0;
    let load = h.load_avg_x100 as f32 / 100.0;
    let cpu_usage = h
        .cpu_usage_pct
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "n/a".into());

    row(
        "cpu",
        &format!("T = {temp_c:.1} °C    usage = {cpu_usage}    load = {load:.2}"),
    );
    row("memory", &format!("{}% used", h.mem_used_pct));
    row("disk (/)", &format!("{}% used", h.disk_used_pct));
    row("uptime", &fmt_uptime(h.uptime_s));

    let net_rate = match (h.net_rx_bytes_per_s, h.net_tx_bytes_per_s) {
        (Some(r), Some(t)) => format!("↓ {}/s    ↑ {}/s", fmt_bytes(r), fmt_bytes(t)),
        _ => "n/a".into(),
    };
    let net_total = match (h.net_bytes_rx_total, h.net_bytes_tx_total) {
        (Some(r), Some(t)) => format!("(total ↓ {} / ↑ {})", fmt_bytes(r), fmt_bytes(t)),
        _ => String::new(),
    };
    row("network", &format!("{net_rate}   {net_total}"));

    let (eth_exec, eth_cons, eth_val) = crate::host_metrics::eth::unpack(h.eth_client_state);
    row("eth execution", crate::host_metrics::eth::state_name(eth_exec));
    row("eth consensus", crate::host_metrics::eth::state_name(eth_cons));
    row("eth validator", crate::host_metrics::eth::state_name(eth_val));
}

fn row(label: &str, value: &str) {
    println!("  {label:<width$}  {value}", width = LBL);
}

fn fmt_mv(mv: i32) -> String {
    // mV → "X.XX" volts
    let v = mv as f32 / 1000.0;
    format!("{v:.2}")
}

fn fmt_ma(ma: i32) -> String {
    // mA → "X.XX" amps (handle sign)
    let a = ma as f32 / 1000.0;
    format!("{a:.2}")
}

fn fmt_uptime(secs: u32) -> String {
    let d = secs / 86_400;
    let h = (secs / 3_600) % 24;
    let m = (secs / 60) % 60;
    let ss = secs % 60;
    if d > 0 {
        format!("{d}d {h}h {m}m {ss}s")
    } else if h > 0 {
        format!("{h}h {m}m {ss}s")
    } else if m > 0 {
        format!("{m}m {ss}s")
    } else {
        format!("{ss}s")
    }
}

fn format_clock_utc(unix_ms: u64) -> String {
    // Render unix epoch ms as "YYYY-MM-DD HH:MM:SS UTC". Civil date via
    // Howard Hinnant's days_from_civil inverse — avoids pulling chrono.
    let secs = unix_ms / 1000;
    let days = (secs / 86_400) as i64;
    let (y, mo, d) = days_to_ymd(days);
    let h = (secs / 3_600) % 24;
    let mi = (secs / 60) % 60;
    let s = secs % 60;
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

// Howard Hinnant's "days from civil" inverse: convert days-from-epoch to (Y, M, D).
// https://howardhinnant.github.io/date_algorithms.html#civil_from_days
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
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

// eth client state is now decoded via host_metrics::eth::{unpack, state_name}.

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
