//! Web3 Pi UPS wire protocol v1 — Rust implementation.
//!
//! Mirrors `Web3-Pi-UPS/common/protocol.h`. Frame format (UBX-style):
//!
//! ```text
//!   AA 55 [DST][SRC][CLASS][OP][FLAGS][SEQ][LEN_L][LEN_H] [payload..LEN] [CK_A][CK_B] 55 AA
//! ```
//!
//! Fletcher-8 covers `DST..LEN_H..payload`. Total wire size = 14 + LEN bytes.

// This module is the complete wire-protocol catalogue. Some addresses, ops,
// and payload variants are defined here but are not exercised by the agent
// binary (e.g., UI ops are RP2040-side). Suppress dead-code warnings for the
// whole module rather than tagging every constant.
#![allow(dead_code, unused_imports)]

mod deframer;
mod fletcher;
mod frame;
pub mod payloads;

pub use deframer::Deframer;
pub use fletcher::fletcher8;
pub use frame::{Frame, FrameError};

/// Protocol version (matches `WUPS_PROTO_VERSION`).
pub const PROTO_VERSION: u8 = 1;

pub const SYNC1: u8 = 0xAA;
pub const SYNC2: u8 = 0x55;
pub const END1: u8 = 0x55;
pub const END2: u8 = 0xAA;

/// SYNC1 + SYNC2 + 8 header bytes = 10.
pub const HEADER_BYTES: usize = 10;
/// CK_A + CK_B + END1 + END2 = 4.
pub const TRAILER_BYTES: usize = 4;
/// Total framing overhead = 14.
pub const FRAMING_BYTES: usize = HEADER_BYTES + TRAILER_BYTES;
/// Header bytes covered by the checksum (DST..LEN_H), excluding SYNC.
pub const HEADER_DATA_BYTES: usize = 8;
/// Maximum payload size (matches `WUPS_MAX_PAYLOAD`).
pub const MAX_PAYLOAD: usize = 240;
/// Maximum complete frame size on the wire.
pub const MAX_FRAME: usize = FRAMING_BYTES + MAX_PAYLOAD;

pub mod addr {
    pub const NULL: u8 = 0x00;
    pub const RPI: u8 = 0x01;
    pub const RP2040: u8 = 0x02;
    pub const CH32X: u8 = 0x03;
    pub const ESP32: u8 = 0x04;
    pub const INTERNAL: u8 = 0x05;
    pub const BROADCAST: u8 = 0xFF;
}

pub mod class {
    pub const SYSTEM: u8 = 0x01;
    pub const POWER: u8 = 0x02;
    pub const NET: u8 = 0x03;
    pub const HOST: u8 = 0x04;
    pub const UI: u8 = 0x05;
}

pub mod flag {
    pub const REQ: u8 = 1 << 0;
    pub const RESP: u8 = 1 << 1;
    pub const EVENT: u8 = 1 << 2;
    pub const NEED_ACK: u8 = 1 << 7;
}

pub mod op {
    pub mod system {
        pub const PING: u8 = 0x01;
        pub const HELLO: u8 = 0x02;
        pub const STATUS_QUERY: u8 = 0x03;
        pub const LOG: u8 = 0x04;
    }
    pub mod power {
        pub const STATUS: u8 = 0x01;
        pub const ENABLE: u8 = 0x02;
        pub const DISABLE: u8 = 0x03;
        pub const CYCLE: u8 = 0x04;
        pub const RESET: u8 = 0x05;
        pub const EVENT: u8 = 0x10;
    }
    pub mod net {
        pub const STATUS: u8 = 0x01;
        pub const PUBLISH: u8 = 0x02;
        pub const DOWNLINK: u8 = 0x10;
        pub const TIME_SYNC: u8 = 0x20;
    }
    pub mod host {
        pub const STATUS: u8 = 0x01;
        pub const SHUTDOWN: u8 = 0x02;
        pub const RESET: u8 = 0x03;
        pub const SERVICE_RESTART: u8 = 0x04;
        pub const EVENT: u8 = 0x10;
    }
    pub mod ui {
        pub const BUTTON_EVENT: u8 = 0x01;
        pub const SET_SCREEN: u8 = 0x02;
        pub const BEEP: u8 = 0x03;
        pub const DISPLAY_MSG: u8 = 0x04;
    }
}

pub mod cap {
    use super::class;
    pub const SYSTEM: u16 = 1 << class::SYSTEM;
    pub const POWER: u16 = 1 << class::POWER;
    pub const NET: u16 = 1 << class::NET;
    pub const HOST: u16 = 1 << class::HOST;
    pub const UI: u16 = 1 << class::UI;
}
