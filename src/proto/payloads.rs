//! Payload structs for each (class, op).
//!
//! Each fixed-size payload has `WIRE_LEN`, `encode() -> [u8; WIRE_LEN]`, and
//! `decode(&[u8]) -> Result<Self, PayloadError>`. Variable-size payloads use
//! `encoded_len`, `encode_into`/`encode`, and `decode`.
//!
//! Mirrors structs in `Web3-Pi-UPS/common/protocol.h`.

use super::PROTO_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PayloadError {
    #[error("expected payload length {expected}, got {got}")]
    UnexpectedLength { expected: usize, got: usize },
    #[error("buffer too short: need at least {needed}, got {got}")]
    TooShort { needed: usize, got: usize },
    #[error("unsupported version: got {got}, expected {expected}")]
    UnsupportedVersion { got: u8, expected: u8 },
    #[error("invalid value: {0}")]
    InvalidValue(&'static str),
    #[error("declared inner length {declared} exceeds buffer remainder {remaining}")]
    LengthMismatch { declared: usize, remaining: usize },
}

fn check_version(b: u8) -> Result<(), PayloadError> {
    if b != PROTO_VERSION {
        Err(PayloadError::UnsupportedVersion {
            got: b,
            expected: PROTO_VERSION,
        })
    } else {
        Ok(())
    }
}

fn check_fixed_len(buf: &[u8], expected: usize) -> Result<(), PayloadError> {
    if buf.len() != expected {
        Err(PayloadError::UnexpectedLength {
            expected,
            got: buf.len(),
        })
    } else {
        Ok(())
    }
}

// ============ class 0x01 SYSTEM ============

/// `system.ping` RESP — `wups_sys_pong_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SysPongV1 {
    pub fw_version: u16,
    pub uptime_ms: u32,
}

impl SysPongV1 {
    pub const WIRE_LEN: usize = 8;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[2..4].copy_from_slice(&self.fw_version.to_le_bytes());
        out[4..8].copy_from_slice(&self.uptime_ms.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            fw_version: u16::from_le_bytes([buf[2], buf[3]]),
            uptime_ms: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        })
    }
}

/// `system.hello` EVENT — `wups_sys_hello_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SysHelloV1 {
    pub proto_version: u8,
    pub node_addr: u8,
    pub fw_version: u16,
    pub caps_classes: u16,
    pub build_id: u32,
}

impl SysHelloV1 {
    pub const WIRE_LEN: usize = 12;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[1] = self.proto_version;
        out[2] = self.node_addr;
        out[4..6].copy_from_slice(&self.fw_version.to_le_bytes());
        out[6..8].copy_from_slice(&self.caps_classes.to_le_bytes());
        out[8..12].copy_from_slice(&self.build_id.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            proto_version: buf[1],
            node_addr: buf[2],
            fw_version: u16::from_le_bytes([buf[4], buf[5]]),
            caps_classes: u16::from_le_bytes([buf[6], buf[7]]),
            build_id: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        })
    }
}

/// `system.log` EVENT — `wups_sys_log_v1_hdr_t` + ASCII text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SysLogV1 {
    pub level: u8, // 0=trace 1=debug 2=info 3=warn 4=error
    pub text: Vec<u8>,
}

impl SysLogV1 {
    pub const HEADER_LEN: usize = 4;

    pub fn encoded_len(&self) -> usize {
        Self::HEADER_LEN + self.text.len()
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), PayloadError> {
        if self.text.len() > u8::MAX as usize {
            return Err(PayloadError::InvalidValue("log text > 255 bytes"));
        }
        out.push(PROTO_VERSION);
        out.push(self.level);
        out.push(self.text.len() as u8);
        out.push(0);
        out.extend_from_slice(&self.text);
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, PayloadError> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        if buf.len() < Self::HEADER_LEN {
            return Err(PayloadError::TooShort {
                needed: Self::HEADER_LEN,
                got: buf.len(),
            });
        }
        check_version(buf[0])?;
        let level = buf[1];
        let text_len = buf[2] as usize;
        let remaining = buf.len() - Self::HEADER_LEN;
        if text_len > remaining {
            return Err(PayloadError::LengthMismatch {
                declared: text_len,
                remaining,
            });
        }
        Ok(Self {
            level,
            text: buf[Self::HEADER_LEN..Self::HEADER_LEN + text_len].to_vec(),
        })
    }
}

// ============ class 0x02 POWER ============

