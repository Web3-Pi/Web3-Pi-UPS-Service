//! Fletcher-8 checksum (UBX-compatible).

/// Compute Fletcher-8 over `buf`. Returns `(ck_a, ck_b)`.
///
/// ```text
///   for each byte b:  a = (a + b) mod 256;  b = (b + a) mod 256
/// ```
pub fn fletcher8(buf: &[u8]) -> (u8, u8) {
    let mut a: u8 = 0;
    let mut b: u8 = 0;
    for &byte in buf {
        a = a.wrapping_add(byte);
        b = b.wrapping_add(a);
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_zero() {
        assert_eq!(fletcher8(&[]), (0, 0));
    }

    #[test]
    fn single_byte() {
        assert_eq!(fletcher8(&[0x01]), (0x01, 0x01));
        assert_eq!(fletcher8(&[0xFF]), (0xFF, 0xFF));
    }

    #[test]
    fn two_bytes() {
        // a: 0+1=1, 1+2=3
        // b: 0+1=1, 1+3=4
        assert_eq!(fletcher8(&[0x01, 0x02]), (3, 4));
    }

    #[test]
    fn rpi_pings_ch32x_spec_example() {
        // From protocol_desc.md "Example: RPi pings CH32X"
        // Header bytes: DST=03 SRC=01 CLASS=01 OP=01 FLAGS=01 SEQ=42 LEN=00 00
        let buf = [0x03, 0x01, 0x01, 0x01, 0x01, 0x42, 0x00, 0x00];
        assert_eq!(fletcher8(&buf), (0x49, 0xF4));
    }

    #[test]
    fn wraps_at_256() {
        // a: FF, FE, FD, FC ; b: FF, FD, FA, F6
        assert_eq!(fletcher8(&[0xFF; 4]), (0xFC, 0xF6));
    }
}
