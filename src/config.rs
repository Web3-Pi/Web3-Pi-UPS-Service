use anyhow::{Context, Result};
use log::{warn, LevelFilter};
use serde::Deserialize;
use simplelog::{
    CombinedLogger, Config as LogConfig, SharedLogger, TermLogger, TerminalMode, WriteLogger,
};
use std::fs::{self, File};
use std::path::Path;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/w3p-ups/config.toml";
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/w3p-ups/ups.sock";

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub serial: SerialConfig,
    pub battery: BatteryConfig,
    pub shutdown: ShutdownConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub ipc: IpcConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SerialConfig {
    pub port: String,
    pub baud_rate: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BatteryConfig {
    pub shutdown_threshold: u8,
    pub min_valid_voltage: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ShutdownConfig {
    pub script_path: String,
    pub delay_seconds: u64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LoggingConfig {
    pub level: String,
    pub file_path: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct IpcConfig {
    pub socket_path: String,
}

impl Default for IpcConfig {
    fn default() -> Self {
        IpcConfig {
            socket_path: DEFAULT_SOCKET_PATH.to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            serial: SerialConfig {
                port: "/dev/ttyACM0".to_string(),
                baud_rate: 115200,
            },
            battery: BatteryConfig {
                shutdown_threshold: 10,
                min_valid_voltage: 8000,
            },
            shutdown: ShutdownConfig {
                script_path: "/etc/w3p-ups/shutdown.sh".to_string(),
                delay_seconds: 30,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                file_path: None,
            },
            ipc: IpcConfig::default(),
        }
    }
}

pub fn load_config(path: &str) -> Result<Config> {
    if Path::new(path).exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;
        toml::from_str(&content).with_context(|| format!("Failed to parse config file: {}", path))
    } else {
        warn!("Config file not found at {}, using defaults", path);
        Ok(Config::default())
    }
}

pub fn load_config_silent(path: &str) -> Config {
    if Path::new(path).exists() {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(config) = toml::from_str(&content) {
                return config;
            }
        }
    }
    Config::default()
}

fn parse_log_level(level: &str) -> LevelFilter {
    match level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    }
}

pub fn setup_logging(config: &LoggingConfig) -> Result<()> {
    let level = parse_log_level(&config.level);
    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![];

    // Terminal/journald logger
    loggers.push(TermLogger::new(
        level,
        LogConfig::default(),
        TerminalMode::Mixed,
        simplelog::ColorChoice::Auto,
    ));

    // File logger (optional)
    if let Some(ref file_path) = config.file_path {
        if !file_path.is_empty() {
            // Ensure parent directory exists
            if let Some(parent) = Path::new(file_path).parent() {
                fs::create_dir_all(parent)?;
            }
            let file = File::create(file_path)
                .with_context(|| format!("Failed to create log file: {}", file_path))?;
            loggers.push(WriteLogger::new(level, LogConfig::default(), file));
        }
    }

    CombinedLogger::init(loggers).context("Failed to initialize logger")?;
    Ok(())
}
