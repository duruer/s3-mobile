//! kernel32: time (GetTickCount, QueryPerformanceCounter/Frequency, GetSystemTime/GetLocalTime, FILETIME
//! conversions, time zone).
//!
//! - `GetTickCount` = host monotonic ms (`Host::now_ms`, stops while the frontend pauses the game).
//! - `QueryPerformanceFrequency` = 1 MHz; the counter is host monotonic µs and strictly increases between calls
//!   (the blit benchmark at 0x5c4280 loops until it advances).
//! - Wall clock = host clock. Local time uses the host's current UTC offset; `GetTimeZoneInformation` reports
//!   that offset as the bias with no daylight rules (TIME_ZONE_ID_UNKNOWN).

use std::time::Instant;

use s3_runtime::hosttime::{now_filetime, utc_offset_secs};
use s3_runtime::{Machine, Registry, ThunkResult};

use super::DLL;

pub(crate) fn register(reg: &mut Registry) {
    reg.register(DLL, "GetTickCount", get_tick_count);
    reg.register(DLL, "QueryPerformanceCounter", query_performance_counter);
    reg.register(DLL, "QueryPerformanceFrequency", query_performance_frequency);
    reg.register(DLL, "GetSystemTime", get_system_time);
    reg.register(DLL, "GetLocalTime", get_local_time);
    reg.register(DLL, "GetSystemTimeAsFileTime", get_system_time_as_file_time);
    reg.register(DLL, "FileTimeToSystemTime", file_time_to_system_time);
    reg.register(DLL, "SystemTimeToFileTime", system_time_to_file_time);
    reg.register(DLL, "FileTimeToLocalFileTime", file_time_to_local_file_time);
    reg.register(DLL, "LocalFileTimeToFileTime", local_file_time_to_file_time);
    reg.register(DLL, "FileTimeToDosDateTime", file_time_to_dos_date_time);
    reg.register(DLL, "DosDateTimeToFileTime", dos_date_time_to_file_time);
    reg.register(DLL, "CompareFileTime", compare_file_time);
    reg.register(DLL, "GetTimeZoneInformation", get_time_zone_information);
}

#[derive(Default)]
struct QpcState {
    start: Option<Instant>,
    last: u64,
}

fn get_tick_count(m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(m.now_ms() as u32, 0)
}

fn query_performance_counter(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let st = m.state::<QpcState>();
    let v = {
        let mut s = st.borrow_mut();
        let start = *s.start.get_or_insert_with(Instant::now);
        let mut v = start.elapsed().as_micros() as u64;
        if v <= s.last {
            v = s.last + 1;
        }
        s.last = v;
        v
    };
    if p == 0 {
        return ThunkResult::stdcall(0, 1);
    }
    m.mem.write_u64(p, v);
    ThunkResult::stdcall(1, 1)
}

fn query_performance_frequency(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p == 0 {
        return ThunkResult::stdcall(0, 1);
    }
    m.mem.write_u64(p, 1_000_000);
    ThunkResult::stdcall(1, 1)
}

// ---------------------------------------------------------------------------------------------- civil time

/// Days since 1970-01-01 -> (year, month 1..12, day 1..31). (H. Hinnant's algorithm.)
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day) -> days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// SYSTEMTIME fields (wYear, wMonth, wDayOfWeek, wDay, wHour, wMinute, wSecond, wMilliseconds).
pub fn filetime_to_systemtime(ft: u64) -> [u16; 8] {
    let ms_total = (ft / 10_000) as i64 - (s3_runtime::hosttime::FILETIME_UNIX_EPOCH / 10_000) as i64;
    let days = ms_total.div_euclid(86_400_000);
    let rem = ms_total.rem_euclid(86_400_000);
    let (y, mo, d) = civil_from_days(days);
    let dow = (days + 4).rem_euclid(7); // 1970-01-01 was a Thursday (4)
    [
        y as u16,
        mo as u16,
        dow as u16,
        d as u16,
        (rem / 3_600_000) as u16,
        (rem / 60_000 % 60) as u16,
        (rem / 1000 % 60) as u16,
        (rem % 1000) as u16,
    ]
}

/// SYSTEMTIME -> FILETIME (None if out of range).
pub fn systemtime_to_filetime(st: &[u16; 8]) -> Option<u64> {
    let (y, mo, d, h, mi, s, ms) = (st[0], st[1], st[3], st[4], st[5], st[6], st[7]);
    if !(1601..=30827).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 || ms > 999 {
        return None;
    }
    let days = days_from_civil(y as i64, mo as u32, d as u32);
    let ms_total = days * 86_400_000 + h as i64 * 3_600_000 + mi as i64 * 60_000 + s as i64 * 1000 + ms as i64;
    let ft = (ms_total + (s3_runtime::hosttime::FILETIME_UNIX_EPOCH / 10_000) as i64) * 10_000;
    (ft >= 0).then_some(ft as u64)
}

fn write_st(m: &mut Machine, p: u32, st: &[u16; 8]) {
    for (i, v) in st.iter().enumerate() {
        m.mem.write_u16(p + 2 * i as u32, *v);
    }
}

