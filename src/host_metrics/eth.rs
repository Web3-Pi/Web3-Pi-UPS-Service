//! ETH client monitoring — systemd *service* state (running / stopped /
//! failed), NOT chain sync status. We deliberately do not query geth/nimbus
//! RPC for synced/syncing; we only report `systemctl is-active <unit>`.
//!
//! Wire encoding (no protocol change): the three per-client states are packed
//! into the single existing `wups_host_status_v1_t.eth_client_state` byte, two
//! bits each — so the firmware (RP2040/ESP32) keeps forwarding the same 12-byte
//! frame untouched. Layout:
//!
//! ```text
//!   bit7 bit6 | bit5 bit4 | bit3 bit2 | bit1 bit0
//!   reserved  | validator | consensus | execution
//! ```

use tokio::process::Command;

use crate::config::EthClientsConfig;

/// 2-bit per-client service state.
pub const ST_UNKNOWN: u8 = 0; // not configured / not installed / query failed
pub const ST_RUNNING: u8 = 1; // systemd ActiveState = active (or activating)
pub const ST_STOPPED: u8 = 2; // systemd ActiveState = inactive (or deactivating)
pub const ST_FAILED: u8 = 3; // systemd ActiveState = failed

/// `systemctl is-active <unit>` → 2-bit state. Empty unit ⇒ not monitored
/// (`ST_UNKNOWN`). A missing `systemctl` (non-systemd / dev host) also maps to
/// `ST_UNKNOWN` rather than erroring — the agent must keep emitting host.status.
async fn unit_state(unit: &str) -> u8 {
    if unit.trim().is_empty() {
        return ST_UNKNOWN;
    }
    match Command::new("systemctl")
        .arg("is-active")
        .arg(unit)
        .output()
        .await
    {
        Ok(out) => match String::from_utf8_lossy(&out.stdout).trim() {
            "active" | "activating" | "reloading" => ST_RUNNING,
            "inactive" | "deactivating" => ST_STOPPED,
            "failed" => ST_FAILED,
            _ => ST_UNKNOWN, // "unknown", unit-not-found, empty, etc.
        },
        Err(_) => ST_UNKNOWN,
    }
}

/// Query all three configured units and return the packed `eth_client_state`
/// byte for `host.status`.
pub async fn read_packed_state(cfg: &EthClientsConfig) -> u8 {
    let execution = unit_state(&cfg.execution).await;
    let consensus = unit_state(&cfg.consensus).await;
    let validator = unit_state(&cfg.validator).await;
    pack(execution, consensus, validator)
}

/// Pack three 2-bit states into one byte (execution = low bits).
pub fn pack(execution: u8, consensus: u8, validator: u8) -> u8 {
    (execution & 0x3) | ((consensus & 0x3) << 2) | ((validator & 0x3) << 4)
}

/// Inverse of [`pack`]: `(execution, consensus, validator)`.
pub fn unpack(b: u8) -> (u8, u8, u8) {
    (b & 0x3, (b >> 2) & 0x3, (b >> 4) & 0x3)
}

/// Human label for a 2-bit state, for the local `w3p-ups status` CLI.
pub fn state_name(s: u8) -> &'static str {
    match s {
        ST_RUNNING => "running",
        ST_STOPPED => "stopped",
        ST_FAILED => "failed",
        _ => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_round_trip() {
        for e in 0..=3u8 {
            for c in 0..=3u8 {
                for v in 0..=3u8 {
                    assert_eq!(unpack(pack(e, c, v)), (e, c, v));
                }
            }
        }
    }

    #[test]
    fn reserved_bits_stay_zero() {
        // Only bits 0..=5 are used; the top two bits must never be set.
        assert_eq!(pack(3, 3, 3), 0b0011_1111);
    }

    #[test]
    fn names() {
        assert_eq!(state_name(ST_RUNNING), "running");
        assert_eq!(state_name(ST_STOPPED), "stopped");
        assert_eq!(state_name(ST_FAILED), "failed");
        assert_eq!(state_name(ST_UNKNOWN), "—");
    }
}
