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
#[serde(deny_unknown_fields)]
pub struct BatteryConfig {
    /// SOC% below which shutdown is initiated (when on battery).
    pub shutdown_threshold_pct: u8,
    /// Margin above threshold required to cancel a pending shutdown.
    #[serde(default = "default_cancel_margin")]
    pub shutdown_cancel_margin_pct: u8,
    /// Battery voltage at 0% SOC (mV). Linear interpolation up to full.
    pub voltage_at_zero_pct: u16,
    /// Battery voltage at 100% SOC (mV).
    pub voltage_at_full_pct: u16,
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
        Self {
            interval_seconds: 5,
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
                voltage_at_zero_pct: 6000,
                voltage_at_full_pct: 8400,
                input_min_valid_mv: 8000,
                input_max_valid_mv: 26000,
            },
            shutdown: ShutdownConfig {
                script_path: "/etc/w3p-ups/shutdown.sh".into(),
                delay_seconds: 30,
            },
            host_metrics: HostMetricsConfig::default(),
            commands: CommandsConfig::default(),
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
