//! ups-live — live terminal dashboard for the Web3 Pi UPS.
//!
//! Opens the UPS's USB-CDC serial port (RP2040, `Web3_Pi Web3_Pi_UPS`,
//! VID:PID 2e8a:000a), deframes the WUPS binary v1 stream and renders a
//! single-screen ANSI dashboard: power path, battery, temperatures, faults,
//! plus a scrolling tail of `system.log` frames (including the CH32X
//! `PD: ...` event trace) and `power.event` broadcasts.
//!
//! Runs anywhere the serial port shows up — developed for macOS bench use
//! (the DR_Swap feature makes a laptop on the UPS output a first-class
//! host), works the same on Linux against /dev/ttyACM*.
//!
//!   cargo run --bin ups-live                  # auto-detect the UPS port
//!   cargo run --bin ups-live -- /dev/cu.usbmodemXXXX

use std::collections::VecDeque;
use std::io::Write as _;
use std::time::Instant;

use clap::Parser;
use tokio::io::AsyncReadExt;
use tokio_serial::{SerialPortBuilderExt, SerialPortType};

use w3p_ups::proto::payloads::{power2_flag, power_event, PowerEventV1, PowerStatusV2, SysLogV1};
use w3p_ups::proto::{addr, class, op, Deframer};

/// USB identity of the RP2040 CDC (see firmware-rp2040/platformio.ini).
const UPS_VID: u16 = 0x2e8a;
const UPS_PID: u16 = 0x000a;

const LOG_LINES: usize = 12;

#[derive(Parser)]
#[command(
    name = "ups-live",
    about = "Live terminal dashboard for the Web3 Pi UPS (WUPS binary v1 over USB-CDC)"
)]
struct Args {
    /// Serial port path; auto-detects the Web3_Pi_UPS device when omitted.
    port: Option<String>,
}

struct App {
    started: Instant,
    status: Option<PowerStatusV2>,
    status_at: Option<Instant>,
    frames_ok: u64,
    frames_err: u64,
    logs: VecDeque<String>,
}

impl App {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            status: None,
            status_at: None,
            frames_ok: 0,
            frames_err: 0,
            logs: VecDeque::with_capacity(LOG_LINES + 1),
        }
    }

    fn push_log(&mut self, line: String) {
        let t = self.started.elapsed().as_secs_f32();
        self.logs.push_back(format!("{t:>7.1}s  {line}"));
        while self.logs.len() > LOG_LINES {
            self.logs.pop_front();
        }
    }
}

fn node_name(a: u8) -> &'static str {
    match a {
        addr::RPI => "RPi",
        addr::RP2040 => "RP2040",
        addr::CH32X => "CH32X",
        addr::ESP32 => "ESP32",
        addr::INTERNAL => "INT",
        addr::BROADCAST => "BCAST",
        _ => "?",
    }
}

fn charge_state_name(cs: u8) -> &'static str {
    match cs {
        0 => "not charging",
        1 => "pre-charge",
        2 => "charging",
        3 => "full",
        _ => "?",
    }
}

fn power_event_name(e: u8) -> &'static str {
    match e {
        power_event::MAINS_LOST => "MAINS LOST",
        power_event::MAINS_RESTORED => "MAINS RESTORED",
        power_event::CHARGE_LOW => "CHARGE LOW",
        power_event::CHARGE_FULL => "CHARGE FULL",
        power_event::FAULT => "FAULT",
        _ => "EVENT?",
    }
}

fn autodetect() -> Option<String> {
    let ports = tokio_serial::available_ports().ok()?;
    // Prefer an exact VID:PID match; macOS lists both /dev/tty.* and
    // /dev/cu.* for the same device — cu.* is the callout side we want.
    let mut best: Option<String> = None;
    for p in ports {
        if let SerialPortType::UsbPort(info) = &p.port_type {
            if info.vid == UPS_VID && info.pid == UPS_PID {
                let name = p.port_name.clone();
                let is_cu = name.contains("/cu.");
                match &best {
                    Some(b) if b.contains("/cu.") && !is_cu => {}
                    _ => best = Some(name),
                }
            }
        }
    }
    best
}

fn fmt_v(mv: u16) -> String {
    format!("{:.2} V", mv as f32 / 1000.0)
}

fn fmt_a(ma: u16) -> String {
    format!("{:.2} A", ma as f32 / 1000.0)
}

fn fmt_temp(dc: i16) -> String {
    if dc == i16::MIN {
        "n/a".to_string()
    } else {
        format!("{:.1}\u{b0}C", dc as f32 / 10.0)
    }
}

