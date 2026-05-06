use std::fs;

pub fn read_used_pct() -> Option<u8> {
    parse_used_pct(&fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_used_pct(s: &str) -> Option<u8> {
    let mut total: Option<u64> = None;
    let mut avail: Option<u64> = None;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail = parse_kb(rest);
        }
        if total.is_some() && avail.is_some() {
            break;
        }
    }
    let t = total?;
    let a = avail?;
    if t == 0 {
        return Some(0);
    }
    let used = t.saturating_sub(a);
    Some(((used * 100) / t).min(100) as u8)
}

fn parse_kb(s: &str) -> Option<u64> {
    s.split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_parses() {
        let s = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\n";
        assert_eq!(parse_used_pct(s), Some(50));
    }

    #[test]
    fn meminfo_full_used() {
        let s = "MemTotal:       1000 kB\nMemAvailable:   0 kB\n";
        assert_eq!(parse_used_pct(s), Some(100));
    }

    #[test]
    fn meminfo_unused() {
        let s = "MemTotal:       1000 kB\nMemAvailable:   1000 kB\n";
        assert_eq!(parse_used_pct(s), Some(0));
    }

    #[test]
    fn meminfo_missing_fields() {
        let s = "MemTotal:       1000 kB\n";
        assert_eq!(parse_used_pct(s), None);
    }
}
