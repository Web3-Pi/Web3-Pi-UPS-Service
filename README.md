# Web3 Pi UPS Service

A lightweight bidirectional agent for the Web3 Pi UPS hardware running on a Raspberry Pi. Speaks a binary UBX-style wire protocol (v1) over USB serial to the UPS controller (RP2040 / CH32X), reports host metrics back to the device, and triggers a graceful shutdown when the battery runs low on grid loss.

## Features

- Bidirectional protocol agent — reads `power.status` / `net.status` / `power.event` and writes `host.status` back to the UPS controller (drives the on-device OLED).
- Battery State of Charge computed locally from a hardcoded LUT for the Web3 Pi UPS 2S Panasonic CGR18650CH pack (matches the OLED reading).
- Initiates graceful shutdown when SOC drops below threshold **and** input PD voltage indicates grid loss; cancels if power is restored during the grace period (with an anti-flap margin).
- Accepts whitelisted `host.service.restart` commands from the device (e.g. `w3p_geth`, `w3p_nimbus-beacon`).
- Exposes a read-only Unix-domain IPC socket for the bundled `status` / `watch` CLI (and future tools).
- Systemd integration with journald logging and automatic reconnect on serial errors.
- Auto-detects the UPS USB device (or accepts an explicit `/dev/ttyACM*` path).

## Requirements

- Raspberry Pi 5 (or compatible ARM64 device)
- Armbian/Ubuntu 24.04+ or similar Linux distribution
- Web3 Pi UPS hardware connected via USB-C (RP2040 bridge speaking the WUPS v1 binary protocol)
- `systemd` for service management

## Installation

### One-liner Install

```bash
curl -fsSL https://raw.githubusercontent.com/Web3-Pi/Web3-Pi-UPS-Service/main/install.sh | sudo bash
```

### Manual Installation