/// `power.status` — `wups_power_status_v1_t`. 20 bytes on wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerStatusV1 {
    pub charge_state: u8, // 0=idle 1=charging 2=charged 3=fault
    pub vbus_in_mv: u16,
    pub vbus_out_mv: u16,
    pub ibus_out_ma: i16,
    pub vbat_mv: u16,
    pub ibat_ma: i16,
    pub temp_dc: i16,
    pub pd_contract_mv: u16,
    pub pd_contract_ma: u16,
    pub faults: u16,
}

impl PowerStatusV1 {
    pub const WIRE_LEN: usize = 20;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[1] = self.charge_state;
        out[2..4].copy_from_slice(&self.vbus_in_mv.to_le_bytes());
        out[4..6].copy_from_slice(&self.vbus_out_mv.to_le_bytes());
        out[6..8].copy_from_slice(&self.ibus_out_ma.to_le_bytes());
        out[8..10].copy_from_slice(&self.vbat_mv.to_le_bytes());
        out[10..12].copy_from_slice(&self.ibat_ma.to_le_bytes());
        out[12..14].copy_from_slice(&self.temp_dc.to_le_bytes());
        out[14..16].copy_from_slice(&self.pd_contract_mv.to_le_bytes());
        out[16..18].copy_from_slice(&self.pd_contract_ma.to_le_bytes());
        out[18..20].copy_from_slice(&self.faults.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            charge_state: buf[1],
            vbus_in_mv: u16::from_le_bytes([buf[2], buf[3]]),
            vbus_out_mv: u16::from_le_bytes([buf[4], buf[5]]),
            ibus_out_ma: i16::from_le_bytes([buf[6], buf[7]]),
            vbat_mv: u16::from_le_bytes([buf[8], buf[9]]),
            ibat_ma: i16::from_le_bytes([buf[10], buf[11]]),
            temp_dc: i16::from_le_bytes([buf[12], buf[13]]),
            pd_contract_mv: u16::from_le_bytes([buf[14], buf[15]]),
            pd_contract_ma: u16::from_le_bytes([buf[16], buf[17]]),
            faults: u16::from_le_bytes([buf[18], buf[19]]),
        })
    }
}

/// Bit positions for `PowerStatusV1::faults`.
pub mod power_fault {
    pub const OVP: u16 = 1 << 0;
    pub const OCP: u16 = 1 << 1;
    pub const OTP: u16 = 1 << 2;
    pub const PD_NEG: u16 = 1 << 3;
}

/// `power.status` v2 — `wups_power_status_v2_t`. 40 bytes, version byte = 2.
///
/// A redesign of v1 (not a prefix-compatible superset): the dispatcher picks
/// the decoder by the version byte. v2 separates INPUT (HUSB238) and OUTPUT
/// PD contracts, exposes real `vsys_mv`/`iin_ma` (v1 aliased these onto
/// `pd_contract_*`), splits the two temperatures, and packs booleans into
/// `flags`. The host currently down-converts v2 to `PowerStatusV1` for
/// storage (see `to_v1`); native v2 exposure is a later step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerStatusV2 {
    pub flags: u8,         // WUPS_PWR2_FLAG_*
    pub charge_state: u8,
    pub vbus_in_mv: u16,
    pub pd_in_mv: u16,     // HUSB238 negotiated input; 0 = N/A
    pub pd_in_ma: u16,
    pub vbus_out_mv: u16,  // PA0 ADC
    pub vout_set_mv: u16,
    pub vout_read_mv: u16,
    pub iout_limit_ma: u16, // current LIMIT, not a load measurement
    pub pd_out_mv: u16,    // output PD contract to the Pi; 0 = rail off
    pub pd_out_ma: u16,
    pub vbat_mv: u16,
    pub ichg_ma: i16,      // charge current; 0 on discharge
    pub vsys_mv: u16,
    pub iin_ma: u16,
    pub temp_lm_dc: i16,
    pub temp_mp_dc: i16,   // -32768 = MP2762A unpowered (N/A)
    pub faults: u16,
    pub uptime_s: u32,
}

/// `flags` bit positions for `PowerStatusV2`.
pub mod power2_flag {
    pub const DC_IN_EN: u8 = 1 << 0;
    pub const VBUS_OUT_EN: u8 = 1 << 1;
    pub const BATT_PRESENT: u8 = 1 << 2;
    pub const POWER_GOOD: u8 = 1 << 3;
    pub const USB_C_ATTACH: u8 = 1 << 4;
}

