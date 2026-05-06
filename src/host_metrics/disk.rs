use std::ffi::CString;

/// Used % of the filesystem mounted at `path`. None on syscall failure.
pub fn read_used_pct(path: &str) -> Option<u8> {
    let cpath = CString::new(path).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: cpath is a valid C string; stat is valid uninit-ed memory we
    // pass &mut to.
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let bs: u64 = stat.f_frsize;
    let total = (stat.f_blocks as u64).saturating_mul(bs);
    let avail = (stat.f_bavail as u64).saturating_mul(bs);
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(avail);
    Some(((used * 100) / total).min(100) as u8)
}
