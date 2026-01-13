# Web3 Pi UPS Service

A lightweight system service for monitoring UPS battery status on Raspberry Pi. Designed for use with RP2040 Pico-based UPS controllers that communicate via USB serial.

## Features

- Monitors battery State of Charge (SOC) and input voltage via serial port
- Initiates graceful shutdown when battery is low AND running on battery power
- Configurable thresholds and shutdown delay
- Systemd integration with journald logging
- Automatic retry on serial connection errors
- Cancels shutdown if power is restored during delay period

## Requirements

- Raspberry Pi 5 (or compatible ARM64 device)
- Armbian/Ubuntu 24.04+ or similar Linux distribution
- RP2040 Pico connected via USB-C outputting JSON status messages
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
port = "/dev/ttyACM0"
baud_rate = 115200

[battery]
shutdown_threshold = 10      # SOC % below which shutdown triggers
min_valid_voltage = 8000     # Input voltage (mV) below = on battery

[shutdown]
script_path = "/etc/w3p-ups/shutdown.sh"
delay_seconds = 30           # Grace period before shutdown

[logging]
level = "info"
# file_path = "/var/log/w3p-ups.log"  # Optional file logging
```

### Shutdown Logic

Shutdown is triggered when **BOTH** conditions are met:
1. Battery SOC is below `shutdown_threshold` (default: 10%)
2. Input voltage is below `min_valid_voltage` (default: 8000 mV), indicating no grid power

If power is restored during the `delay_seconds` period, the shutdown is cancelled.

## Expected JSON Format

The service expects JSON messages from the RP2040 Pico in this format:

```json
{"up":162,"pd":0,"pdo":0,"cc":0,"t":393,"vs":0,"is":0,"vr":28,"ir":50,"soc":100,"bv":8118,"ba":487,"cs":2,"pg":1,"vi":14925,"ii":550,"ci":875,"cf":0}
```

Key fields:
- `soc`: State of Charge (battery percentage, 0-100)
- `vi`: Input voltage in mV (8000-21000 indicates grid power present)

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

### Command Line Options

```bash
w3p-ups --help              # Show help
w3p-ups --version           # Show version
w3p-ups -c /path/config     # Use custom config file
```

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

### No JSON data received
- Verify the RP2040 Pico is connected and powered
- Check baud rate matches (default: 115200)
- Test with: `cat /dev/ttyACM0`

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
