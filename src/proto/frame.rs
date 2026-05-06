use super::{
    fletcher::fletcher8, END1, END2, FRAMING_BYTES, HEADER_BYTES, MAX_PAYLOAD, SYNC1, SYNC2,
};

/// One protocol frame on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub dst: u8,
    pub src: u8,
    pub class: u8,
    pub op: u8,
    pub flags: u8,
    pub seq: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("payload too long: {0} bytes (max 240)")]
    PayloadTooLong(usize),
    #[error("buffer too short for a complete frame")]
    TooShort,
    #[error("invalid sync bytes")]
    BadSync,
    #[error("invalid end marker")]
    BadEndMarker,
    #[error("checksum mismatch")]
    BadChecksum {
        expected: (u8, u8),
        computed: (u8, u8),
    },
}

impl Frame {
    /// Total wire length of this frame in bytes.
    pub fn encoded_len(&self) -> usize {
        FRAMING_BYTES + self.payload.len()
    }

    /// Append the wire encoding of this frame to `out`.
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), FrameError> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(FrameError::PayloadTooLong(self.payload.len()));
        }
        let len = self.payload.len() as u16;
        let len_bytes = len.to_le_bytes();

        out.push(SYNC1);
        out.push(SYNC2);
        let cks_start = out.len();
        out.push(self.dst);
        out.push(self.src);
        out.push(self.class);
        out.push(self.op);
        out.push(self.flags);
        out.push(self.seq);
        out.push(len_bytes[0]);
        out.push(len_bytes[1]);
        out.extend_from_slice(&self.payload);

        let (cka, ckb) = fletcher8(&out[cks_start..]);
        out.push(cka);
        out.push(ckb);
        out.push(END1);
        out.push(END2);
        Ok(())
    }

    /// Allocate and return the wire encoding of this frame.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    /// Decode one complete frame from the start of `buf`.
    ///
    /// Returns the parsed frame and the number of bytes consumed. The buffer
    /// must start at the SYNC1 byte.
    pub fn decode(buf: &[u8]) -> Result<(Self, usize), FrameError> {
        if buf.len() < FRAMING_BYTES {
            return Err(FrameError::TooShort);
        }
        if buf[0] != SYNC1 || buf[1] != SYNC2 {
            return Err(FrameError::BadSync);
        }
        let dst = buf[2];
        let src = buf[3];
        let class = buf[4];
        let op = buf[5];
        let flags = buf[6];
        let seq = buf[7];
        let len = u16::from_le_bytes([buf[8], buf[9]]) as usize;
        if len > MAX_PAYLOAD {
            return Err(FrameError::PayloadTooLong(len));
        }
        let total = FRAMING_BYTES + len;
        if buf.len() < total {
            return Err(FrameError::TooShort);
        }
        let payload_start = HEADER_BYTES;
        let payload_end = payload_start + len;
        let cka = buf[payload_end];
        let ckb = buf[payload_end + 1];
        let e1 = buf[payload_end + 2];
        let e2 = buf[payload_end + 3];

        let (computed_a, computed_b) = fletcher8(&buf[2..payload_end]);
        if computed_a != cka || computed_b != ckb {
            return Err(FrameError::BadChecksum {
                expected: (cka, ckb),
                computed: (computed_a, computed_b),
            });
        }
        if e1 != END1 || e2 != END2 {
            return Err(FrameError::BadEndMarker);
        }

        Ok((
            Frame {
                dst,
                src,
                class,
                op,
                flags,
                seq,
                payload: buf[payload_start..payload_end].to_vec(),
            },
            total,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{addr, class, flag, op};

    fn ping_req() -> Frame {
        Frame {
            dst: addr::CH32X,
            src: addr::RPI,
            class: class::SYSTEM,
            op: op::system::PING,
            flags: flag::REQ,
            seq: 0x42,
            payload: vec![],
        }
    }

    #[test]
    fn encode_matches_spec_example() {
        // protocol_desc.md "Example: RPi pings CH32X":
        //   AA 55 03 01 01 01 01 42 00 00 49 F4 55 AA
        let bytes = ping_req().encode().unwrap();
        assert_eq!(
            bytes,
            vec![
                0xAA, 0x55, 0x03, 0x01, 0x01, 0x01, 0x01, 0x42, 0x00, 0x00, 0x49, 0xF4, 0x55, 0xAA
            ]
        );
        assert_eq!(bytes.len(), FRAMING_BYTES);
    }

    #[test]
    fn empty_payload_round_trip() {
        let frame = ping_req();
        let bytes = frame.encode().unwrap();
        let (decoded, consumed) = Frame::decode(&bytes).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn payload_round_trip() {
        let frame = Frame {
            dst: addr::RPI,
            src: addr::CH32X,
            class: class::POWER,
            op: op::power::STATUS,
            flags: flag::EVENT,
            seq: 7,
            payload: (0..20u8).collect(),
        };
        let bytes = frame.encode().unwrap();
        assert_eq!(bytes.len(), FRAMING_BYTES + 20);
        let (decoded, consumed) = Frame::decode(&bytes).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn max_payload_round_trip() {
        let frame = Frame {
            dst: addr::RP2040,
            src: addr::ESP32,
            class: class::NET,
            op: op::net::PUBLISH,
            flags: flag::REQ,
            seq: 0,
            payload: vec![0xAB; MAX_PAYLOAD],
        };
        let bytes = frame.encode().unwrap();
        let (decoded, _) = Frame::decode(&bytes).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn payload_too_long_rejected_on_encode() {
        let frame = Frame {
            dst: 0,
            src: 0,
            class: 0,
            op: 0,
            flags: 0,
            seq: 0,
            payload: vec![0; MAX_PAYLOAD + 1],
        };
        assert!(matches!(frame.encode(), Err(FrameError::PayloadTooLong(_))));
    }

    #[test]
    fn bad_sync_rejected() {
        let mut bytes = ping_req().encode().unwrap();
        bytes[0] = 0x00;
        assert_eq!(Frame::decode(&bytes), Err(FrameError::BadSync));
    }

    #[test]
    fn bad_checksum_rejected() {
        let mut bytes = ping_req().encode().unwrap();
        let cka_idx = bytes.len() - 4;
        bytes[cka_idx] = bytes[cka_idx].wrapping_add(1);
        assert!(matches!(
            Frame::decode(&bytes),
            Err(FrameError::BadChecksum { .. })
        ));
    }

    #[test]
    fn bad_end_marker_rejected() {
        let mut bytes = ping_req().encode().unwrap();
        let last = bytes.len() - 1;
        bytes[last] = 0x00;
        assert_eq!(Frame::decode(&bytes), Err(FrameError::BadEndMarker));
    }

    #[test]
    fn truncated_too_short() {
        let bytes = ping_req().encode().unwrap();
        for trim in 1..bytes.len() {
            let truncated = &bytes[..bytes.len() - trim];
            assert_eq!(Frame::decode(truncated), Err(FrameError::TooShort));
        }
    }

    #[test]
    fn declared_len_exceeds_max() {
        // Hand-craft header with LEN=0xFFFF.
        let mut bytes = vec![SYNC1, SYNC2, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF];
        bytes.extend_from_slice(&[0; FRAMING_BYTES]); // padding so .len() check passes
        assert!(matches!(
            Frame::decode(&bytes),
            Err(FrameError::PayloadTooLong(_))
        ));
    }
}
