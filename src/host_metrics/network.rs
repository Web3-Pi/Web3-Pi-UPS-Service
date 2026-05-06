use std::fs;

#[derive(Debug, Clone, Copy, Default)]
pub struct NetTotals {
    pub bytes_rx: u64,
    pub bytes_tx: u64,
}

pub fn read_totals() -> Option<NetTotals> {
    parse_totals(&fs::read_to_string("/proc/net/dev").ok()?)
}

fn parse_totals(s: &str) -> Option<NetTotals> {
    // First two lines are headers in /proc/net/dev.
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut had_any = false;
    for line in s.lines().skip(2) {
        let Some(colon) = line.find(':') else {
            continue;
        };
        let iface = line[..colon].trim();
        if iface == "lo" {
            continue;
        }
        let nums: Vec<&str> = line[colon + 1..].split_whitespace().collect();
        // 16 fields per row: 8 RX (bytes/packets/errs/drop/fifo/frame/compressed/multicast)
        // followed by 8 TX (bytes/packets/errs/drop/fifo/colls/carrier/compressed).
        if nums.len() >= 9 {
            let r = nums[0].parse::<u64>().unwrap_or(0);
            let t = nums[8].parse::<u64>().unwrap_or(0);
            rx = rx.saturating_add(r);
            tx = tx.saturating_add(t);
            had_any = true;
        }
    }
    if had_any {
        Some(NetTotals {
            bytes_rx: rx,
            bytes_tx: tx,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  1024    10     0    0    0    0     0          0         1024    10     0    0    0    0     0          0
  eth0: 12345    100    0    0    0    0     0          0         54321   200    0    0    0    0     0          0
  wlan0:  500    5      0    0    0    0     0          0         2000    8      0    0    0    0     0          0
";

    #[test]
    fn aggregates_non_loopback() {
        let n = parse_totals(SAMPLE).unwrap();
        // eth0 (12345) + wlan0 (500) = 12845; lo skipped.
        assert_eq!(n.bytes_rx, 12345 + 500);
        // eth0 (54321) + wlan0 (2000) = 56321
        assert_eq!(n.bytes_tx, 54321 + 2000);
    }

    #[test]
    fn loopback_only_returns_none() {
        let s = "h1\nh2\n    lo:  1 1 0 0 0 0 0 0  1 1 0 0 0 0 0 0\n";
        assert!(parse_totals(s).is_none());
    }

    #[test]
    fn empty_returns_none() {
        let s = "h1\nh2\n";
        assert!(parse_totals(s).is_none());
    }
}