impl PowerStatusV2 {
    pub const WIRE_LEN: usize = 40;
    pub const VERSION: u8 = 2;

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        if buf[0] != Self::VERSION {
            return Err(PayloadError::UnsupportedVersion {
                got: buf[0],
                expected: Self::VERSION,
            });
        }
        let u16le = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
        let i16le = |i: usize| i16::from_le_bytes([buf[i], buf[i + 1]]);
        Ok(Self {
            flags: buf[1],
            charge_state: buf[2],
            // buf[3] = reserved
            vbus_in_mv: u16le(4),
            pd_in_mv: u16le(6),
            pd_in_ma: u16le(8),
            vbus_out_mv: u16le(10),
            vout_set_mv: u16le(12),
            vout_read_mv: u16le(14),
            iout_limit_ma: u16le(16),
            pd_out_mv: u16le(18),
            pd_out_ma: u16le(20),
            vbat_mv: u16le(22),
            ichg_ma: i16le(24),
            vsys_mv: u16le(26),
            iin_ma: u16le(28),
            temp_lm_dc: i16le(30),
            temp_mp_dc: i16le(32),
            faults: u16le(34),
            uptime_s: u32::from_le_bytes([buf[36], buf[37], buf[38], buf[39]]),
        })
    }

    /// Down-convert to the legacy `PowerStatusV1` the rest of the host still
    /// consumes. Preserves the v1 host's existing semantics: `pd_contract_*`
    /// carried VSYS/IIN, `vbus_out`/`ibus_out` carried the TPS readback/limit,
    /// `temp_dc` was the hotter of the two sensors.
    pub fn to_v1(&self) -> PowerStatusV1 {
        let temp_dc = if self.temp_mp_dc == i16::MIN {
            self.temp_lm_dc
        } else {
            self.temp_lm_dc.max(self.temp_mp_dc)
        };
        PowerStatusV1 {
            charge_state: self.charge_state,
            vbus_in_mv: self.vbus_in_mv,
            vbus_out_mv: self.vout_read_mv,
            ibus_out_ma: self.iout_limit_ma as i16,
            vbat_mv: self.vbat_mv,
            ibat_ma: self.ichg_ma,
            temp_dc,
            pd_contract_mv: self.vsys_mv,
            pd_contract_ma: self.iin_ma,
            faults: self.faults,
        }
    }
}

/// `power.cycle` REQ — `wups_power_cycle_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerCycleV1 {
    pub off_ms: u16,
}

impl PowerCycleV1 {
    pub const WIRE_LEN: usize = 4;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[2..4].copy_from_slice(&self.off_ms.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            off_ms: u16::from_le_bytes([buf[2], buf[3]]),
        })
    }
}

/// `power.event` BCAST — `wups_power_event_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerEventV1 {
    pub event: u8,
}

impl PowerEventV1 {
    pub const WIRE_LEN: usize = 2;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        [PROTO_VERSION, self.event]
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self { event: buf[1] })
    }
}

pub mod power_event {
    pub const MAINS_LOST: u8 = 1;
    pub const MAINS_RESTORED: u8 = 2;
    pub const CHARGE_LOW: u8 = 3;
    pub const CHARGE_FULL: u8 = 4;
    pub const FAULT: u8 = 5;
}

// ============ class 0x03 NET ============

/// `net.status` — `wups_net_status_v1_t`. 20 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetStatusV1 {
    pub state: u8, // 0=off 1=init 2=net_attach 3=ppp_up 4=mqtt_up 5=err
    pub rssi_dbm: i8,
    pub rsrp_dbm: i8,
    pub rsrq_db: i8,
    pub errors: u16,
    pub ip_addr: u32,
    pub bytes_tx: u32,
    pub bytes_rx: u32,
}

impl NetStatusV1 {
    pub const WIRE_LEN: usize = 20;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[1] = self.state;
        out[2] = self.rssi_dbm as u8;
        out[3] = self.rsrp_dbm as u8;
        out[4] = self.rsrq_db as u8;
        out[6..8].copy_from_slice(&self.errors.to_le_bytes());
        out[8..12].copy_from_slice(&self.ip_addr.to_le_bytes());
        out[12..16].copy_from_slice(&self.bytes_tx.to_le_bytes());
        out[16..20].copy_from_slice(&self.bytes_rx.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            state: buf[1],
            rssi_dbm: buf[2] as i8,
            rsrp_dbm: buf[3] as i8,
            rsrq_db: buf[4] as i8,
            errors: u16::from_le_bytes([buf[6], buf[7]]),
            ip_addr: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            bytes_tx: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            bytes_rx: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
        })
    }
}

