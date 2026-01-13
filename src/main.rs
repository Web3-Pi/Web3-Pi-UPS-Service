mod config;
mod daemon;
mod ipc;
mod monitor;
mod status;
mod ups_data;

use anyhow::Result;
use config::{load_config, load_config_silent, setup_logging, DEFAULT_CONFIG_PATH};
use log::info;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, PartialEq)]
enum Command {
    Daemon,
    Status,
    Monitor,
    Help,
    Version,
}

fn print_help() {
    println!("w3p-ups v{} - Web3 Pi UPS Monitoring Service", VERSION);
    println!();
    println!("USAGE:");
    println!("    w3p-ups [OPTIONS] [COMMAND]");
    println!();
    println!("COMMANDS:");
    println!("    status     Display current UPS status and exit");
    println!("    monitor    Live TUI monitor (press 'q' to exit)");
    println!("    daemon     Run as background monitoring daemon (default)");
    println!();
    println!("OPTIONS:");
    println!(
        "    -c, --config <PATH>    Path to config file (default: {})",
        DEFAULT_CONFIG_PATH
    );
    println!("    -v, --version          Print version information");
    println!("    -h, --help             Print this help message");
    println!("    --status               Alias for 'status' command");
    println!("    --monitor              Alias for 'monitor' command");
    println!();
    println!("DESCRIPTION:");
    println!("    Monitors UPS battery status via serial port and initiates graceful");
    println!("    shutdown when battery is low and running on battery power.");
    println!();
    println!("    The 'status' and 'monitor' commands connect to the running daemon");
    println!("    via Unix socket to retrieve UPS data. Ensure the daemon is running:");
    println!("        sudo systemctl start w3p-ups");
    println!();
    println!("CONFIG:");
    println!("    Default config location: {}", DEFAULT_CONFIG_PATH);
    println!();
    println!("SIGNALS:");
    println!("    SIGTERM, SIGINT - Graceful shutdown of the service");
}

fn parse_args() -> (Command, String) {
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = DEFAULT_CONFIG_PATH.to_string();
    let mut command = Command::Daemon;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            // Commands (subcommands)
            "status" => command = Command::Status,
            "monitor" => command = Command::Monitor,
            "daemon" => command = Command::Daemon,

            // Flags
            "-h" | "--help" => command = Command::Help,
            "-v" | "--version" => command = Command::Version,
            "--status" => command = Command::Status,
            "--monitor" => command = Command::Monitor,

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

    (command, config_path)
}

fn main() -> Result<()> {
    let (command, config_path) = parse_args();

    // Handle help and version early
    match command {
        Command::Help => {
            print_help();
            return Ok(());
        }
        Command::Version => {
            println!("w3p-ups v{}", VERSION);
            return Ok(());
        }
        _ => {}
    }

    // For status and monitor, we don't need full logging setup
    // and we use a silent config loader that doesn't log warnings
    match command {
        Command::Status => {
            let config = load_config_silent(&config_path);
            return status::run_status(&config);
        }
        Command::Monitor => {
            let config = load_config_silent(&config_path);
            return monitor::run_monitor(&config);
        }
        Command::Daemon => {
            // Full daemon mode with logging
            let config = load_config(&config_path)?;
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
                match daemon::run_daemon(&config, running.clone()) {
                    Ok(()) => break,
                    Err(e) => {
                        if !running.load(Ordering::SeqCst) {
                            break;
                        }
                        log::error!("Monitoring error: {}. Retrying in 5 seconds...", e);
                        thread::sleep(std::time::Duration::from_secs(5));
                    }
                }
            }

            info!("w3p-ups stopped");
        }
        _ => unreachable!(),
    }

    Ok(())
}
