#!/bin/bash
# Web3 Pi UPS Shutdown Script
# This script is called when battery level is critical and no grid power is available
# Customize this script to gracefully stop your services before shutdown

LOGFILE="/var/log/w3p-ups-shutdown.log"

log() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') - $1" | tee -a "$LOGFILE"
}

log "=== UPS Emergency Shutdown ==="
log "Reason: Grid power lost and UPS battery critically low"
log "Starting graceful shutdown sequence..."

# Add your custom shutdown commands here
# Example: Stop specific services
# log "Stopping web3-pi services..."
# systemctl stop web3-pi-node 2>/dev/null || true

# Stop Ethereum services gracefully
log "Stopping Ethereum services..."
systemctl stop nimbus-validator.service 2>/dev/null
systemctl stop nimbus-beacon-node.service 2>/dev/null
systemctl stop geth.service 2>/dev/null

# Flush all buffered data from RAM to disk to prevent data loss
log "Syncing filesystems..."
sync

# Final system shutdown
log "Executing system shutdown..."
shutdown -h now

log "Shutdown command sent"