fn fmt_uptime(s: u32) -> String {
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

const CLR: &str = "\x1b[K"; // clear to end of line
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

fn render(app: &App, port: &str) {
    let mut out = String::with_capacity(4096);
    out.push_str("\x1b[H"); // cursor home; full clear only at startup

    out.push_str(&format!(
        "{BOLD}Web3 Pi UPS \u{2014} live{RESET}  {DIM}{port}{RESET}{CLR}\n"
    ));

    match &app.status {
        None => {
            out.push_str(&format!("{CLR}\n{YELLOW}waiting for power.status\u{2026}{RESET}{CLR}\n"));
            for _ in 0..6 {
                out.push_str(&format!("{CLR}\n"));
            }
        }
        Some(s) => {
            let f = s.flags;
            let on = |b: u8, yes: &str, no: &str| {
                if f & b != 0 {
                    format!("{GREEN}{yes}{RESET}")
                } else {
                    format!("{RED}{no}{RESET}")
                }
            };
            let age = app
                .status_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let stale = if age > 3.0 {
                format!("  {RED}STALE {age:.0}s{RESET}")
            } else {
                String::new()
            };

            out.push_str(&format!(
                "{DIM}ups uptime {}   frames {} (crc err {}){RESET}{stale}{CLR}\n",
                fmt_uptime(s.uptime_s),
                app.frames_ok,
                app.frames_err
            ));
            out.push_str(&format!("{CLR}\n"));

            out.push_str(&format!(
                "{BOLD}INPUT{RESET}    mains {}   usb-c {}   Vin {}   PD-in {} / {}   Iin {}{CLR}\n",
                on(power2_flag::POWER_GOOD, "OK", "LOST"),
                on(power2_flag::USB_C_ATTACH, "attached", "none"),
                fmt_v(s.vbus_in_mv),
                fmt_v(s.pd_in_mv),
                fmt_a(s.pd_in_ma),
                fmt_a(s.iin_ma),
            ));

            out.push_str(&format!(
                "{BOLD}OUTPUT{RESET}   rail {}   PD-out {} / {}   set {}   read {}   ilim {}{CLR}\n",
                on(power2_flag::VBUS_OUT_EN, "ON", "OFF"),
                fmt_v(s.pd_out_mv),
                fmt_a(s.pd_out_ma),
                fmt_v(s.vout_set_mv),
                fmt_v(s.vout_read_mv),
                fmt_a(s.iout_limit_ma),
            ));

            out.push_str(&format!(
                "{BOLD}BATTERY{RESET}  {}   {}   chg {} mA   {}{CLR}\n",
                on(power2_flag::BATT_PRESENT, "present", "MISSING"),
                fmt_v(s.vbat_mv),
                s.ichg_ma,
                charge_state_name(s.charge_state),
            ));

            let faults = if s.faults == 0 {
                format!("{GREEN}none{RESET}")
            } else {
                format!("{RED}0x{:04X}{RESET}", s.faults)
            };
            out.push_str(&format!(
                "{BOLD}SYSTEM{RESET}   Vsys {}   temp board {} / chg {}   faults {}{CLR}\n",
                fmt_v(s.vsys_mv),
                fmt_temp(s.temp_lm_dc),
                fmt_temp(s.temp_mp_dc),
                faults,
            ));
            out.push_str(&format!("{CLR}\n"));
        }
    }

    out.push_str(&format!("{BOLD}log{RESET}{CLR}\n"));
    for i in 0..LOG_LINES {
        match app.logs.get(i) {
            Some(l) => out.push_str(&format!("  {l}{CLR}\n")),
            None => out.push_str(&format!("{CLR}\n")),
        }
    }
    out.push_str(&format!(
        "{DIM}Ctrl-C to quit{RESET}{CLR}\n\x1b[J"
    ));

    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
}

fn handle_frame(app: &mut App, frame: w3p_ups::proto::Frame) {
    app.frames_ok += 1;
    match (frame.class, frame.op) {
        (class::POWER, o) if o == op::power::STATUS => {
            if let Ok(s) = PowerStatusV2::decode(&frame.payload) {
                app.status = Some(s);
                app.status_at = Some(Instant::now());
            }
        }
        (class::POWER, o) if o == op::power::EVENT => {
            if let Ok(e) = PowerEventV1::decode(&frame.payload) {
                app.push_log(format!(
                    "{YELLOW}\u{26a1} power.event: {}{RESET}",
                    power_event_name(e.event)
                ));
            }
        }
        (class::SYSTEM, o) if o == op::system::LOG => {
            if let Ok(l) = SysLogV1::decode(&frame.payload) {
                let text = String::from_utf8_lossy(&l.text).to_string();
                app.push_log(format!("[{}] {}", node_name(frame.src), text));
            }
        }
        _ => {}
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let port_path = match args.port.or_else(autodetect) {
        Some(p) => p,
        None => anyhow::bail!(
            "no Web3_Pi_UPS (2e8a:000a) serial device found \u{2014} pass the port path explicitly"
        ),
    };

    // USB-CDC ignores the baud rate; 115200 matches the RP2040's Serial.begin.
    let mut port = tokio_serial::new(&port_path, 115_200).open_native_async()?;

    print!("\x1b[2J\x1b[?25l"); // clear screen, hide cursor
    let restore = || print!("\x1b[?25h\x1b[0m\n");

    let mut app = App::new();
    app.push_log(format!("connected to {port_path}"));
    render(&app, &port_path);

    let mut buf = [0u8; 1024];
    let mut deframer = Deframer::new();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            r = port.read(&mut buf) => {
                let n = match r {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        app.push_log(format!("{RED}serial error: {e}{RESET}"));
                        render(&app, &port_path);
                        break;
                    }
                };
                let mut frames = Vec::new();
                deframer.feed_slice(&buf[..n], |r| frames.push(r));
                let mut dirty = false;
                for r in frames {
                    match r {
                        Ok(f) => { handle_frame(&mut app, f); dirty = true; }
                        Err(_) => { app.frames_err += 1; }
                    }
                }
                if dirty {
                    render(&app, &port_path);
                }
            }
        }
    }

    restore();
    Ok(())
}
