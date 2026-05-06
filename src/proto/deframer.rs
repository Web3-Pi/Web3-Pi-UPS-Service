use super::frame::{Frame, FrameError};
use super::{fletcher::fletcher8, END1, END2, HEADER_DATA_BYTES, MAX_PAYLOAD, SYNC1, SYNC2};

/// Streaming byte-by-byte frame parser.
///
/// Feed bytes via [`Deframer::feed`]. On a complete, valid frame, `feed`
/// returns `Some(Ok(frame))`. On a parse error, it returns `Some(Err(_))` and
/// resets to scanning for the next sync sequence.
pub struct Deframer {
    state: State,
    /// Accumulator: 8 header bytes + payload + CK_A + CK_B (excluding SYNC).
    buf: Vec<u8>,
    payload_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Sync1,
    Sync2,
    Header,
    Payload,
    CkA,
    CkB,
    End1,
    End2,
}

impl Default for Deframer {
    fn default() -> Self {
        Self::new()
    }
}

impl Deframer {
    pub fn new() -> Self {
        Self {
            state: State::Sync1,
            buf: Vec::with_capacity(HEADER_DATA_BYTES + MAX_PAYLOAD + 2),
            payload_len: 0,
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Sync1;
        self.buf.clear();
        self.payload_len = 0;
    }

    /// Feed one byte. Returns `Some(Ok(frame))` on a completed frame,
    /// `Some(Err(_))` on a parse error (deframer auto-resets), or `None` if
    /// more bytes are needed.
    pub fn feed(&mut self, byte: u8) -> Option<Result<Frame, FrameError>> {
        match self.state {
            State::Sync1 => {
                if byte == SYNC1 {
                    self.state = State::Sync2;
                }
                None
            }
            State::Sync2 => {
                if byte == SYNC2 {
                    self.buf.clear();
                    self.payload_len = 0;
                    self.state = State::Header;
                } else if byte == SYNC1 {
                    // Two 0xAA in a row — second one might be the real sync1.
                } else {
                    self.state = State::Sync1;
                }
                None
            }
            State::Header => {
                self.buf.push(byte);
                if self.buf.len() == HEADER_DATA_BYTES {
                    let len = u16::from_le_bytes([self.buf[6], self.buf[7]]) as usize;
                    if len > MAX_PAYLOAD {
                        let err = FrameError::PayloadTooLong(len);
                        self.reset();
                        return Some(Err(err));
                    }
                    self.payload_len = len;
                    self.state = if len == 0 { State::CkA } else { State::Payload };
                }
                None
            }
            State::Payload => {
                self.buf.push(byte);
                if self.buf.len() == HEADER_DATA_BYTES + self.payload_len {
                    self.state = State::CkA;
                }
                None
            }
            State::CkA => {
                self.buf.push(byte);
                self.state = State::CkB;
                None
            }
            State::CkB => {
                self.buf.push(byte);
                let payload_end = HEADER_DATA_BYTES + self.payload_len;
                let cka = self.buf[payload_end];
                let ckb = self.buf[payload_end + 1];
                let (computed_a, computed_b) = fletcher8(&self.buf[..payload_end]);
                if cka != computed_a || ckb != computed_b {
                    let err = FrameError::BadChecksum {
                        expected: (cka, ckb),
                        computed: (computed_a, computed_b),
                    };
                    self.reset();
                    return Some(Err(err));
                }
                self.state = State::End1;
                None
            }
            State::End1 => {
                if byte == END1 {
                    self.state = State::End2;
                    None
                } else {
                    self.reset();
                    Some(Err(FrameError::BadEndMarker))
                }
            }
            State::End2 => {
                if byte == END2 {
                    let frame = Frame {
                        dst: self.buf[0],
                        src: self.buf[1],
                        class: self.buf[2],
                        op: self.buf[3],
                        flags: self.buf[4],
                        seq: self.buf[5],
                        payload: self.buf[HEADER_DATA_BYTES..HEADER_DATA_BYTES + self.payload_len]
                            .to_vec(),
                    };
                    self.reset();
                    Some(Ok(frame))
                } else {
                    self.reset();
                    Some(Err(FrameError::BadEndMarker))
                }
            }
        }
    }

    /// Feed a slice of bytes; pushes any complete results to `sink`.
    pub fn feed_slice<F: FnMut(Result<Frame, FrameError>)>(&mut self, bytes: &[u8], mut sink: F) {
        for &b in bytes {
            if let Some(r) = self.feed(b) {
                sink(r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{addr, class, flag, op};

    fn run(bytes: &[u8]) -> Vec<Result<Frame, FrameError>> {
        let mut d = Deframer::new();
        let mut out = Vec::new();
        d.feed_slice(bytes, |r| out.push(r));
        out
    }

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
    fn parses_one_clean_frame() {
        let bytes = ping_req().encode().unwrap();
        let results = run(&bytes);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), &ping_req());
    }

    #[test]
    fn parses_two_back_to_back() {
        let mut bytes = ping_req().encode().unwrap();
        bytes.extend_from_slice(&ping_req().encode().unwrap());
        let results = run(&bytes);
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_eq!(r.as_ref().unwrap(), &ping_req());
        }
    }

    #[test]
    fn skips_garbage_prefix() {
        let mut bytes = vec![0x12, 0x34, 0x55, 0x99, 0x00];
        bytes.extend_from_slice(&ping_req().encode().unwrap());
        let results = run(&bytes);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), &ping_req());
    }