/// `net.publish` REQ — header + topic + payload.
///
/// Wire: `[ver][qos][retain][topic_len][payload_len_lo][payload_len_hi][topic..][payload..]`.
/// `wups_net_downlink_v1_hdr_t` shares the same layout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetPublishV1 {
    pub qos: u8,
    pub retain: u8,
    pub topic: Vec<u8>,
    pub payload: Vec<u8>,
}

impl NetPublishV1 {
    pub const HEADER_LEN: usize = 6;
    /// Per protocol spec, topic length is capped at 200 bytes.
    pub const MAX_TOPIC_LEN: usize = 200;

    pub fn encoded_len(&self) -> usize {
        Self::HEADER_LEN + self.topic.len() + self.payload.len()
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), PayloadError> {
        if self.topic.len() > Self::MAX_TOPIC_LEN {
            return Err(PayloadError::InvalidValue("topic > 200 bytes"));
        }
        if self.payload.len() > u16::MAX as usize {
            return Err(PayloadError::InvalidValue("payload > 65535 bytes"));
        }
        out.push(PROTO_VERSION);
        out.push(self.qos);
        out.push(self.retain);
        out.push(self.topic.len() as u8);
        out.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.topic);
        out.extend_from_slice(&self.payload);
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, PayloadError> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        if buf.len() < Self::HEADER_LEN {
            return Err(PayloadError::TooShort {
                needed: Self::HEADER_LEN,
                got: buf.len(),
            });
        }
        check_version(buf[0])?;
        let qos = buf[1];
        let retain = buf[2];
        let topic_len = buf[3] as usize;
        let payload_len = u16::from_le_bytes([buf[4], buf[5]]) as usize;
        let total = Self::HEADER_LEN + topic_len + payload_len;
        if buf.len() < total {
            return Err(PayloadError::LengthMismatch {
                declared: topic_len + payload_len,
                remaining: buf.len() - Self::HEADER_LEN,
            });
        }
        let topic_end = Self::HEADER_LEN + topic_len;
        Ok(Self {
            qos,
            retain,
            topic: buf[Self::HEADER_LEN..topic_end].to_vec(),
            payload: buf[topic_end..topic_end + payload_len].to_vec(),
        })
    }
}

/// `net.time_sync` BCAST INTERNAL — `wups_net_time_sync_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetTimeSyncV1 {
    pub ms_frac: u16,
    pub unix_s: u32,
}

impl NetTimeSyncV1 {
    pub const WIRE_LEN: usize = 8;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[2..4].copy_from_slice(&self.ms_frac.to_le_bytes());
        out[4..8].copy_from_slice(&self.unix_s.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            ms_frac: u16::from_le_bytes([buf[2], buf[3]]),
            unix_s: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        })
    }
}

// ============ class 0x04 HOST ============

/// `host.status` — `wups_host_status_v1_t`. 12 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostStatusV1 {
    pub eth_client_state: u8, // 0=stopped 1=syncing 2=synced 3=error
    pub cpu_temp_dc: i16,
    pub mem_used_pct: u8,
    pub disk_used_pct: u8,
    pub load_avg_x100: u16, // 1-min load × 100
    pub uptime_s: u32,
}

impl HostStatusV1 {
    pub const WIRE_LEN: usize = 12;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[1] = self.eth_client_state;
        out[2..4].copy_from_slice(&self.cpu_temp_dc.to_le_bytes());
        out[4] = self.mem_used_pct;
        out[5] = self.disk_used_pct;
        out[6..8].copy_from_slice(&self.load_avg_x100.to_le_bytes());
        out[8..12].copy_from_slice(&self.uptime_s.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            eth_client_state: buf[1],
            cpu_temp_dc: i16::from_le_bytes([buf[2], buf[3]]),
            mem_used_pct: buf[4],
            disk_used_pct: buf[5],
            load_avg_x100: u16::from_le_bytes([buf[6], buf[7]]),
            uptime_s: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        })
    }
}

/// `host.shutdown` / `host.reset` REQ — `wups_host_shutdown_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostShutdownV1 {
    pub reason: u8, // 1=low_battery 2=remote_cmd 3=user 4=fault
    pub delay_s: u16,
}

impl HostShutdownV1 {
    pub const WIRE_LEN: usize = 4;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[1] = self.reason;
        out[2..4].copy_from_slice(&self.delay_s.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            reason: buf[1],
            delay_s: u16::from_le_bytes([buf[2], buf[3]]),
        })
    }
}

