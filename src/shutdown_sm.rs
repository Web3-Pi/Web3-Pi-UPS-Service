use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{error, info, warn};

use crate::config::{BatteryConfig, ShutdownConfig};
use crate::proto::payloads::{host_event, host_shutdown_reason, HostEventV1, HostShutdownV1};
use crate::proto::{addr, class, flag, op, Frame};
use crate::soc::pack_mv_to_soc_pct;
use crate::state::State;
use crate::transport::OutboundFrame;

/// Whether the input voltage indicates we are running on battery.
pub fn is_on_battery(vbus_in_mv: u16, min: u16, max: u16) -> bool {
    vbus_in_mv < min || vbus_in_mv > max
}

/// 1 Hz tick: re-evaluate the low-battery shutdown decision.
pub async fn shutdown_sm_loop(
    state: Arc<State>,
    battery: BatteryConfig,
    shutdown: ShutdownConfig,
    out_tx: mpsc::Sender<OutboundFrame>,
) {
    info!(
        threshold_pct = battery.shutdown_threshold_pct,
        cancel_margin_pct = battery.shutdown_cancel_margin_pct,
        delay_s = shutdown.delay_seconds,
        script = %shutdown.script_path,
        "shutdown SM running"
    );
    let mut tick = interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        if step(&state, &battery, &shutdown, &out_tx).await {
            // Shutdown initiated; block here so the supervisor doesn't
            // restart us before the system actually powers down.
            wait_forever().await;
        }
    }
}

/// One SM step. Returns `true` if shutdown was just initiated.
async fn step(
    state: &State,
    battery: &BatteryConfig,
    shutdown: &ShutdownConfig,
    out_tx: &mpsc::Sender<OutboundFrame>,
) -> bool {
    let snap = state.snapshot().await;
    let Some(power) = snap.last_power else {
        return false;
    };

    let soc = pack_mv_to_soc_pct(power.vbat_mv);
    let on_batt = is_on_battery(
        power.vbus_in_mv,
        battery.input_min_valid_mv,
        battery.input_max_valid_mv,
    );
    let critical = soc < battery.shutdown_threshold_pct;

    match (snap.shutdown_pending_since, critical && on_batt) {
        (None, true) => {
            warn!(
                soc,
                vbat_mv = power.vbat_mv,
                vbus_in_mv = power.vbus_in_mv,
                "low battery on battery power; shutdown in {} s unless restored",
                shutdown.delay_seconds
            );
            state.set_shutdown_pending(Some(Instant::now())).await;
            announce_shutdown_imminent(out_tx).await;
            false
        }
        (Some(start), true) => {
            let elapsed = start.elapsed().as_secs();
            if elapsed >= shutdown.delay_seconds {
                warn!(
                    soc,
                    vbus_in_mv = power.vbus_in_mv,
                    "delay elapsed; initiating shutdown"
                );
                trigger_shutdown(shutdown).await;
                true
            } else {
                let remaining = shutdown.delay_seconds - elapsed;
                warn!(soc, "shutdown countdown: {remaining} s remaining");
                false
            }
        }
        (Some(_), false) => {
            // Cancellation hysteresis: only cancel if power is truly back OR the
            // battery has cleared the cancel margin above the threshold.
            let cancel_threshold = battery
                .shutdown_threshold_pct
                .saturating_add(battery.shutdown_cancel_margin_pct);
            let recovered = soc >= cancel_threshold;
            let restored = !on_batt;
            if recovered || restored {
                info!(
                    soc,
                    on_batt,
                    "shutdown cancelled ({})",
                    if restored {
                        "power restored"
                    } else {
                        "battery recovered"
                    }
                );
                state.set_shutdown_pending(None).await;
            }
            false
        }
        (None, false) => false,
    }
}

async fn announce_shutdown_imminent(out_tx: &mpsc::Sender<OutboundFrame>) {
    let payload = HostEventV1 {
        event: host_event::SHUTDOWN_IMMINENT,
    }
    .encode();
    let frame = Frame {
        dst: addr::BROADCAST,
        src: addr::RPI,
        class: class::HOST,
        op: op::host::EVENT,
        flags: flag::EVENT,
        seq: 0, // best-effort EVENT, no correlation
        payload: payload.to_vec(),
    };
    if out_tx.send(OutboundFrame { frame }).await.is_err() {
        warn!("could not announce host.event SHUTDOWN_IMMINENT (outbound channel closed)");
    }

    // Also emit the canonical host.shutdown REQ payload as a broadcast hint so
    // peers (RP2040 OLED, ESP32 cloud relay) know the reason and delay.
    let payload = HostShutdownV1 {
        reason: host_shutdown_reason::LOW_BATTERY,
        delay_s: 0,
    }
    .encode();
    let frame = Frame {
        dst: addr::BROADCAST,
        src: addr::RPI,
        class: class::HOST,
        op: op::host::SHUTDOWN,
        flags: flag::EVENT,
        seq: 0,
        payload: payload.to_vec(),
    };
    let _ = out_tx.send(OutboundFrame { frame }).await;
}

async fn trigger_shutdown(shutdown: &ShutdownConfig) {
    let path = &shutdown.script_path;
    if Path::new(path).exists() {
        info!("executing shutdown script: {path}");
        match Command::new("sh").arg(path).spawn() {
            Ok(_child) => {}
            Err(e) => {
                error!("failed to spawn shutdown script: {e}");
                fallback_shutdown().await;
            }
        }
    } else {
        warn!("shutdown script not found at {path}; falling back to `shutdown -h now`");
        fallback_shutdown().await;
    }
}

async fn fallback_shutdown() {
    if let Err(e) = Command::new("shutdown").args(["-h", "now"]).spawn() {
        error!("fallback shutdown failed: {e}");
    }
}

async fn wait_forever() -> ! {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_battery_below_min() {
        assert!(is_on_battery(4000, 8000, 26000));
    }

    #[test]
    fn on_battery_above_max() {
        assert!(is_on_battery(30000, 8000, 26000));
    }

    #[test]
    fn on_grid_in_range() {
        assert!(!is_on_battery(12000, 8000, 26000));
    }
}
