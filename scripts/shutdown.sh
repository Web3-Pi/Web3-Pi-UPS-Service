#!/bin/bash
# Web3 Pi UPS Shutdown Script
# This script is called when battery level is critical and no grid power is available
# Customize this script to gracefully stop your services before shutdown

set -e

LOGFILE="/var/log/w3p-ups-shutdown.log"

log() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') - $1" | tee -a "$LOGFILE"
}

log "=== UPS Shutdown initiated ==="
log "Battery critical - starting graceful shutdown sequence"

# Add your custom shutdown commands here
# Example: Stop Docker containers gracefully
# if command -v docker &> /dev/null; then
#     log "Stopping Docker containers..."
#     docker stop $(docker ps -q) 2>/dev/null || true
# fi

# Example: Stop specific services
# log "Stopping web3-pi services..."
# systemctl stop web3-pi-node 2>/dev/null || true

# Example: Sync filesystems
log "Syncing filesystems..."
sync

# Final system shutdown
log "Executing system shutdown..."
shutdown -h now

log "Shutdown command sent"