pub mod host_shutdown_reason {
    pub const LOW_BATTERY: u8 = 1;
    pub const REMOTE_CMD: u8 = 2;
    pub const USER: u8 = 3;
    pub const FAULT: u8 = 4;
}

/// `host.service_restart` REQ — `wups_host_service_restart_v1_hdr_t` + unit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostServiceRestartV1 {
    pub unit: Vec<u8>, // ASCII unit name without `.service` suffix
}

impl HostServiceRestartV1 {
    pub const HEADER_LEN: usize = 4;

    pub fn encoded_len(&self) -> usize {
        Self::HEADER_LEN + self.unit.len()
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), PayloadError> {
        if self.unit.len() > u8::MAX as usize {
            return Err(PayloadError::InvalidValue("unit name > 255 bytes"));
        }
        out.push(PROTO_VERSION);
        out.push(self.unit.len() as u8);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&self.unit);
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, PayloadError> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        if buf.len() < Self::HEADER_LEN {
            return Err(PayloadError::TooShort {
                needed: Self::HEADER_LEN,
                got: buf.len(),
            });
        }
        check_version(buf[0])?;
        let unit_len = buf[1] as usize;
        let remaining = buf.len() - Self::HEADER_LEN;
        if unit_len > remaining {
            return Err(PayloadError::LengthMismatch {
                declared: unit_len,
                remaining,
            });
        }
        Ok(Self {
            unit: buf[Self::HEADER_LEN..Self::HEADER_LEN + unit_len].to_vec(),
        })
    }
}

/// `host.event` BCAST — `wups_host_event_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostEventV1 {
    pub event: u8,
}

impl HostEventV1 {
    pub const WIRE_LEN: usize = 2;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        [PROTO_VERSION, self.event]
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self { event: buf[1] })
    }
}

pub mod host_event {
    pub const SHUTDOWN_IMMINENT: u8 = 1;
    pub const LOW_DISK: u8 = 2;
    pub const ETH_SYNCED: u8 = 3;
    pub const ETH_LOST: u8 = 4;
}

// ============ class 0x05 UI ============

/// `ui.button_event` BCAST — `wups_ui_button_event_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiButtonEventV1 {
    pub button: u8, // 0=left 1=right
    pub action: u8, // 0=press 1=release 2=long
}

impl UiButtonEventV1 {
    pub const WIRE_LEN: usize = 4;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        [PROTO_VERSION, self.button, self.action, 0]
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            button: buf[1],
            action: buf[2],
        })
    }
}

/// `ui.set_screen` REQ — `wups_ui_set_screen_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiSetScreenV1 {
    pub screen: u8,
}

impl UiSetScreenV1 {
    pub const WIRE_LEN: usize = 2;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        [PROTO_VERSION, self.screen]
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self { screen: buf[1] })
    }
}

/// `ui.beep` REQ — `wups_ui_beep_v1_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiBeepV1 {
    pub freq_hz: u16,
    pub dur_ms: u16,
}

impl UiBeepV1 {
    pub const WIRE_LEN: usize = 6;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0] = PROTO_VERSION;
        out[2..4].copy_from_slice(&self.freq_hz.to_le_bytes());
        out[4..6].copy_from_slice(&self.dur_ms.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        check_fixed_len(buf, Self::WIRE_LEN)?;
        check_version(buf[0])?;
        Ok(Self {
            freq_hz: u16::from_le_bytes([buf[2], buf[3]]),
            dur_ms: u16::from_le_bytes([buf[4], buf[5]]),
        })
    }
}

/// `ui.display_msg` REQ — `wups_ui_display_msg_v1_hdr_t` + text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UiDisplayMsgV1 {
    pub line: u8,
    pub text: Vec<u8>,
}

impl UiDisplayMsgV1 {
    pub const HEADER_LEN: usize = 4;

