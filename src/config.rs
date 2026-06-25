use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/w3p-ups/config.toml";

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub serial: SerialConfig,
    pub battery: BatteryConfig,
    pub shutdown: ShutdownConfig,
    #[serde(default)]
    pub host_metrics: HostMetricsConfig,
    #[serde(default)]
    pub commands: CommandsConfig,
    #[serde(default)]
    pub eth_clients: EthClientsConfig,
    #[serde(default)]
    pub ipc: IpcConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SerialConfig {
    /// "auto" to auto-detect, or a path like "/dev/ttyACM0".
    pub port: String,
    pub baud_rate: u32,
}

#[derive(Deserialize, Debug, Clone)]
// Tolerate extra keys here so a v2.0.0 config (which had
// `voltage_at_zero_pct` / `voltage_at_full_pct`) keeps parsing on v2.0.1.
// The SOC curve is now a hardcoded LUT (see [`crate::soc`]) — those config
// knobs would be misleading if kept.
pub struct BatteryConfig {
    /// SOC% below which shutdown is initiated (when on battery).
    pub shutdown_threshold_pct: u8,
    /// Margin above threshold required to cancel a pending shutdown.
    #[serde(default = "default_cancel_margin")]
    pub shutdown_cancel_margin_pct: u8,
    /// Input (PD) voltage range considered "on grid". Outside this → on battery.
    pub input_min_valid_mv: u16,
    pub input_max_valid_mv: u16,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ShutdownConfig {
    pub script_path: String,
    pub delay_seconds: u64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct HostMetricsConfig {
    /// Period between host.status emissions to RP2040 (s). 0 disables.
    pub interval_seconds: u64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct CommandsConfig {
    /// Master kill switch for `host.service.restart` REQs.
    pub allow_service_restart: bool,
    /// Whitelist of systemd unit names (without `.service`) allowed to be restarted.
    pub service_whitelist: Vec<String>,
}

/// systemd unit names for the three Ethereum-client roles the agent monitors.
/// We report the unit's *service* state (running/stopped/failed) only — never
/// chain sync status. An empty string disables monitoring for that role.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct EthClientsConfig {
    /// Execution-layer client unit (e.g. `geth`, `reth`, `besu`).
    pub execution: String,
    /// Consensus-layer / beacon client unit (e.g. `nimbus-beacon-node`, `lighthouse-beacon`).
    pub consensus: String,
    /// Validator client unit (e.g. `nimbus-validator`).
    pub validator: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct IpcConfig {
    pub socket_path: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    /// trace | debug | info | warn | error
    pub level: String,
    /// When true, emit logs through journald (in addition to stderr).
    pub journald: bool,
}

fn default_cancel_margin() -> u8 {
    5
}

impl Default for HostMetricsConfig {
    fn default() -> Self {
        // 30 s, sized against the M.2 modem's ~500 MB/mo LTE data plan
        // (measured per-frame packet size). 0 disables. Host metrics only
        // reach the backend when this service is running on the Pi — if the
        // Pi is not connected over USB, no host.status frames are produced
        // and the RP2040 has nothing to relay.
        Self {
            interval_seconds: 30,
        }
    }
}

impl Default for CommandsConfig {
    fn default() -> Self {
        Self {
            allow_service_restart: true,
            service_whitelist: vec![
                "w3p_geth".into(),
                "w3p_nimbus-beacon".into(),
                "w3p_lighthouse-beacon".into(),
                "nimbus-validator".into(),
            ],
        }
    }
}

impl Default for EthClientsConfig {
    fn default() -> Self {
        // Stock Web3-Pi-vOS units (geth + nimbus). Override per-deployment for
        // other client combos (reth/besu, lighthouse/teku, …). `systemctl
        // is-active` resolves the `.service` suffix, so bare names are fine.
        Self {
            execution: "geth".into(),
            consensus: "nimbus-beacon-node".into(),
            validator: "nimbus-validator".into(),
        }
    }
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            socket_path: "/run/w3p-ups/agent.sock".into(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            journald: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            serial: SerialConfig {
                port: "auto".into(),
                baud_rate: 115200,
            },
            battery: BatteryConfig {
                shutdown_threshold_pct: 10,
                shutdown_cancel_margin_pct: 5,
                input_min_valid_mv: 8000,
                input_max_valid_mv: 26000,
            },
            shutdown: ShutdownConfig {
                script_path: "/etc/w3p-ups/shutdown.sh".into(),
                delay_seconds: 30,
            },
            host_metrics: HostMetricsConfig::default(),
            commands: CommandsConfig::default(),
            eth_clients: EthClientsConfig::default(),
            ipc: IpcConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

pub fn load(path: &str) -> Result<Config> {
    if Path::new(path).exists() {
        let content = fs::read_to_string(path).with_context(|| format!("read config: {path}"))?;
        toml::from_str(&content).with_context(|| format!("parse config: {path}"))
    } else {
        // Return defaults; caller logs the situation.
        Ok(Config::default())
    }
}
