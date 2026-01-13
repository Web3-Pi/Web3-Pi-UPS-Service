use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use serde::Deserialize;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use simplelog::{
    CombinedLogger, Config as LogConfig, LevelFilter, SharedLogger, TermLogger, TerminalMode,
    WriteLogger,
};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_CONFIG_PATH: &str = "/etc/w3p-ups/config.toml";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize, Debug)]
struct Config {
    serial: SerialConfig,
    battery: BatteryConfig,
    shutdown: ShutdownConfig,
    logging: LoggingConfig,
}

#[derive(Deserialize, Debug)]
struct SerialConfig {
    port: String,
    baud_rate: u32,
}

#[derive(Deserialize, Debug)]
struct BatteryConfig {
    shutdown_threshold: u8,
    min_valid_voltage: u32,
}

#[derive(Deserialize, Debug)]
struct ShutdownConfig {
    script_path: String,
    delay_seconds: u64,
}

#[derive(Deserialize, Debug)]
struct LoggingConfig {
    level: String,
    file_path: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]  // Fields parsed from JSON may be used for logging/debugging
struct UpsData {
    #[serde(default)]
    up: u32,        // uptime
    #[serde(default)]
    pd: u8,         // PD status
    #[serde(default)]
    pdo: u8,        // PDO
    #[serde(default)]
    cc: u8,         // CC line
    #[serde(default)]
    t: u32,         // temperature
    #[serde(default)]
    vs: u32,        // voltage source
    #[serde(default)]
    is: u32,        // current source
    #[serde(default)]
    vr: u32,        // voltage rail
    #[serde(default)]
    ir: u32,        // current rail
    soc: u8,        // State of Charge (battery %)
    #[serde(default)]
    bv: u32,        // battery voltage
    #[serde(default)]
    ba: i32,        // battery current (can be negative when discharging)
    #[serde(default)]
    cs: u8,         // charging state
    #[serde(default)]
    pg: u8,         // power good
    vi: u32,        // input voltage (8000-21000 = grid power OK)
    #[serde(default)]
    ii: u32,        // input current
    #[serde(default)]
    ci: u32,        // charge current
    #[serde(default)]
    cf: u8,         // charge flag
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
        }
    }
}

fn load_config(path: &str) -> Result<Config> {
    if Path::new(path).exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path))
    } else {
        warn!("Config file not found at {}, using defaults", path);
        Ok(Config::default())
    }
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

