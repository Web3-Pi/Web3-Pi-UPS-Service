//! Remote command handlers (AGENT-3): `host.shutdown`, `host.reset`,
//! `host.service_restart`. Whitelist enforcement + master kill switch from
//! `[commands]` config.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::{CommandsConfig, ShutdownConfig};
use crate::proto::payloads::HostServiceRestartV1;
use crate::proto::{addr, flag, Frame};
use crate::state::State;
use crate::transport::OutboundFrame;

// host.service RESP result codes (RESP payload[0]). 0 = ok; the panel surfaces
// any non-zero value as `code_N`, so a denied/failed action is no longer
// mis-reported as success by the empty-payload=success convention.
const RESP_OK: u8 = 0;
const RESP_BAD_REQUEST: u8 = 1; // malformed payload / non-UTF8 unit name
const RESP_DENIED: u8 = 2; // kill switch off, or unit not in whitelist
const RESP_SYSTEMCTL_FAILED: u8 = 3; // systemctl exited non-zero / failed to run

pub struct CommandsHandler {
    #[allow(dead_code)] // kept for future use (e.g., recording last command in state)
    state: Arc<State>,
    commands_cfg: CommandsConfig,
    shutdown_cfg: ShutdownConfig,
    whitelist: HashSet<String>,
}

impl CommandsHandler {
    pub fn new(
        state: Arc<State>,
        commands_cfg: CommandsConfig,
        shutdown_cfg: ShutdownConfig,
    ) -> Self {
        let whitelist: HashSet<String> = commands_cfg.service_whitelist.iter().cloned().collect();
        info!(
            allow_service_restart = commands_cfg.allow_service_restart,
            whitelist_size = whitelist.len(),
            "commands handler ready"
        );
        Self {
            state,
            commands_cfg,
            shutdown_cfg,
            whitelist,
        }
    }

    pub async fn handle_host_shutdown(&self, req: &Frame, out_tx: &mpsc::Sender<OutboundFrame>) {
        info!(src = req.src, seq = req.seq, "host.shutdown REQ");
        spawn_shutdown_script(&self.shutdown_cfg.script_path).await;
        send_resp(req, out_tx).await;
    }

    pub async fn handle_host_reset(&self, req: &Frame, out_tx: &mpsc::Sender<OutboundFrame>) {
        info!(src = req.src, seq = req.seq, "host.reset REQ");
        if let Err(e) = Command::new("shutdown").args(["-r", "now"]).spawn() {
            error!("spawn `shutdown -r now`: {e}");
        }
        send_resp(req, out_tx).await;
    }

    /// Handle a `host.service.{start,stop,restart}` REQ. `action` is the
    /// systemctl verb ("start" | "stop" | "restart"). All three share the
    /// `wups_host_service_restart_v1_hdr_t` payload (version + unit name), the
    /// `allow_service_restart` kill switch, and the unit whitelist.
    pub async fn handle_host_service_action(
        &self,
        req: &Frame,
        out_tx: &mpsc::Sender<OutboundFrame>,
        action: &str,
    ) {
        let payload = match HostServiceRestartV1::decode(&req.payload) {
            Ok(p) => p,
            Err(e) => {
                warn!(src = req.src, action, "host.service decode: {e}");
                send_resp_code(req, out_tx, RESP_BAD_REQUEST).await;
                return;
            }
        };
        let unit = match std::str::from_utf8(&payload.unit) {
            Ok(s) => s.to_string(),
            Err(_) => {
                warn!(src = req.src, action, "host.service: unit name not UTF-8");
                send_resp_code(req, out_tx, RESP_BAD_REQUEST).await;
                return;
            }
        };

        if !self.commands_cfg.allow_service_restart {
            warn!(unit = %unit, action, "host.service denied: kill switch is off");
            send_resp_code(req, out_tx, RESP_DENIED).await;
            return;
        }
        if !self.whitelist.contains(&unit) {
            warn!(unit = %unit, action, "host.service denied: not in whitelist");
            send_resp_code(req, out_tx, RESP_DENIED).await;
            return;
        }

        // We append `.service` for systemd; whitelist entries are stored
        // without the suffix so they match what operators type in the cloud UI.
        let unit_with_suffix = format!("{unit}.service");
        info!(unit = %unit, action, "host.service executing systemctl");
        // Await the exit status so the RESP reports the REAL outcome — a
        // fire-and-forget spawn() reports success even when systemctl failed
        // (wrong/unknown unit, etc.). The agent runs as root, so a non-zero
        // status is a genuine failure, surfaced to the panel as code_3.
        let code = match Command::new("systemctl")
            .args([action, &unit_with_suffix])
            .status()
            .await
        {
            Ok(s) if s.success() => RESP_OK,
            Ok(s) => {
                warn!(unit = %unit, action, exit = ?s.code(), "systemctl exited non-zero");
                RESP_SYSTEMCTL_FAILED
            }
            Err(e) => {
                error!(unit = %unit, action, "systemctl {action} failed to run: {e}");
                RESP_SYSTEMCTL_FAILED
            }
        };
        send_resp_code(req, out_tx, code).await;
    }
}

async fn spawn_shutdown_script(path: &str) {
    if Path::new(path).exists() {
        if let Err(e) = Command::new("sh").arg(path).spawn() {
            error!("spawn shutdown script {path}: {e}; falling back to shutdown -h now");
            let _ = Command::new("shutdown").args(["-h", "now"]).spawn();
        }
    } else {
        warn!("shutdown script {path} not found; running shutdown -h now");
        let _ = Command::new("shutdown").args(["-h", "now"]).spawn();
    }
}

async fn send_resp(req: &Frame, out_tx: &mpsc::Sender<OutboundFrame>) {
    let resp = Frame {
        dst: req.src,
        src: addr::RPI,
        class: req.class,
        op: req.op,
        flags: flag::RESP,
        seq: req.seq,
        payload: vec![],
    };
    if out_tx.send(OutboundFrame { frame: resp }).await.is_err() {
        warn!("send_resp: outbound closed");
    }
}

/// Like [`send_resp`] but carries a single result-code byte (0 = ok, non-zero =
/// failure; the panel surfaces non-zero as `code_N`). Used by host.service so a
/// denied/failed action isn't mis-scored as success by the empty-payload rule.
async fn send_resp_code(req: &Frame, out_tx: &mpsc::Sender<OutboundFrame>, code: u8) {
    let resp = Frame {
        dst: req.src,
        src: addr::RPI,
        class: req.class,
        op: req.op,
        flags: flag::RESP,
        seq: req.seq,
        payload: vec![code],
    };
    if out_tx.send(OutboundFrame { frame: resp }).await.is_err() {
        warn!("send_resp_code: outbound closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(allow: bool, list: &[&str]) -> CommandsConfig {
        CommandsConfig {
            allow_service_restart: allow,
            service_whitelist: list.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_handler(commands: CommandsConfig) -> CommandsHandler {
        let state = State::new();
        let shutdown = ShutdownConfig {
            script_path: "/nonexistent".into(),
            delay_seconds: 0,
        };
        CommandsHandler::new(state, commands, shutdown)
    }

    #[test]
    fn whitelist_built_from_config() {
        let h = make_handler(cfg(true, &["w3p_geth", "nimbus-validator"]));
        assert!(h.whitelist.contains("w3p_geth"));
        assert!(h.whitelist.contains("nimbus-validator"));
        assert!(!h.whitelist.contains("ssh"));
    }

    #[test]
    fn empty_whitelist_when_unconfigured() {
        let h = make_handler(cfg(true, &[]));
        assert!(h.whitelist.is_empty());
    }
}
