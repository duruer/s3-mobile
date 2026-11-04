//! Host wall-clock helpers for kernel32 time functions (FILETIME = 100 ns ticks since 1601-01-01 UTC).

use std::time::{SystemTime, UNIX_EPOCH};

/// 100 ns ticks between 1601-01-01 and 1970-01-01.
pub const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// Current UTC time as FILETIME.
pub fn now_filetime() -> u64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    FILETIME_UNIX_EPOCH + d.as_secs() * 10_000_000 + d.subsec_nanos() as u64 / 100
}

/// FILETIME of a host `SystemTime` (file timestamps).
pub fn systemtime_to_filetime(t: SystemTime) -> u64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => FILETIME_UNIX_EPOCH + d.as_secs() * 10_000_000 + d.subsec_nanos() as u64 / 100,
        Err(_) => FILETIME_UNIX_EPOCH,
    }
}

/// Host local time offset from UTC in seconds (east positive), e.g. +3600 for CET. 0 if unknown.
pub fn utc_offset_secs() -> i32 {
    #[cfg(unix)]
    {
        // SAFETY: localtime_r writes into our zeroed tm; time(NULL) is always safe.
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if !libc::localtime_r(&t, &mut tm).is_null() {
                return tm.tm_gmtoff as i32;
            }
        }
    }
    0
}