fn read_st(m: &Machine, p: u32) -> [u16; 8] {
    let mut st = [0u16; 8];
    for (i, v) in st.iter_mut().enumerate() {
        *v = m.mem.read_u16(p + 2 * i as u32);
    }
    st
}

fn local_offset_ft() -> i64 {
    utc_offset_secs() as i64 * 10_000_000
}

fn get_system_time(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let st = filetime_to_systemtime(now_filetime());
    write_st(m, p, &st);
    ThunkResult::stdcall(0, 1)
}

fn get_local_time(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let st = filetime_to_systemtime((now_filetime() as i64 + local_offset_ft()) as u64);
    write_st(m, p, &st);
    ThunkResult::stdcall(0, 1)
}

fn get_system_time_as_file_time(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    m.mem.write_u64(p, now_filetime());
    ThunkResult::stdcall(0, 1)
}

fn file_time_to_system_time(m: &mut Machine) -> ThunkResult {
    let (f, s) = (m.arg_u32(0), m.arg_u32(1));
    let ft = m.mem.read_u64(f);
    if ft >= 0x8000_0000_0000_0000 {
        m.set_last_error(87);
        return ThunkResult::stdcall(0, 2);
    }
    let st = filetime_to_systemtime(ft);
    write_st(m, s, &st);
    ThunkResult::stdcall(1, 2)
}

fn system_time_to_file_time(m: &mut Machine) -> ThunkResult {
    let (s, f) = (m.arg_u32(0), m.arg_u32(1));
    let st = read_st(m, s);
    match systemtime_to_filetime(&st) {
        Some(ft) => {
            m.mem.write_u64(f, ft);
            ThunkResult::stdcall(1, 2)
        }
        None => {
            m.set_last_error(87);
            ThunkResult::stdcall(0, 2)
        }
    }
}

fn file_time_to_local_file_time(m: &mut Machine) -> ThunkResult {
    let (a, b) = (m.arg_u32(0), m.arg_u32(1));
    let ft = m.mem.read_u64(a) as i64 + local_offset_ft();
    m.mem.write_u64(b, ft.max(0) as u64);
    ThunkResult::stdcall(1, 2)
}

fn local_file_time_to_file_time(m: &mut Machine) -> ThunkResult {
    let (a, b) = (m.arg_u32(0), m.arg_u32(1));
    let ft = m.mem.read_u64(a) as i64 - local_offset_ft();
    m.mem.write_u64(b, ft.max(0) as u64);
    ThunkResult::stdcall(1, 2)
}

fn file_time_to_dos_date_time(m: &mut Machine) -> ThunkResult {
    let (f, pd, pt) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let st = filetime_to_systemtime(m.mem.read_u64(f));
    if !(1980..=2107).contains(&st[0]) {
        m.set_last_error(87);
        return ThunkResult::stdcall(0, 3);
    }
    let date = ((st[0] - 1980) << 9) | (st[1] << 5) | st[3];
    let time = (st[4] << 11) | (st[5] << 5) | (st[6] / 2);
    m.mem.write_u16(pd, date);
    m.mem.write_u16(pt, time);
    ThunkResult::stdcall(1, 3)
}

fn dos_date_time_to_file_time(m: &mut Machine) -> ThunkResult {
    let (date, time, f) = (m.arg_u32(0) as u16, m.arg_u32(1) as u16, m.arg_u32(2));
    let st = [(date >> 9) + 1980, (date >> 5) & 15, 0, date & 31, time >> 11, (time >> 5) & 63, (time & 31) * 2, 0];
    match systemtime_to_filetime(&st) {
        Some(ft) => {
            m.mem.write_u64(f, ft);
            ThunkResult::stdcall(1, 3)
        }
        None => ThunkResult::stdcall(0, 3),
    }
}

fn compare_file_time(m: &mut Machine) -> ThunkResult {
    let (a, b) = (m.mem.read_u64(m.arg_u32(0)), m.mem.read_u64(m.arg_u32(1)));
    ThunkResult::stdcall(a.cmp(&b) as i32 as u32, 2)
}

/// `GetTimeZoneInformation(TIME_ZONE_INFORMATION*)` (172 bytes). Returns TIME_ZONE_ID_UNKNOWN (0).
fn get_time_zone_information(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    m.mem.fill(p, 172, 0);
    let bias = -(utc_offset_secs() / 60);
    m.mem.write_u32(p, bias as u32);
    m.mem.write_wstr(p + 4, "Local Standard Time");
    m.mem.write_wstr(p + 88, "Local Daylight Time");
    ThunkResult::stdcall(0, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip() {
        // 2026-11-05 12:34:56.789 UTC
        let st = [2026, 11, 4, 5, 12, 34, 56, 789];
        let ft = systemtime_to_filetime(&st).unwrap();
        assert_eq!(filetime_to_systemtime(ft), st); // 2026-11-05 is a Thursday (4)
        assert_eq!(filetime_to_systemtime(0)[..2], [1601, 1]);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