fn setup_logging(config: &LoggingConfig) -> Result<()> {
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

fn execute_shutdown_script(script_path: &str) -> Result<()> {
    info!("Executing shutdown script: {}", script_path);

    if !Path::new(script_path).exists() {
        error!("Shutdown script not found: {}", script_path);
        // Fallback to direct shutdown command
        warn!("Falling back to direct shutdown command");
        Command::new("shutdown")
            .args(["-h", "now"])
            .spawn()
            .context("Failed to execute shutdown command")?;
        return Ok(());
    }

    Command::new("sh")
        .arg(script_path)
        .spawn()
        .with_context(|| format!("Failed to execute shutdown script: {}", script_path))?;

    Ok(())
}

fn is_on_battery(vi: u32, min_valid_voltage: u32) -> bool {
    vi < min_valid_voltage
}

fn should_shutdown(ups_data: &UpsData, config: &BatteryConfig) -> bool {
    let low_soc = ups_data.soc < config.shutdown_threshold;
    let on_battery = is_on_battery(ups_data.vi, config.min_valid_voltage);
    low_soc && on_battery
}

fn run_monitoring_loop(config: &Config, running: Arc<AtomicBool>) -> Result<()> {
    info!("Opening serial port: {} at {} baud", config.serial.port, config.serial.baud_rate);

    let port = serialport::new(&config.serial.port, config.serial.baud_rate)
        .timeout(Duration::from_secs(10))
        .open()
        .with_context(|| format!("Failed to open serial port: {}", config.serial.port))?;

    let mut reader = BufReader::new(port);
    let mut line = String::new();
    let mut shutdown_timer: Option<Instant> = None;
    let mut last_log_time = Instant::now();
    let log_interval = Duration::from_secs(60); // Log status every 60 seconds

    info!("Monitoring started. Shutdown threshold: {}% SOC when on battery (vi < {})",
          config.battery.shutdown_threshold,
          config.battery.min_valid_voltage);

    while running.load(Ordering::SeqCst) {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                warn!("Serial port EOF, retrying...");
                thread::sleep(Duration::from_secs(1));
                continue;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match serde_json::from_str::<UpsData>(trimmed) {
                    Ok(ups_data) => {
                        let on_battery = is_on_battery(ups_data.vi, config.battery.min_valid_voltage);
                        let power_status = if on_battery { "BATTERY" } else { "GRID" };

                        // Periodic status logging
                        if last_log_time.elapsed() >= log_interval {
                            info!(
                                "Status: SOC={}%, VI={}mV ({}), BV={}mV, BA={}mA",
                                ups_data.soc, ups_data.vi, power_status, ups_data.bv, ups_data.ba
                            );
                            last_log_time = Instant::now();
                        }

                        debug!(
                            "UPS: SOC={}%, VI={}mV, power={}",
                            ups_data.soc, ups_data.vi, power_status
                        );

                        if should_shutdown(&ups_data, &config.battery) {
                            match shutdown_timer {
                                None => {
                                    warn!(
                                        "Low battery detected! SOC={}%, on battery power. \
                                         Shutdown in {} seconds unless power restored.",
                                        ups_data.soc, config.shutdown.delay_seconds
                                    );
                                    shutdown_timer = Some(Instant::now());
                                }
                                Some(start_time) => {
                                    let elapsed = start_time.elapsed().as_secs();
                                    if elapsed >= config.shutdown.delay_seconds {
                                        warn!(
                                            "Shutdown delay elapsed. Initiating shutdown... \
                                             (SOC={}%, VI={}mV)",
                                            ups_data.soc, ups_data.vi
                                        );
                                        execute_shutdown_script(&config.shutdown.script_path)?;
                                        return Ok(());
                                    } else {
                                        let remaining = config.shutdown.delay_seconds - elapsed;
                                        warn!(
                                            "Low battery! SOC={}%, shutdown in {} seconds",
                                            ups_data.soc, remaining
                                        );
                                    }
                                }
                            }
                        } else {
                            // Conditions no longer met, cancel shutdown timer
                            if shutdown_timer.is_some() {
                                info!(
                                    "Power restored or battery charged. Shutdown cancelled. \
                                     SOC={}%, VI={}mV",
                                    ups_data.soc, ups_data.vi
                                );
                                shutdown_timer = None;
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Failed to parse JSON '{}': {}", trimmed, e);
                    }
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    debug!("Serial read timeout, continuing...");
                    continue;
                }
                error!("Serial read error: {}", e);
                thread::sleep(Duration::from_secs(1));
            }
        }
    }

    info!("Monitoring stopped");
    Ok(())
}

fn print_help() {
    println!("w3p-ups v{} - Web3 Pi UPS Monitoring Service", VERSION);
    println!();
    println!("USAGE:");
    println!("    w3p-ups [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    -c, --config <PATH>    Path to config file (default: {})", DEFAULT_CONFIG_PATH);
    println!("    -v, --version          Print version information");
    println!("    -h, --help             Print this help message");
    println!();
    println!("DESCRIPTION:");
    println!("    Monitors UPS battery status via serial port and initiates graceful");
    println!("    shutdown when battery is low and running on battery power.");
    println!();
    println!("CONFIG:");
    println!("    Default config location: {}", DEFAULT_CONFIG_PATH);
    println!();
    println!("SIGNALS:");
    println!("    SIGTERM, SIGINT - Graceful shutdown of the service");
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = DEFAULT_CONFIG_PATH.to_string();

    // Simple argument parsing
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "-v" | "--version" => {
                println!("w3p-ups v{}", VERSION);
                return Ok(());
            }
            "-c" | "--config" => {
                if i + 1 < args.len() {
                    config_path = args[i + 1].clone();
                    i += 1;
                } else {
                    eprintln!("Error: --config requires a path argument");
                    std::process::exit(1);
                }
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                eprintln!("Use --help for usage information");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Load configuration
    let config = load_config(&config_path)?;

    // Setup logging
    setup_logging(&config.logging)?;

    info!("w3p-ups v{} starting", VERSION);
    info!("Config loaded from: {}", config_path);

    // Setup signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            info!("Received signal {:?}, shutting down...", sig);
            r.store(false, Ordering::SeqCst);
        }
    });

    // Run main monitoring loop with retry logic
    loop {
        match run_monitoring_loop(&config, running.clone()) {
            Ok(()) => break,
            Err(e) => {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                error!("Monitoring error: {}. Retrying in 5 seconds...", e);
                thread::sleep(Duration::from_secs(5));
            }
        }
    }

    info!("w3p-ups stopped");
    Ok(())
}
