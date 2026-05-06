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

    pub async fn handle_host_service_restart(
        &self,
        req: &Frame,
        out_tx: &mpsc::Sender<OutboundFrame>,
    ) {
        let payload = match HostServiceRestartV1::decode(&req.payload) {
            Ok(p) => p,
            Err(e) => {
                warn!(src = req.src, "host.service_restart decode: {e}");
                send_resp(req, out_tx).await;
                return;
            }
        };
        let unit = match std::str::from_utf8(&payload.unit) {
            Ok(s) => s.to_string(),
            Err(_) => {
                warn!(src = req.src, "host.service_restart: unit name not UTF-8");
                send_resp(req, out_tx).await;
                return;
            }
        };

        if !self.commands_cfg.allow_service_restart {
            warn!(unit = %unit, "host.service_restart denied: kill switch is off");
            send_resp(req, out_tx).await;
            return;
        }
        if !self.whitelist.contains(&unit) {
            warn!(unit = %unit, "host.service_restart denied: not in whitelist");
            send_resp(req, out_tx).await;
            return;
        }

        // We append `.service` for systemd; whitelist entries are stored
        // without the suffix so they match what operators type in the cloud UI.
        let unit_with_suffix = format!("{unit}.service");
        info!(unit = %unit, "host.service_restart executing systemctl restart");
        if let Err(e) = Command::new("systemctl")
            .args(["restart", &unit_with_suffix])
            .spawn()
        {
            error!(unit = %unit, "systemctl restart spawn: {e}");
        }
        send_resp(req, out_tx).await;
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
