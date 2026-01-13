use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[allow(dead_code)]
pub struct UpsData {
    #[serde(default)]
    pub up: u32, // uptime (seconds)
    #[serde(default)]
    pub pd: u8, // PD status
    #[serde(default)]
    pub pdo: u8, // PDO
    #[serde(default)]
    pub cc: u8, // CC line
    #[serde(default)]
    pub t: u32, // temperature (in 0.1°C units)
    #[serde(default)]
    pub vs: u32, // voltage source
    #[serde(default)]
    pub is: u32, // current source
    #[serde(default)]
    pub vr: u32, // voltage rail
    #[serde(default)]
    pub ir: u32, // current rail
    pub soc: u8, // State of Charge (battery %)
    #[serde(default)]
    pub bv: u32, // battery voltage (mV)
    #[serde(default)]
    pub ba: i32, // battery current (mA, negative when discharging)
    #[serde(default)]
    pub cs: u8, // charging state
    #[serde(default)]
    pub pg: u8, // power good
    pub vi: u32, // input voltage (mV, 8000-21000 = grid power OK)
    #[serde(default)]
    pub ii: u32, // input current (mA)
    #[serde(default)]
    pub ci: u32, // charge current (mA)
    #[serde(default)]
    pub cf: u8, // charge flag
}

impl UpsData {
    pub fn is_on_battery(&self, min_valid_voltage: u32) -> bool {
        self.vi < min_valid_voltage
    }

    pub fn charging_state_str(&self) -> &'static str {
        match self.cs {
            0 => "Not charging",
            1 => "Pre-charge",
            2 => "Charging",
            3 => "Charge complete",
            _ => "Unknown",
        }
    }

    pub fn battery_voltage_v(&self) -> f32 {
        self.bv as f32 / 1000.0
    }

    pub fn input_voltage_v(&self) -> f32 {
        self.vi as f32 / 1000.0
    }

    pub fn temperature_c(&self) -> f32 {
        self.t as f32 / 10.0
    }
}

pub fn is_on_battery(vi: u32, min_valid_voltage: u32) -> bool {
    vi < min_valid_voltage
}

pub fn should_shutdown(ups_data: &UpsData, shutdown_threshold: u8, min_valid_voltage: u32) -> bool {
    let low_soc = ups_data.soc < shutdown_threshold;
    let on_battery = is_on_battery(ups_data.vi, min_valid_voltage);
    low_soc && on_battery
}
