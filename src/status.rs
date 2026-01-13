use crate::config::Config;
use crate::ipc::{connect_to_daemon, read_ups_data};
use crate::ups_data::UpsData;
use anyhow::Result;

pub fn run_status(config: &Config) -> Result<()> {
    let mut reader = connect_to_daemon(&config.ipc.socket_path)?;
    let ups = read_ups_data(&mut reader)?;
    print_status_report(&ups, config.battery.min_valid_voltage);
    Ok(())
}

fn print_status_report(ups: &UpsData, min_valid_voltage: u32) {
    let on_battery = ups.is_on_battery(min_valid_voltage);
    let power_source = if on_battery { "Battery" } else { "Grid/USB-C" };

    println!("=== Web3 Pi UPS Status ===");
    println!();
    println!("Battery:");
    println!("  State of Charge: {}%", ups.soc);
    println!("  Voltage:         {:.2} V", ups.battery_voltage_v());
    println!("  Current:         {} mA", ups.ba);
    println!("  Charging:        {}", ups.charging_state_str());
    println!();
    println!("Power:");
    println!("  Source:          {}", power_source);
    println!("  Input Voltage:   {:.2} V", ups.input_voltage_v());
    println!(
        "  Power Good:      {}",
        if ups.pg == 1 { "Yes" } else { "No" }
    );
    println!();
    println!("System:");
    println!("  Temperature:     {:.1} °C", ups.temperature_c());
    println!("  Uptime:          {} sec", ups.up);
}