    pub fn encoded_len(&self) -> usize {
        Self::HEADER_LEN + self.text.len()
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), PayloadError> {
        if self.text.len() > u8::MAX as usize {
            return Err(PayloadError::InvalidValue("display text > 255 bytes"));
        }
        out.push(PROTO_VERSION);
        out.push(self.line);
        out.push(self.text.len() as u8);
        out.push(0);
        out.extend_from_slice(&self.text);
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, PayloadError> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, PayloadError> {
        if buf.len() < Self::HEADER_LEN {
            return Err(PayloadError::TooShort {
                needed: Self::HEADER_LEN,
                got: buf.len(),
            });
        }
        check_version(buf[0])?;
        let line = buf[1];
        let text_len = buf[2] as usize;
        let remaining = buf.len() - Self::HEADER_LEN;
        if text_len > remaining {
            return Err(PayloadError::LengthMismatch {
                declared: text_len,
                remaining,
            });
        }
        Ok(Self {
            line,
            text: buf[Self::HEADER_LEN..Self::HEADER_LEN + text_len].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T, F, G>(orig: T, encode: F, decode: G)
    where
        T: PartialEq + std::fmt::Debug,
        F: FnOnce(&T) -> Vec<u8>,
        G: FnOnce(&[u8]) -> Result<T, PayloadError>,
    {
        let bytes = encode(&orig);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, orig);
    }

    #[test]
    fn sys_pong_round_trip() {
        round_trip(
            SysPongV1 {
                fw_version: 0x0103,
                uptime_ms: 12345,
            },
            |p| p.encode().to_vec(),
            SysPongV1::decode,
        );
    }

    #[test]
    fn sys_hello_round_trip() {
        round_trip(
            SysHelloV1 {
                proto_version: PROTO_VERSION,
                node_addr: 0x02,
                fw_version: 0x0203,
                caps_classes: 0b1110,
                build_id: 0xDEADBEEF,
            },
            |p| p.encode().to_vec(),
            SysHelloV1::decode,
        );
    }

    #[test]
    fn sys_log_round_trip_with_text() {
        let log = SysLogV1 {
            level: 3,
            text: b"hello world".to_vec(),
        };
        let bytes = log.encode().unwrap();
        assert_eq!(bytes.len(), SysLogV1::HEADER_LEN + 11);
        let decoded = SysLogV1::decode(&bytes).unwrap();
        assert_eq!(decoded, log);
    }

    #[test]
    fn sys_log_empty_text() {
        let log = SysLogV1 {
            level: 0,
            text: vec![],
        };
        let bytes = log.encode().unwrap();
        let decoded = SysLogV1::decode(&bytes).unwrap();
        assert_eq!(decoded, log);
    }

    #[test]
    fn power_status_round_trip() {
        round_trip(
            PowerStatusV1 {
                charge_state: 1,
                vbus_in_mv: 12000,
                vbus_out_mv: 5100,
                ibus_out_ma: -250,
                vbat_mv: 11700,
                ibat_ma: 300,
                temp_dc: 253,
                pd_contract_mv: 9000,
                pd_contract_ma: 3000,
                faults: power_fault::PD_NEG,
            },
            |p| p.encode().to_vec(),
            PowerStatusV1::decode,
        );
    }

    #[test]
    fn power_status_known_bytes() {
        let p = PowerStatusV1 {
            charge_state: 0x01,
            vbus_in_mv: 0x1234,
            vbus_out_mv: 0x5678,
            ibus_out_ma: -1,
            vbat_mv: 0x0BCD,
            ibat_ma: 0x0100,
            temp_dc: -50,
            pd_contract_mv: 0,
            pd_contract_ma: 0,
            faults: 0,
        };
        let bytes = p.encode();
        // version=01, charge_state=01, vbus_in=34 12, vbus_out=78 56,
        // ibus_out=FF FF, vbat=CD 0B, ibat=00 01, temp=CE FF, rest zeros
        assert_eq!(
            &bytes[..],
            &[
                0x01, 0x01, 0x34, 0x12, 0x78, 0x56, 0xFF, 0xFF, 0xCD, 0x0B, 0x00, 0x01, 0xCE, 0xFF,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn power_status_v2_known_bytes() {
        // 40-byte v2 frame with a distinct value per field so any offset swap
        // is caught. Locks the four-way wire contract (CH32X / RP2040 / Rust).
        let bytes: [u8; 40] = [
            0x02, 0x1F, 0x02, 0x00, // version, flags(all 5), charge_state, reserved
            0x20, 0x4E,             // vbus_in_mV   = 20000
            0x98, 0x3A,             // pd_in_mV     = 15000
            0xD6, 0x06,             // pd_in_mA     = 1750
            0xEC, 0x13,             // vbus_out_mV  = 5100
            0xEC, 0x13,             // vout_set_mV  = 5100
            0xBA, 0x13,             // vout_read_mV = 5050
            0x88, 0x13,             // iout_limit_mA= 5000
            0x88, 0x13,             // pd_out_mV    = 5000
            0x88, 0x13,             // pd_out_mA    = 5000
            0xD0, 0x20,             // vbat_mV      = 8400
            0x06, 0xFF,             // ichg_mA      = -250
            0xFC, 0x21,             // vsys_mV      = 8700
            0xF4, 0x01,             // iin_mA       = 500
            0xFD, 0x00,             // temp_lm_dC   = 253
            0x00, 0x80,             // temp_mp_dC   = -32768 (N/A)
            0x00, 0x07,             // faults       = 0x0700 (TPS SCP/OCP/OVP)
            0x40, 0xE2, 0x01, 0x00, // uptime_s     = 123456
        ];
        let p = PowerStatusV2::decode(&bytes).expect("decode v2");
        assert_eq!(p.flags, 0x1F);
        assert_eq!(p.charge_state, 2);
        assert_eq!(p.vbus_in_mv, 20000);
        assert_eq!(p.pd_in_mv, 15000);
        assert_eq!(p.pd_in_ma, 1750);
        assert_eq!(p.vbus_out_mv, 5100);
        assert_eq!(p.vout_set_mv, 5100);
        assert_eq!(p.vout_read_mv, 5050);
        assert_eq!(p.iout_limit_ma, 5000);
        assert_eq!(p.pd_out_mv, 5000);
        assert_eq!(p.pd_out_ma, 5000);
        assert_eq!(p.vbat_mv, 8400);
        assert_eq!(p.ichg_ma, -250);
        assert_eq!(p.vsys_mv, 8700);
        assert_eq!(p.iin_ma, 500);
        assert_eq!(p.temp_lm_dc, 253);
        assert_eq!(p.temp_mp_dc, i16::MIN);
        assert_eq!(p.faults, 0x0700);
        assert_eq!(p.uptime_s, 123456);

        // to_v1() mapping: VSYS/IIN -> pd_contract_*, vout_read -> vbus_out,
        // iout_limit -> ibus_out, temp_dc = LM (MP is N/A sentinel).
        let v1 = p.to_v1();
        assert_eq!(v1.charge_state, 2);
        assert_eq!(v1.vbus_in_mv, 20000);
        assert_eq!(v1.vbus_out_mv, 5050);
        assert_eq!(v1.ibus_out_ma, 5000);
        assert_eq!(v1.vbat_mv, 8400);
        assert_eq!(v1.ibat_ma, -250);
        assert_eq!(v1.temp_dc, 253); // LM only, MP unpowered
        assert_eq!(v1.pd_contract_mv, 8700); // VSYS
        assert_eq!(v1.pd_contract_ma, 500); // IIN
        assert_eq!(v1.faults, 0x0700);
    }

    #[test]
    fn power_status_v2_rejects_bad_version_and_len() {
        let mut buf = [0u8; 40];
        buf[0] = 1; // wrong version in a correct-length buffer
        assert!(matches!(
            PowerStatusV2::decode(&buf),
            Err(PayloadError::UnsupportedVersion { .. })
        ));
        let short = [2u8; 20];
        assert!(matches!(
            PowerStatusV2::decode(&short),
            Err(PayloadError::UnexpectedLength { .. })
        ));
    }

    #[test]
    fn power_cycle_round_trip() {
        round_trip(
            PowerCycleV1 { off_ms: 5000 },
            |p| p.encode().to_vec(),
            PowerCycleV1::decode,
        );
    }

    #[test]
    fn power_event_round_trip() {
        round_trip(
            PowerEventV1 {
                event: power_event::MAINS_LOST,
            },
            |p| p.encode().to_vec(),
            PowerEventV1::decode,
        );
    }

    #[test]
    fn net_status_round_trip() {
        round_trip(
            NetStatusV1 {
                state: 4,
                rssi_dbm: -85,
                rsrp_dbm: -110,
                rsrq_db: -12,
                errors: 3,
                ip_addr: 0x0A000001,
                bytes_tx: 1024,
                bytes_rx: 4096,
            },
            |p| p.encode().to_vec(),
            NetStatusV1::decode,
        );
    }

    #[test]
    fn net_publish_round_trip_with_topic_and_payload() {
        let pub_msg = NetPublishV1 {
            qos: 1,
            retain: 0,
            topic: b"ups/123/telemetry".to_vec(),
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let bytes = pub_msg.encode().unwrap();
        let decoded = NetPublishV1::decode(&bytes).unwrap();
        assert_eq!(decoded, pub_msg);
    }

    #[test]
    fn net_publish_topic_too_long_rejected() {
        let pub_msg = NetPublishV1 {
            qos: 0,
            retain: 0,
            topic: vec![b'x'; NetPublishV1::MAX_TOPIC_LEN + 1],
            payload: vec![],
        };
        assert!(matches!(
            pub_msg.encode(),
            Err(PayloadError::InvalidValue(_))
        ));
    }

    #[test]
    fn net_time_sync_round_trip() {
        round_trip(
            NetTimeSyncV1 {
                ms_frac: 500,
                unix_s: 1_762_500_000,
            },
            |p| p.encode().to_vec(),
            NetTimeSyncV1::decode,
        );
    }

    #[test]
    fn host_status_round_trip() {
        round_trip(
            HostStatusV1 {
                eth_client_state: 1,
                cpu_temp_dc: 552, // 55.2 °C
                mem_used_pct: 42,
                disk_used_pct: 71,
                load_avg_x100: 125, // 1.25
                uptime_s: 3600 * 48,
            },
            |p| p.encode().to_vec(),
            HostStatusV1::decode,
        );
    }

    #[test]
    fn host_shutdown_round_trip() {
        round_trip(
            HostShutdownV1 {
                reason: host_shutdown_reason::LOW_BATTERY,
                delay_s: 30,
            },
            |p| p.encode().to_vec(),
            HostShutdownV1::decode,
        );
    }

    #[test]
    fn host_event_round_trip() {
        round_trip(
            HostEventV1 {
                event: host_event::ETH_SYNCED,
            },
            |p| p.encode().to_vec(),
            HostEventV1::decode,
        );
    }

    #[test]
    fn host_service_restart_round_trip() {
        let r = HostServiceRestartV1 {
            unit: b"w3p_geth".to_vec(),
        };
        let bytes = r.encode().unwrap();
        let decoded = HostServiceRestartV1::decode(&bytes).unwrap();
        assert_eq!(decoded, r);
    }

    #[test]
    fn host_service_restart_rejects_truncated_unit() {
        let bytes = [PROTO_VERSION, 10, 0, 0, b'a', b'b'];
        assert!(matches!(
            HostServiceRestartV1::decode(&bytes),
            Err(PayloadError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn ui_button_event_round_trip() {
        round_trip(
            UiButtonEventV1 {
                button: 1,
                action: 2,
            },
            |p| p.encode().to_vec(),
            UiButtonEventV1::decode,
        );
    }

    #[test]
    fn ui_set_screen_round_trip() {
        round_trip(
            UiSetScreenV1 { screen: 3 },
            |p| p.encode().to_vec(),
            UiSetScreenV1::decode,
        );
    }

    #[test]
    fn ui_beep_round_trip() {
        round_trip(
            UiBeepV1 {
                freq_hz: 2000,
                dur_ms: 100,
            },
            |p| p.encode().to_vec(),
            UiBeepV1::decode,
        );
    }

    #[test]
    fn ui_display_msg_round_trip() {
        let msg = UiDisplayMsgV1 {
            line: 2,
            text: b"BATTERY LOW".to_vec(),
        };
        let bytes = msg.encode().unwrap();
        let decoded = UiDisplayMsgV1::decode(&bytes).unwrap();
        assert_eq!(decoded, msg);
    }

    // ---------- error cases ----------

    #[test]
    fn decode_rejects_wrong_length() {
        assert!(matches!(
            PowerStatusV1::decode(&[0u8; PowerStatusV1::WIRE_LEN - 1]),
            Err(PayloadError::UnexpectedLength { .. })
        ));
        assert!(matches!(
            HostStatusV1::decode(&[0u8; HostStatusV1::WIRE_LEN + 5]),
            Err(PayloadError::UnexpectedLength { .. })
        ));
    }

    #[test]
    fn decode_rejects_unsupported_version() {
        let mut bytes = [0u8; PowerStatusV1::WIRE_LEN];
        bytes[0] = 99;
        assert!(matches!(
            PowerStatusV1::decode(&bytes),
            Err(PayloadError::UnsupportedVersion { got: 99, .. })
        ));
    }

    #[test]
    fn sys_log_decode_rejects_truncated_text() {
        // header says text_len=10, but body only has 3 bytes of text
        let bytes = [PROTO_VERSION, 2, 10, 0, b'a', b'b', b'c'];
        assert!(matches!(
            SysLogV1::decode(&bytes),
            Err(PayloadError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn net_publish_decode_rejects_truncated_payload() {
        // header claims topic_len=3, payload_len=10, but only 3 bytes of body
        let bytes = [PROTO_VERSION, 0, 0, 3, 10, 0, b't', b'o', b'p'];
        assert!(matches!(
            NetPublishV1::decode(&bytes),
            Err(PayloadError::LengthMismatch { .. })
        ));
    }
}
