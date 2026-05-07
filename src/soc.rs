//! Battery state-of-charge computation.
//!
//! Mirrors `voltageToSoc()` in
//! [`Web3-Pi-UPS/firmware-rp2040/src/main.cpp`] for the **Panasonic
//! CGR18650CH 2250 mAh** cell, applied to the 2S pack used in the Web3 Pi
//! UPS. Per-cell voltage thresholds (3.20–4.00 V); the pack voltage is
//! halved before the lookup. Linear interpolation between the table
//! entries; clamped at both ends.
//!
//! Keep this LUT in lockstep with the RP2040 firmware so the OLED and the
//! agent agree on SOC.

const LUT: &[(u16, u8)] = &[
    (4000, 100),
    (3900, 88),
    (3800, 75),
    (3700, 60),
    (3600, 40),
    (3500, 22),
    (3400, 10),
    (3300, 4),
    (3200, 0),
];

/// Compute SOC% from 2S pack voltage (mV).
pub fn pack_mv_to_soc_pct(vbat_mv: u16) -> u8 {
    let cell_mv = vbat_mv / 2;
    if cell_mv >= LUT[0].0 {
        return LUT[0].1;
    }
    let last = LUT[LUT.len() - 1];
    if cell_mv <= last.0 {
        return last.1;
    }
    for w in LUT.windows(2) {
        let (v_hi, s_hi) = w[0]; // higher voltage, higher SOC
        let (v_lo, s_lo) = w[1];
        if cell_mv <= v_hi && cell_mv > v_lo {
            let dv = (v_hi - v_lo) as i32;
            let ds = s_hi as i32 - s_lo as i32;
            let above = (cell_mv - v_lo) as i32;
            let interp = s_lo as i32 + above * ds / dv;
            return interp.clamp(0, 100) as u8;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_clamp() {
        assert_eq!(pack_mv_to_soc_pct(8400), 100); // 4.20 V/cell — over the top
        assert_eq!(pack_mv_to_soc_pct(8000), 100); // 4.00 V/cell — table top
        assert_eq!(pack_mv_to_soc_pct(6400), 0); // 3.20 V/cell — table bottom
        assert_eq!(pack_mv_to_soc_pct(5000), 0); // below table
    }

    #[test]
    fn matches_lut_points_exactly() {
        for &(v_cell, expected) in LUT {
            let v_pack = v_cell * 2;
            assert_eq!(
                pack_mv_to_soc_pct(v_pack),
                expected,
                "mismatch at {v_cell} mV/cell ({v_pack} mV pack)"
            );
        }
    }

    #[test]
    fn interp_3_95_v_cell_is_94pct() {
        // Halfway between 4.00 V (100 %) and 3.90 V (88 %) → 94 %.
        let pack = 3950 * 2;
        assert_eq!(pack_mv_to_soc_pct(pack), 94);
    }

    #[test]
    fn knee_at_3_55_v_cell_is_16pct() {
        // Between 3.60 V (40 %) and 3.50 V (22 %) — the discharge knee.
        // Mid: 3.55 V → 31 %.
        let pack = 3550 * 2;
        let pct = pack_mv_to_soc_pct(pack);
        assert!((pct as i32 - 31).abs() <= 1, "expected ~31, got {pct}");
    }

    #[test]
    fn monotonic_non_decreasing() {
        // Sanity: SOC never decreases as voltage increases through the table.
        let mut last = 0u8;
        for v in (6400..=8000).step_by(50) {
            let s = pack_mv_to_soc_pct(v);
            assert!(s >= last, "non-monotonic at {v} mV: {s} < {last}");
            last = s;
        }
    }
}