    #[test]
    fn double_aa_resyncs() {
        let mut bytes = vec![0xAA];
        bytes.extend_from_slice(&ping_req().encode().unwrap());
        let results = run(&bytes);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), &ping_req());
    }

    #[test]
    fn bad_checksum_emits_error_then_recovers() {
        let mut bytes = ping_req().encode().unwrap();
        let cka_idx = bytes.len() - 4;
        bytes[cka_idx] = bytes[cka_idx].wrapping_add(1);
        bytes.extend_from_slice(&ping_req().encode().unwrap());

        let results = run(&bytes);
        assert!(results
            .iter()
            .any(|r| matches!(r, Err(FrameError::BadChecksum { .. }))));
        assert!(results
            .iter()
            .any(|r| matches!(r, Ok(f) if f == &ping_req())));
    }

    #[test]
    fn bad_end_marker_emits_error_then_recovers() {
        let mut bytes = ping_req().encode().unwrap();
        let last = bytes.len() - 1;
        bytes[last] = 0x00;
        bytes.extend_from_slice(&ping_req().encode().unwrap());

        let results = run(&bytes);
        assert!(results
            .iter()
            .any(|r| matches!(r, Err(FrameError::BadEndMarker))));
        assert!(results
            .iter()
            .any(|r| matches!(r, Ok(f) if f == &ping_req())));
    }

    #[test]
    fn oversized_len_emits_error() {
        let bytes = vec![SYNC1, SYNC2, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF];
        let results = run(&bytes);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(FrameError::PayloadTooLong(_))));
    }

    #[test]
    fn truncated_yields_no_complete_frame() {
        let bytes = ping_req().encode().unwrap();
        for trim in 1..bytes.len() {
            let truncated = &bytes[..bytes.len() - trim];
            let results = run(truncated);
            for r in results {
                assert!(r.is_err());
            }
        }
    }

    #[test]
    fn payload_round_trip_through_deframer() {
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
        let results = run(&bytes);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), &frame);
    }

    #[test]
    fn fed_one_byte_at_a_time() {
        let bytes = ping_req().encode().unwrap();
        let mut d = Deframer::new();
        let mut completed: Option<Frame> = None;
        for &b in &bytes {
            if let Some(r) = d.feed(b) {
                completed = Some(r.unwrap());
            }
        }
        assert_eq!(completed.unwrap(), ping_req());
    }
}
