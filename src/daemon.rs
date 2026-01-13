use crate::config::Config;
use crate::ipc::IpcServer;
use crate::ups_data::{is_on_battery, should_shutdown, UpsData};
use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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

pub fn run_daemon(config: &Config, running: Arc<AtomicBool>) -> Result<()> {
    info!(
        "Opening serial port: {} at {} baud",
        config.serial.port, config.serial.baud_rate
    );

    let port = serialport::new(&config.serial.port, config.serial.baud_rate)
        .timeout(Duration::from_secs(10))
        .open()
        .with_context(|| format!("Failed to open serial port: {}", config.serial.port))?;

    let mut reader = BufReader::new(port);

    // Initialize IPC server
    info!("Starting IPC server on: {}", config.ipc.socket_path);
    let mut ipc = IpcServer::new(&config.ipc.socket_path)?;

    let mut line = String::new();
    let mut shutdown_timer: Option<Instant> = None;
    let mut last_log_time = Instant::now();
    let log_interval = Duration::from_secs(60);

    info!(
        "Monitoring started. Shutdown threshold: {}% SOC when on battery (vi < {})",
        config.battery.shutdown_threshold, config.battery.min_valid_voltage
    );

    while running.load(Ordering::SeqCst) {
        // Accept new IPC clients
        ipc.accept_clients();

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
                        // Broadcast to IPC clients
                        ipc.broadcast(&ups_data);

                        let on_battery =
                            is_on_battery(ups_data.vi, config.battery.min_valid_voltage);
                        let power_status = if on_battery { "BATTERY" } else { "GRID" };

                        // Periodic status logging
                        if last_log_time.elapsed() >= log_interval {
                            info!(
                                "Status: SOC={}%, VI={}mV ({}), BV={}mV, BA={}mA, clients={}",
                                ups_data.soc,
                                ups_data.vi,
                                power_status,
                                ups_data.bv,
                                ups_data.ba,
                                ipc.client_count()
                            );
                            last_log_time = Instant::now();
                        }

                        debug!(
                            "UPS: SOC={}%, VI={}mV, power={}",
                            ups_data.soc, ups_data.vi, power_status
                        );

                        if should_shutdown(
                            &ups_data,
                            config.battery.shutdown_threshold,
                            config.battery.min_valid_voltage,
                        ) {
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
