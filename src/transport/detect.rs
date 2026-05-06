use std::fs;
use std::path::Path;

use anyhow::{anyhow, Result};
use tracing::{debug, info, warn};

/// Resolve a configured serial port: either an explicit path or "auto".
pub fn resolve_port(configured: &str) -> Result<String> {
    if configured == "auto" {
        info!("auto-detecting Web3_Pi_UPS device...");
        detect_ups_port()
            .ok_or_else(|| anyhow!("Web3_Pi_UPS device not found. Check USB connection."))
    } else {
        Ok(configured.to_string())
    }
}

/// Find the UPS serial port via sysfs.
///
/// Priority:
///   1. USB product == "Web3_Pi_UPS" (production firmware)
///   2. USB product contains "Pico" (legacy bring-up firmware)
///   3. First available `/dev/ttyACM*` (last-ditch fallback)
pub fn detect_ups_port() -> Option<String> {
    let tty_class = Path::new("/sys/class/tty");

    let mut web3_pi_ups: Option<String> = None;
    let mut raspberry_pi_pico: Option<String> = None;
    let mut first_ttyacm: Option<String> = None;

    let entries = fs::read_dir(tty_class).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if !name_str.starts_with("ttyACM") {
            continue;
        }

        let device_path = format!("/dev/{name_str}");

        if first_ttyacm.is_none() {
            first_ttyacm = Some(device_path.clone());
        }

        let product_path = entry.path().join("device/../product");
        if let Ok(product) = fs::read_to_string(&product_path) {
            let product = product.trim();
            if product == "Web3_Pi_UPS" {
                web3_pi_ups = Some(device_path.clone());
                debug!("found Web3_Pi_UPS at {device_path}");
            } else if product.contains("Pico") && raspberry_pi_pico.is_none() {
                raspberry_pi_pico = Some(device_path.clone());
                debug!("found Raspberry Pi Pico at {device_path} (legacy firmware candidate)");
            }
        }
    }

    if let Some(port) = web3_pi_ups {
        info!("auto-detected Web3_Pi_UPS at {port}");
        return Some(port);
    }
    if let Some(port) = raspberry_pi_pico {
        warn!("Web3_Pi_UPS not found, using Raspberry Pi Pico at {port} (legacy firmware)");
        return Some(port);
    }
    if let Some(port) = first_ttyacm {
        warn!("no known UPS device found, falling back to first ttyACM: {port}");
        return Some(port);
    }
    None
}
