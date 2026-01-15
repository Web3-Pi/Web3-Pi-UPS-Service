use anyhow::{anyhow, Context, Result};
use log::{debug, error, info, trace, warn};
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

fn default_cancel_margin() -> u8 {
    5
}

#[derive(Deserialize, Debug)]
struct BatteryConfig {
    shutdown_threshold: u8,
    #[serde(default = "default_cancel_margin")]
    shutdown_cancel_margin: u8,
    min_valid_voltage: u32,
    max_valid_voltage: u32,
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
#[allow(dead_code)] // Fields parsed from JSON may be used for logging/debugging
struct UpsData {
    #[serde(default)]
    up: u32, // uptime
    #[serde(default)]
    pd: u8, // PD status
    #[serde(default)]
    pdo: u8, // PDO
    #[serde(default)]
    cc: u8, // CC line
    #[serde(default)]
    t: u32, // temperature
    #[serde(default)]
    vs: u32, // voltage source
    #[serde(default)]
    is: u32, // current source
    #[serde(default)]
    vr: u32, // voltage rail
    #[serde(default)]
    ir: u32, // current rail
    soc: u8, // State of Charge (battery %)
    #[serde(default)]
    sd: u8, // Shutdown decision battery % (used for shutdown logic)
    #[serde(default)]
    bv: u32, // battery voltage
    #[serde(default)]
    ba: i32, // battery current (can be negative when discharging)
    #[serde(default)]
    cs: u8, // charging state
    #[serde(default)]
    pg: u8, // power good
    vi: u32, // input voltage (8000-21000 = grid power OK)
    #[serde(default)]
    ii: u32, // input current
    #[serde(default)]
    ci: u32, // charge current
    #[serde(default)]
    cf: u8, // charge flag
}

impl Default for Config {
    fn default() -> Self {
        Config {
            serial: SerialConfig {
                port: "auto".to_string(),
                baud_rate: 115200,
            },
            battery: BatteryConfig {
                shutdown_threshold: 10,
                shutdown_cancel_margin: 5,
                min_valid_voltage: 8000,
                max_valid_voltage: 26000,
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
        toml::from_str(&content).with_context(|| format!("Failed to parse config file: {}", path))
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
    } else {
        Command::new("sh")
            .arg(script_path)
            .spawn()
            .with_context(|| format!("Failed to execute shutdown script: {}", script_path))?;
    }

    // Wait indefinitely for system to shut down
    // This prevents systemd from restarting the service before shutdown completes
    info!("Waiting for system shutdown...");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn is_on_battery(vi: u32, min_valid_voltage: u32, max_valid_voltage: u32) -> bool {
    vi < min_valid_voltage || vi > max_valid_voltage
}

/// Auto-detect Web3_Pi_UPS device by scanning sysfs for ttyACM devices
///
/// Detection priority:
/// 1. New firmware: product == "Web3_Pi_UPS"
/// 2. Legacy firmware: Raspberry Pi Pico (RP2040 with default USB descriptors)
/// 3. Fallback: First available ttyACM device
fn detect_ups_port() -> Option<String> {
    let tty_class = Path::new("/sys/class/tty");

    let mut web3_pi_ups: Option<String> = None;
    let mut raspberry_pi_pico: Option<String> = None;
    let mut first_ttyacm: Option<String> = None;

    if let Ok(entries) = fs::read_dir(tty_class) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Only check ttyACM devices (CDC-ACM USB serial)
            if !name_str.starts_with("ttyACM") {
                continue;
            }

            let device_path = format!("/dev/{}", name_str);

            // Track first ttyACM as last resort fallback
            if first_ttyacm.is_none() {
                first_ttyacm = Some(device_path.clone());
            }

            // Read product name from sysfs: /sys/class/tty/ttyACMx/device/../product
            let product_path = entry.path().join("device/../product");
            if let Ok(product) = fs::read_to_string(&product_path) {
                let product = product.trim();

                // Priority 1: New firmware with proper USB descriptors
                if product == "Web3_Pi_UPS" {
                    web3_pi_ups = Some(device_path.clone());
                    debug!("Found Web3_Pi_UPS at {}", device_path);
                }
                // Priority 2: Legacy firmware (RP2040 with default "Pico" product)
                else if product.contains("Pico") {
                    if raspberry_pi_pico.is_none() {
                        raspberry_pi_pico = Some(device_path.clone());
                        debug!("Found Raspberry Pi Pico at {} (legacy firmware candidate)", device_path);
                    }
                }
            }
        }
    }

    // Return by priority
    if let Some(port) = web3_pi_ups {
        info!("Auto-detected Web3_Pi_UPS at {}", port);
        return Some(port);
    }

    if let Some(port) = raspberry_pi_pico {
        warn!("Web3_Pi_UPS not found, using Raspberry Pi Pico at {} (legacy firmware)", port);
        return Some(port);
    }

    if let Some(port) = first_ttyacm {
        warn!("No known UPS device found, trying first ttyACM device: {}", port);
        return Some(port);
    }

    None
}

fn should_shutdown(ups_data: &UpsData, config: &BatteryConfig) -> bool {
    let low_battery = ups_data.sd < config.shutdown_threshold;
    let on_battery = is_on_battery(ups_data.vi, config.min_valid_voltage, config.max_valid_voltage);
    low_battery && on_battery
}

fn run_monitoring_loop(config: &Config, running: Arc<AtomicBool>) -> Result<()> {
    // Resolve port path - auto-detect or use configured value
    let port_path = if config.serial.port == "auto" {
        info!("Auto-detecting Web3_Pi_UPS device...");
        detect_ups_port().ok_or_else(|| {
            anyhow!("Web3_Pi_UPS device not found. Check USB connection.")
        })?
    } else {
        config.serial.port.clone()
    };

    info!(
        "Opening serial port: {} at {} baud",
        port_path, config.serial.baud_rate
    );

    let port = serialport::new(&port_path, config.serial.baud_rate)
        .timeout(Duration::from_secs(10))
        .open()
        .with_context(|| format!("Failed to open serial port: {}", port_path))?;

    let mut reader = BufReader::new(port);
    let mut line = String::new();
    let mut shutdown_timer: Option<Instant> = None;
    let mut last_log_time = Instant::now();
    let log_interval = Duration::from_secs(60); // Log status every 60 seconds

    info!(
        "Monitoring started. Shutdown at SD<{}%, cancel at SD>={}% (vi range {}-{}mV)",
        config.battery.shutdown_threshold,
        config.battery.shutdown_threshold + config.battery.shutdown_cancel_margin,
        config.battery.min_valid_voltage,
        config.battery.max_valid_voltage
    );

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

                // Log raw JSON at trace level
                trace!("RAW: {}", trimmed);

                match serde_json::from_str::<UpsData>(trimmed) {
                    Ok(ups_data) => {
                        let on_battery =
                            is_on_battery(ups_data.vi, config.battery.min_valid_voltage, config.battery.max_valid_voltage);
                        let power_status = if on_battery { "BATTERY" } else { "GRID" };

                        // Periodic status logging
                        if last_log_time.elapsed() >= log_interval {
                            info!(
                                "Status: SD={}%, SOC={}%, VI={}mV ({}), BV={}mV, BA={}mA",
                                ups_data.sd, ups_data.soc, ups_data.vi, power_status, ups_data.bv, ups_data.ba
                            );
                            last_log_time = Instant::now();
                        }

                        debug!(
                            "UPS: SD={}%, SOC={}%, VI={}mV, power={}",
                            ups_data.sd, ups_data.soc, ups_data.vi, power_status
                        );

                        if should_shutdown(&ups_data, &config.battery) {
                            match shutdown_timer {
                                None => {
                                    warn!(
                                        "Low battery detected! SD={}%, on battery power. \
                                         Shutdown in {} seconds unless power restored.",
                                        ups_data.sd, config.shutdown.delay_seconds
                                    );
                                    shutdown_timer = Some(Instant::now());
                                }
                                Some(start_time) => {
                                    let elapsed = start_time.elapsed().as_secs();
                                    if elapsed >= config.shutdown.delay_seconds {
                                        warn!(
                                            "Shutdown delay elapsed. Initiating shutdown... \
                                             (SD={}%, VI={}mV)",
                                            ups_data.sd, ups_data.vi
                                        );
                                        execute_shutdown_script(&config.shutdown.script_path)?;
                                        return Ok(());
                                    } else {
                                        let remaining = config.shutdown.delay_seconds - elapsed;
                                        warn!(
                                            "Low battery! SD={}%, shutdown in {} seconds",
                                            ups_data.sd, remaining
                                        );
                                    }
                                }
                            }
                        } else if shutdown_timer.is_some() {
                            // Hysteresis: only cancel if power restored OR battery significantly recovered
                            let cancel_threshold = config.battery.shutdown_threshold
                                .saturating_add(config.battery.shutdown_cancel_margin);
                            let battery_recovered = ups_data.sd >= cancel_threshold;
                            let power_restored = !on_battery;

                            if power_restored || battery_recovered {
                                info!(
                                    "Shutdown cancelled: {}. SD={}%, VI={}mV",
                                    if power_restored { "power restored" } else { "battery recovered" },
                                    ups_data.sd, ups_data.vi
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
    println!(
        "    -c, --config <PATH>    Path to config file (default: {})",
        DEFAULT_CONFIG_PATH
    );
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
