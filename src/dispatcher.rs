use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

use crate::commands::CommandsHandler;
use crate::proto::payloads::{
    HostEventV1, NetStatusV1, PowerEventV1, PowerStatusV1, PowerStatusV2, SysHelloV1, SysLogV1,
};
use crate::proto::{addr, class, flag, op, Frame};
use crate::state::State;
use crate::transport::OutboundFrame;

/// Loop forever (until inbound channel closes), dispatching incoming frames.
pub async fn dispatch_loop(
    state: Arc<State>,
    mut inbound: mpsc::Receiver<Frame>,
    outbound: mpsc::Sender<OutboundFrame>,
    commands: Arc<CommandsHandler>,
) {
    while let Some(frame) = inbound.recv().await {
        handle(&state, frame, &outbound, &commands).await;
    }
    info!("dispatcher: inbound closed; exiting");
}

async fn handle(
    state: &State,
    frame: Frame,
    outbound: &mpsc::Sender<OutboundFrame>,
    commands: &CommandsHandler,
) {
    let cls = frame.class;
    let opc = frame.op;
    let flags = frame.flags;

    match (cls, opc) {
        // ---- SYSTEM ----
        (class::SYSTEM, op::system::HELLO) => match SysHelloV1::decode(&frame.payload) {
            Ok(h) => {
                info!(
                    src = frame.src,
                    proto_version = h.proto_version,
                    fw_version = format!("{:#06x}", h.fw_version),
                    caps = format!("{:#06x}", h.caps_classes),
                    build_id = format!("{:#010x}", h.build_id),
                    "system.hello"
                );
                state.record_hello(frame.src, h).await;
            }
            Err(e) => warn!("system.hello decode: {e}"),
        },
        (class::SYSTEM, op::system::LOG) => match SysLogV1::decode(&frame.payload) {
            Ok(l) => {
                let text = String::from_utf8_lossy(&l.text);
                info!(src = frame.src, level = l.level, "remote log: {text}");
            }
            Err(e) => warn!("system.log decode: {e}"),
        },
        (class::SYSTEM, op::system::PING) if flags & flag::REQ != 0 => {
            // ping REQ — handled by command path in PR #6; for now, just trace.
            trace!(
                src = frame.src,
                seq = frame.seq,
                "system.ping REQ (unhandled)"
            );
        }
        (class::SYSTEM, op::system::PING) if flags & flag::RESP != 0 => {
            trace!(src = frame.src, seq = frame.seq, "system.ping RESP");
        }

        // ---- POWER ----
        (class::POWER, op::power::STATUS) => {
            // Dispatch on the version byte: v2 is decoded then down-converted
            // to v1 for storage (host stays v1-native for now); v1 as before.
            let decoded = match frame.payload.first().copied() {
                Some(2) => PowerStatusV2::decode(&frame.payload).map(|p2| p2.to_v1()),
                _ => PowerStatusV1::decode(&frame.payload),
            };
            match decoded {
                Ok(p) => {
                    debug!(
                        vbus_in_mv = p.vbus_in_mv,
                        vbat_mv = p.vbat_mv,
                        ibat_ma = p.ibat_ma,
                        charge_state = p.charge_state,
                        faults = format!("{:#06x}", p.faults),
                        "power.status"
                    );
                    state.update_power(p).await;
                }
                Err(e) => warn!("power.status decode: {e}"),
            }
        }
        (class::POWER, op::power::EVENT) => match PowerEventV1::decode(&frame.payload) {
            Ok(e) => {
                info!(event = e.event, "power.event");
                state.update_power_event(e.event).await;
            }
            Err(err) => warn!("power.event decode: {err}"),
        },

        // ---- NET ----
        (class::NET, op::net::STATUS) => match NetStatusV1::decode(&frame.payload) {
            Ok(n) => {
                debug!(
                    state = n.state,
                    rssi_dbm = n.rssi_dbm,
                    bytes_tx = n.bytes_tx,
                    bytes_rx = n.bytes_rx,
                    "net.status"
                );
                state.update_net(n).await;
            }
            Err(e) => warn!("net.status decode: {e}"),
        },
        (class::NET, op::net::DOWNLINK) => {
            // Forwarded MQTT downlink; command path lives in PR #6.
            trace!("net.downlink (unhandled in this PR)");
        }
        (class::NET, op::net::TIME_SYNC) => {
            trace!("net.time_sync (unhandled in this PR)");
        }

        // ---- HOST (commands directed to this RPi agent) ----
        (class::HOST, op::host::SHUTDOWN) if flags & flag::REQ != 0 => {
            commands.handle_host_shutdown(&frame, outbound).await;
        }
        (class::HOST, op::host::RESET) if flags & flag::REQ != 0 => {
            commands.handle_host_reset(&frame, outbound).await;
        }
        (class::HOST, op::host::SERVICE_RESTART) if flags & flag::REQ != 0 => {
            commands.handle_host_service_restart(&frame, outbound).await;
        }
        (class::HOST, op::host::EVENT) => match HostEventV1::decode(&frame.payload) {
            Ok(e) => debug!(event = e.event, src = frame.src, "host.event echoed back"),
            Err(err) => warn!("host.event decode: {err}"),
        },

        _ => {
            trace!(
                src = frame.src,
                dst = frame.dst,
                class = cls,
                op = opc,
                flags = flags,
                payload_len = frame.payload.len(),
                "unhandled frame"
            );
        }
    }

    // Drop NULL/unknown DST silently — the RP2040 hub is the authoritative router.
    let _ = (addr::NULL,);
}
