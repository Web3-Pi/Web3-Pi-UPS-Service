# Web3 Pi UPS Service

A bidirectional Linux host agent for the [Web3 Pi UPS](https://github.com/Web3-Pi/Web3-Pi-UPS). It communicates with the RP2040 hub over USB serial using the WUPS v1 binary protocol, reports Raspberry Pi metrics to the device and remote panel, and triggers a graceful shutdown when the battery runs low during a power outage.

The crate contains two binaries: `w3p-ups`, the systemd agent with `status` / `watch` commands, and `ups-live`, a desktop terminal dashboard for direct USB diagnostics. The packaged service version is defined in [Cargo.toml](Cargo.toml).

## Features

- Bidirectional protocol agent — reads `power.status` / `net.status` / `power.event` and writes `host.status` back to the UPS controller (drives the on-device OLED).
- Battery State of Charge computed locally from a hardcoded LUT for the Web3 Pi UPS 2S Panasonic CGR18650CH pack (matches the OLED reading).
- Initiates graceful shutdown when SOC drops below threshold **and** input PD voltage indicates grid loss; cancels if power is restored during the grace period (with an anti-flap margin).
- Monitors execution, consensus and validator systemd units separately; reports running/stopped/failed/unknown state.
- Accepts whitelisted `host.service.start`, `host.service.stop` and `host.service.restart` commands, plus host shutdown/reboot requests relayed by the device.
- Exposes a read-only Unix-domain IPC socket for the bundled `status` / `watch` CLI (and future tools).
- Systemd integration with journald logging and automatic reconnect on serial errors.
- Auto-detects the UPS USB device (or accepts an explicit `/dev/ttyACM*` path).

## Requirements

- Raspberry Pi 5 (or compatible ARM64 device)
- Armbian/Ubuntu 24.04+ or similar Linux distribution
- Web3 Pi UPS hardware connected via a data-capable USB-C cable (RP2040 hub speaking the WUPS v1 binary protocol)
- `systemd` for service management

The release installer supports Linux **aarch64** only. The `ups-live` binary can also be built for macOS; see [the desktop dashboard](#ups-live--desktop-live-dashboard) below.

## Installation

### One-liner Install

```bash
curl -fsSL https://raw.githubusercontent.com/Web3-Pi/Web3-Pi-UPS-Service/main/install.sh | sudo bash
```

The installer downloads the latest GitHub release, installs and starts the systemd service, and preserves an existing configuration and shutdown script. When updating, stop the service before running the installer, compare `/etc/w3p-ups/config.toml` with the newly installed `config.toml.example`, then restart the service.

For older installations, replace legacy `w3p_*` entries in `service_whitelist` with the actual systemd units on your host. The current defaults are `geth`, `nimbus-beacon-node` and `nimbus-validator`. A preserved configuration is not migrated automatically.

### Manual Installation

1. Download the `w3p-ups-<tag>-aarch64.tar.gz` archive from [GitHub Releases](https://github.com/Web3-Pi/Web3-Pi-UPS-Service/releases).

2. In an empty working directory containing that archive, extract and install. These commands are for a fresh installation:

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

Edit `/etc/w3p-ups/config.toml` and restart `w3p-ups`. See [config.toml.example](config.toml.example) for the full configuration:

```toml
[serial]
# "auto" detects the Web3_Pi_UPS USB device, or set a path like "/dev/ttyACM0".
port = "auto"
baud_rate = 115200

[battery]
shutdown_threshold_pct = 10        # Critical SOC % — below this triggers shutdown when on battery
shutdown_cancel_margin_pct = 5     # SOC recovery margin when grid power is still absent
input_min_valid_mv = 8000          # PD input voltage range that means grid is present;
input_max_valid_mv = 26000         # outside this range → on battery

[shutdown]
script_path = "/etc/w3p-ups/shutdown.sh"
delay_seconds = 30                 # Grace period before shutdown

[host_metrics]
interval_seconds = 30              # Period between host.status emissions to the UPS. 0 disables.

[commands]
allow_service_restart = true       # Controls ALL start/stop/restart requests
service_whitelist = [              # Allowed systemd units, without `.service`
    "geth",
    "nimbus-beacon-node",
    "nimbus-validator",
]

[eth_clients]
execution = "geth"
consensus = "nimbus-beacon-node"
validator = "nimbus-validator"     # Empty string disables monitoring for this role

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

The pending shutdown is cancelled if **either** grid power returns **or** SOC reaches `shutdown_threshold_pct + shutdown_cancel_margin_pct` (15% with the defaults). When the grace period has elapsed, the script runs on a tick where the battery is still critical and grid power is absent. The bundled [shutdown script](scripts/shutdown.sh) stops Ethereum services, syncs filesystems and requests system shutdown.

### Ethereum Clients and Remote Commands

`[eth_clients]` selects the systemd units to monitor. These are service states, not Ethereum chain-sync status: the agent does not query client RPC endpoints. Use the same actual unit names in `[commands].service_whitelist` to allow the panel to start, stop or restart them.

Despite its historical name, `allow_service_restart` controls all three service actions. It does not disable separate host shutdown/reboot requests or low-battery shutdown. Service command responses distinguish success (`0`), malformed requests (`1`), denied units/actions (`2`) and `systemctl` failures (`3`).

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
- `host.service.start` / `host.service.stop` / `host.service.restart` REQ — act on a whitelisted systemd unit
- `host.shutdown` / `host.reset` REQ — shut down or reboot the host

Frames the agent emits to the UPS:

- `host.status` — CPU temp, memory %, disk %, 1-min load, uptime, Ethereum client state (rendered on the OLED)
- Shutdown announcements and responses to device requests

The daemon accepts `power.status` payload versions 1 and 2; payload versions are separate from the WUPS framing version. It normalizes v2 into the legacy state used by the shutdown logic and CLI. See [`src/proto/`](src/proto/) for the implemented payloads and the shared [firmware protocol header](https://github.com/Web3-Pi/Web3-Pi-UPS/blob/main/common/protocol.h) for the bus specification.

## ups-live — desktop live dashboard

A second binary in this crate: a single-screen terminal dashboard for bench
work. With USB-PD DR_Swap support in the UPS firmware, plugging the UPS output into a laptop
gives the laptop the USB host role (PD DR_Swap), so the WUPS stream is
available directly on macOS/Linux — no Raspberry Pi needed.

```bash
cargo run --release --bin ups-live          # auto-detects the UPS (2e8a:000a)
cargo run --release --bin ups-live -- /dev/cu.usbmodemXXXX
```

Shows the live power path (input/PD contracts/output rail), battery, charge
state, temperatures and faults from `power.status` v2, plus a scrolling tail
of `system.log` frames — including the CH32X `PD: ...` protocol event trace —
and `power.event` broadcasts. Reuses the agent's `proto` module via the crate
library target; no protocol duplication.

Requirements:

- Stable Rust toolchain (`rustup`); install Linux build dependencies as described below
- UPS firmware with DR_Swap support — older
  firmware never hands the USB host role to a laptop, so no serial device
  appears
- on first attach macOS asks to allow the "Web3_Pi_UPS" accessory — click
  Allow

The release archive and installer contain `w3p-ups` only; build `ups-live` from source. On a Raspberry Pi, use `w3p-ups status`, `w3p-ups watch` or `journalctl -u w3p-ups -f` alongside the running agent. Stop the agent for a direct `ups-live` or Workbench session and restart it afterwards to restore shutdown monitoring. Only one client should use the serial port at a time.

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
w3p-ups -c /path/config.toml # Run the daemon with a custom config

w3p-ups status              # Print one snapshot from the running daemon and exit
w3p-ups watch               # Stream live snapshots (Ctrl-C to stop)
```

`status` / `watch` connect to the IPC socket at `/run/w3p-ups/agent.sock` and render power, network, and host blocks read from the daemon's in-memory snapshot. They do not open the serial port. If the daemon uses a custom config/socket path, pass the same `-c` option to the CLI.

## Customizing Shutdown Script

Edit `/etc/w3p-ups/shutdown.sh` to add custom shutdown procedures:

```bash
#!/bin/sh
# Stop your services gracefully before shutdown
systemctl stop my-important-service
sync
shutdown -h now
```

The agent invokes this script with `sh`, so keep custom commands compatible with the system's POSIX shell.

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

# Stop the daemon before testing it in the foreground
sudo systemctl stop w3p-ups
sudo /usr/local/bin/w3p-ups -c /etc/w3p-ups/config.toml
# After Ctrl-C, restore the service
sudo systemctl start w3p-ups
```

### No frames received from the UPS

- Verify the Web3 Pi UPS is connected and powered.
- Check baud rate matches (default: 115200).
- Confirm the UPS firmware is on a compatible WUPS v1 build (older firmware emitting JSON is not supported by this service).
- Close any Workbench, `ups-live` or serial-terminal session using the same device.
- Bump log level to `debug` in `[logging]` to see deframer activity.

## Building from Source

Use a stable Rust toolchain, matching CI. Run these commands from the repository root:

```bash
# Install build dependencies (Debian/Ubuntu)
sudo apt install -y build-essential pkg-config libudev-dev

# Native build
cargo build --release --locked

# Same local checks used by CI
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

For an ARM64 Linux cross-build, CI uses `cross` with the target dependencies in [Cross.toml](Cross.toml). A working Docker-compatible container runtime is required:

```bash
cargo install cross --locked
cross build --release --target aarch64-unknown-linux-gnu --locked
```

For a fresh native installation, install the built agent and repository support files:

```bash
sudo install -m 755 target/release/w3p-ups /usr/local/bin/
sudo mkdir -p /etc/w3p-ups
sudo cp config.toml.example /etc/w3p-ups/config.toml
sudo cp scripts/shutdown.sh /etc/w3p-ups/
sudo chmod +x /etc/w3p-ups/shutdown.sh
sudo cp systemd/w3p-ups.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now w3p-ups
```

For a cross-build, use `target/aarch64-unknown-linux-gnu/release/w3p-ups` as the binary path on the target host. The [release workflow](.github/workflows/release.yml) packages the binary, example config, shutdown script and systemd unit.

## Related Projects

- [Web3-Pi-UPS](https://github.com/Web3-Pi/Web3-Pi-UPS) — hardware, firmware and shared WUPS protocol
- [Web3-Pi-UPS-Panel](https://github.com/Web3-Pi/Web3-Pi-UPS-Panel) — remote telemetry and device/host commands
- [Web3-Pi-UPS-Workbench](https://github.com/Web3-Pi/Web3-Pi-UPS-Workbench) — direct USB browser dashboard and firmware tools
- [Web3 Pi UPS documentation](https://docs.web3pi.io/ups/) — user documentation

## License

[GNU General Public License v3.0 only](LICENSE).