1. Download the latest release from [GitHub Releases](https://github.com/Web3-Pi/Web3-Pi-UPS-Service/releases)

2. Extract and install:
```bash
tar -xzf w3p-ups-*.tar.gz
sudo install -m 755 w3p-ups /usr/local/bin/
sudo mkdir -p /etc/w3p-ups
sudo cp config.toml.example /etc/w3p-ups/config.toml
sudo cp shutdown.sh /etc/w3p-ups/
sudo chmod +x /etc/w3p-ups/shutdown.sh
sudo cp w3p-ups.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now w3p-ups
```

## Configuration

Edit `/etc/w3p-ups/config.toml`:

```toml
[serial]
# "auto" detects the Web3_Pi_UPS USB device, or set a path like "/dev/ttyACM0".
port = "auto"
baud_rate = 115200

[battery]
shutdown_threshold_pct = 10        # Critical SOC % — below this triggers shutdown when on battery
shutdown_cancel_margin_pct = 5     # Anti-flap: SOC must recover this far above threshold to cancel
input_min_valid_mv = 8000          # PD input voltage range that means grid is present;
input_max_valid_mv = 26000         # outside this range → on battery

[shutdown]
script_path = "/etc/w3p-ups/shutdown.sh"
delay_seconds = 30                 # Grace period before shutdown

[host_metrics]
interval_seconds = 30              # Period between host.status emissions to the UPS. 0 disables.

[commands]
allow_service_restart = true       # Master switch for host.service.restart REQs from the device
service_whitelist = [              # systemd units (without `.service`) that may be restarted
    "w3p_geth",
    "w3p_nimbus-beacon",
    "w3p_lighthouse-beacon",
    "nimbus-validator",
]

[ipc]
socket_path = "/run/w3p-ups/agent.sock"   # Unix socket for `status` / `watch`

[logging]
level = "info"                     # trace | debug | info | warn | error
journald = false                   # set true on systemd hosts to log via journald
```

### Shutdown Logic

Shutdown is triggered when **BOTH** conditions are met:
1. Battery SOC is below `shutdown_threshold_pct` (default: 10%)
2. PD input voltage is outside `input_min_valid_mv..input_max_valid_mv` (default 8000–26000 mV), indicating grid loss

If power is restored during the `delay_seconds` window and SOC recovers above `shutdown_threshold_pct + shutdown_cancel_margin_pct`, the pending shutdown is cancelled.

## Wire Protocol

The agent speaks the **WUPS v1** binary protocol over USB serial — a UBX-style framing format:

```text
AA 55 [DST][SRC][CLASS][OP][FLAGS][SEQ][LEN_L][LEN_H] [payload..LEN] [CK_A][CK_B] 55 AA
```

Fletcher-8 checksum covers the header bytes (`DST..LEN_H`) and payload. Total wire overhead is 14 bytes; maximum payload is 240 bytes.

Frames the agent consumes from the UPS:
- `power.status` — VBUS/VBAT/IBAT, charge state, temperature, faults (used to drive SOC and shutdown logic)
- `power.event` — `MAINS_LOST` / `MAINS_RESTORED` / `CHARGE_LOW` / `CHARGE_FULL` / `FAULT`
- `net.status` — RSSI/RSRP/RSRQ and traffic counters from the cellular modem (when present)
- `host.service.restart` REQ — restart a whitelisted systemd unit

Frames the agent emits to the UPS:
- `host.status` — CPU temp, memory %, disk %, 1-min load, uptime, Ethereum client state (rendered on the OLED)

See [`src/proto/`](src/proto/) for the complete payload catalogue.

## Usage

### Service Management

```bash
# Check service status
sudo systemctl status w3p-ups

# View live logs
sudo journalctl -u w3p-ups -f

# Restart service after config change
sudo systemctl restart w3p-ups

# Stop service
sudo systemctl stop w3p-ups
```

### CLI

```bash
w3p-ups --help              # Show help
w3p-ups --version           # Show version
w3p-ups -c /path/config     # Use custom config file

w3p-ups status              # Print one snapshot from the running daemon and exit
w3p-ups watch               # Stream live snapshots (Ctrl-C to stop)
```

`status` / `watch` connect to the IPC socket at `/run/w3p-ups/agent.sock` and render power, network, and host blocks read from the daemon's in-memory snapshot.

## Customizing Shutdown Script

Edit `/etc/w3p-ups/shutdown.sh` to add custom shutdown procedures:

```bash
#!/bin/bash
# Stop your services gracefully before shutdown
systemctl stop my-important-service
docker stop $(docker ps -q)
sync
shutdown -h now
```

## Uninstallation

```bash
curl -fsSL https://raw.githubusercontent.com/Web3-Pi/Web3-Pi-UPS-Service/main/install.sh | sudo bash -s -- --uninstall
```

Or manually:
```bash
sudo systemctl stop w3p-ups
sudo systemctl disable w3p-ups
sudo rm /usr/local/bin/w3p-ups
sudo rm /etc/systemd/system/w3p-ups.service
sudo systemctl daemon-reload
# Optionally remove config: sudo rm -rf /etc/w3p-ups
```

## Troubleshooting

### Serial port not found
```bash
# Check if device exists
ls -la /dev/ttyACM*

# Check permissions
sudo usermod -a -G dialout $USER
# Log out and back in for group change to take effect
```

### Service won't start
```bash
# Check detailed logs
sudo journalctl -u w3p-ups -e --no-pager

# Test manually
sudo /usr/local/bin/w3p-ups -c /etc/w3p-ups/config.toml
```

### No frames received from the UPS
- Verify the Web3 Pi UPS is connected and powered.
- Check baud rate matches (default: 115200).
- Confirm the UPS firmware is on a compatible WUPS v1 build (older firmware emitting JSON is not supported by this service).
- Sniff raw bytes: `sudo cat /dev/ttyACM0 | xxd | head` — you should see `AA 55 ...` frame starts.
- Bump log level to `debug` in `[logging]` to see deframer activity.

## Building from Source

Requires Rust 1.70+ and system dependencies:

```bash
# Install build dependencies (Debian/Ubuntu)
sudo apt install -y pkg-config libudev-dev

# Native build
cargo build --release

# Cross-compile for ARM64 (from x86_64)
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu

# Set as a service
sudo install -m 755 target/release/w3p-ups /usr/local/bin/
sudo mkdir -p /etc/w3p-ups
sudo cp config.toml.example /etc/w3p-ups/config.toml
sudo cp scripts/shutdown.sh /etc/w3p-ups/
sudo chmod +x /etc/w3p-ups/shutdown.sh
sudo cp systemd/w3p-ups.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now w3p-ups
```

## Part of Web3 Pi Project

This service is designed for the [Web3 Pi](https://web3pi.io) project, providing reliable power management for blockchain nodes running on Raspberry Pi.
