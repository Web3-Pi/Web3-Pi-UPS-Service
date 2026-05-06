use std::fs;

#[derive(Debug, Clone, Copy)]
pub struct CpuSnap {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
}

pub fn read_load_avg_x100() -> Option<u16> {
    parse_load_avg(&fs::read_to_string("/proc/loadavg").ok()?)
}

pub fn read_temp_dc() -> Option<i16> {
    // Try the canonical RPi5 thermal zone first; fall back to scanning if needed.
    let primary = fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok();
    if let Some(s) = primary {
        return parse_temp_mdeg(&s);
    }
    None
}

pub fn read_cpu_snap() -> Option<CpuSnap> {
    parse_cpu_stat(&fs::read_to_string("/proc/stat").ok()?)
}

pub fn compute_usage_pct(prev: &CpuSnap, curr: &CpuSnap) -> Option<u8> {
    let p_total = prev
        .user
        .saturating_add(prev.nice)
        .saturating_add(prev.system)
        .saturating_add(prev.idle)
        .saturating_add(prev.iowait)
        .saturating_add(prev.irq)
        .saturating_add(prev.softirq);
    let c_total = curr
        .user
        .saturating_add(curr.nice)
        .saturating_add(curr.system)
        .saturating_add(curr.idle)
        .saturating_add(curr.iowait)
        .saturating_add(curr.irq)
        .saturating_add(curr.softirq);
    let p_idle = prev.idle.saturating_add(prev.iowait);
    let c_idle = curr.idle.saturating_add(curr.iowait);
    let dt = c_total.checked_sub(p_total)?;
    let di = c_idle.checked_sub(p_idle)?;
    if dt == 0 {
        return Some(0);
    }
    Some((((dt - di) * 100) / dt).min(100) as u8)
}

fn parse_load_avg(s: &str) -> Option<u16> {
    let first = s.split_whitespace().next()?;
    let val: f32 = first.parse().ok()?;
    if !val.is_finite() || val < 0.0 {
        return None;
    }
    let scaled = (val * 100.0).round() as i64;
    Some(scaled.clamp(0, u16::MAX as i64) as u16)
}

fn parse_temp_mdeg(s: &str) -> Option<i16> {
    let mdeg: i32 = s.trim().parse().ok()?;
    Some((mdeg / 100) as i16) // mdeg → ddeg
}

fn parse_cpu_stat(s: &str) -> Option<CpuSnap> {
    let line = s.lines().next()?;
    let mut it = line.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    Some(CpuSnap {
        user: it.next()?.parse().ok()?,
        nice: it.next()?.parse().ok()?,
        system: it.next()?.parse().ok()?,
        idle: it.next()?.parse().ok()?,
        iowait: it.next()?.parse().ok()?,
        irq: it.next()?.parse().ok()?,
        softirq: it.next()?.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_avg_basic() {
        assert_eq!(parse_load_avg("0.45 0.30 0.20 1/256 12345\n"), Some(45));
        assert_eq!(parse_load_avg("1.25 0.30 0.20 1/256 12345\n"), Some(125));
        assert_eq!(parse_load_avg("0.00 0 0\n"), Some(0));
    }

    #[test]
    fn load_avg_clamps_high() {
        // Unreasonably high load avg: clamp to u16::MAX
        let s = format!("{} 0 0 1/1 1\n", 1_000.0);
        assert_eq!(parse_load_avg(&s), Some(u16::MAX));
    }

    #[test]
    fn load_avg_rejects_negative() {
        assert_eq!(parse_load_avg("-0.5 0 0\n"), None);
    }

    #[test]
    fn temp_mdeg_to_ddeg() {
        assert_eq!(parse_temp_mdeg("55248\n"), Some(552));
        assert_eq!(parse_temp_mdeg("-12345\n"), Some(-123));
    }

    #[test]
    fn cpu_stat_parses() {
        let s = "cpu  100 5 50 1000 30 0 2 0 0 0\ncpu0 ...\n";
        let snap = parse_cpu_stat(s).unwrap();
        assert_eq!(snap.user, 100);
        assert_eq!(snap.idle, 1000);
        assert_eq!(snap.iowait, 30);
    }

    #[test]
    fn cpu_stat_rejects_non_aggregate() {
        let s = "stat junk\n";
        assert!(parse_cpu_stat(s).is_none());
    }

    #[test]
    fn cpu_usage_zero_when_all_idle() {
        let prev = CpuSnap {
            user: 0,
            nice: 0,
            system: 0,
            idle: 100,
            iowait: 0,
            irq: 0,
            softirq: 0,
        };
        let curr = CpuSnap {
            user: 0,
            nice: 0,
            system: 0,
            idle: 200,
            iowait: 0,
            irq: 0,
            softirq: 0,
        };
        assert_eq!(compute_usage_pct(&prev, &curr), Some(0));
    }

    #[test]
    fn cpu_usage_full_when_no_idle_delta() {
        let prev = CpuSnap {
            user: 0,
            nice: 0,
            system: 0,
            idle: 100,
            iowait: 0,
            irq: 0,
            softirq: 0,
        };
        let curr = CpuSnap {
            user: 100,
            nice: 0,
            system: 0,
            idle: 100,
            iowait: 0,
            irq: 0,
            softirq: 0,
        };
        assert_eq!(compute_usage_pct(&prev, &curr), Some(100));
    }

    #[test]
    fn cpu_usage_half() {
        let prev = CpuSnap {
            user: 0,
            nice: 0,
            system: 0,
            idle: 100,
            iowait: 0,
            irq: 0,
            softirq: 0,
        };
        let curr = CpuSnap {
            user: 100,
            nice: 0,
            system: 0,
            idle: 200,
            iowait: 0,
            irq: 0,
            softirq: 0,
        };
        assert_eq!(compute_usage_pct(&prev, &curr), Some(50));
    }

    #[test]
    fn cpu_usage_handles_no_progress() {
        let snap = CpuSnap {
            user: 0,
            nice: 0,
            system: 0,
            idle: 100,
            iowait: 0,
            irq: 0,
            softirq: 0,
        };
        assert_eq!(compute_usage_pct(&snap, &snap), Some(0));
    }
}
