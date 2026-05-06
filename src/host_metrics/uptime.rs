use std::fs;

pub fn read_uptime_s() -> Option<u32> {
    parse_uptime(&fs::read_to_string("/proc/uptime").ok()?)
}

fn parse_uptime(s: &str) -> Option<u32> {
    let first = s.split_whitespace().next()?;
    let val: f64 = first.parse().ok()?;
    if !val.is_finite() || val < 0.0 {
        return None;
    }
    Some((val.round() as u64).min(u32::MAX as u64) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_uptime() {
        assert_eq!(parse_uptime("12345.67 5432.10\n"), Some(12346));
    }

    #[test]
    fn rejects_negative() {
        assert_eq!(parse_uptime("-1.0 0\n"), None);
    }

    #[test]
    fn handles_zero() {
        assert_eq!(parse_uptime("0.0 0.0\n"), Some(0));
    }
}
