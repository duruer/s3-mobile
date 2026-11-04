//! kernel32: code pages, locale and string functions (MultiByteToWideChar, GetLocaleInfo, LCMapString,
//! GetStringType, CompareString, lstr*).
//!
//! The guest runs in locale 0x0409 (English, US) with ANSI code page 1252 and OEM code page 437
//! (HYPOTHESIS: the game picks its text language from the registry value `Language`, not from the locale).
//! All ANSI<->UTF-16 conversions use Windows-1252 regardless of the code page argument (UTF-8 = 65001 is
//! honoured); every byte is a single character (no DBCS lead bytes).

use s3_runtime::{Machine, Registry, ThunkResult};

use super::DLL;

pub const LCID_DEFAULT: u32 = 0x0409;
pub const ACP: u32 = 1252;
pub const OEMCP: u32 = 437;
const CP_UTF8: u32 = 65001;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_FLAGS: u32 = 1004;

pub(crate) fn register(reg: &mut Registry) {
    reg.register(DLL, "GetACP", |_m| ThunkResult::stdcall(ACP, 0));
    reg.register(DLL, "GetOEMCP", |_m| ThunkResult::stdcall(OEMCP, 0));
    reg.register(DLL, "GetCPInfo", get_cp_info);
    reg.register(DLL, "IsValidCodePage", is_valid_code_page);
    reg.register(DLL, "GetUserDefaultLCID", |_m| ThunkResult::stdcall(LCID_DEFAULT, 0));
    reg.register(DLL, "GetSystemDefaultLCID", |_m| ThunkResult::stdcall(LCID_DEFAULT, 0));
    reg.register(DLL, "GetUserDefaultLangID", |_m| ThunkResult::stdcall(LCID_DEFAULT, 0));
    reg.register(DLL, "GetSystemDefaultLangID", |_m| ThunkResult::stdcall(LCID_DEFAULT, 0));
    reg.register(DLL, "GetThreadLocale", |_m| ThunkResult::stdcall(LCID_DEFAULT, 0));
    reg.register(DLL, "IsValidLocale", |_m| ThunkResult::stdcall(1, 2));
    reg.register(DLL, "EnumSystemLocalesA", enum_system_locales_a);
    reg.register(DLL, "GetLocaleInfoA", get_locale_info_a);
    reg.register(DLL, "GetLocaleInfoW", get_locale_info_w);
    reg.register(DLL, "MultiByteToWideChar", multi_byte_to_wide_char);
    reg.register(DLL, "WideCharToMultiByte", wide_char_to_multi_byte);
    reg.register(DLL, "GetStringTypeA", get_string_type_a);
    reg.register(DLL, "GetStringTypeW", get_string_type_w);
    reg.register(DLL, "LCMapStringA", lc_map_string_a);
    reg.register(DLL, "LCMapStringW", lc_map_string_w);
    reg.register(DLL, "CompareStringA", compare_string_a);
    reg.register(DLL, "CompareStringW", compare_string_w);
    reg.register(DLL, "lstrlenA", lstrlen_a);
    reg.register(DLL, "lstrlenW", lstrlen_w);
    reg.register(DLL, "lstrcpyA", lstrcpy_a);
    reg.register(DLL, "lstrcpynA", lstrcpyn_a);
    reg.register(DLL, "lstrcatA", lstrcat_a);
    reg.register(DLL, "lstrcmpA", lstrcmp_a);
    reg.register(DLL, "lstrcmpiA", lstrcmpi_a);
}

// ---------------------------------------------------------------------------------------------- cp1252

/// Windows-1252 bytes 0x80..0x9F (0 = undefined, mapped to the C1 control of the same value like Windows).
const CP1252_80: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D,
    0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
    0x0153, 0x009D, 0x017E, 0x0178,
];

/// One cp1252 byte -> UTF-16 unit.
pub fn cp1252_to_u16(b: u8) -> u16 {
    if (0x80..0xA0).contains(&b) {
        CP1252_80[(b - 0x80) as usize]
    } else {
        b as u16
    }
}

/// One UTF-16 unit -> cp1252 byte (`None` if not representable).
pub fn u16_to_cp1252(u: u16) -> Option<u8> {
    if u < 0x80 || (0xA0..=0xFF).contains(&u) {
        return Some(u as u8);
    }
    CP1252_80.iter().position(|&c| c == u).map(|i| 0x80 + i as u8)
}

fn decode(cp: u32, bytes: &[u8]) -> Vec<u16> {
    if cp == CP_UTF8 {
        String::from_utf8_lossy(bytes).encode_utf16().collect()
    } else {
        bytes.iter().map(|&b| cp1252_to_u16(b)).collect()
    }
}

/// Encode; returns (bytes, used_default_char).
fn encode(cp: u32, units: &[u16], default: u8) -> (Vec<u8>, bool) {
    if cp == CP_UTF8 {
        return (String::from_utf16_lossy(units).into_bytes(), false);
    }
    let mut used = false;
    let v = units
        .iter()
        .map(|&u| {
            u16_to_cp1252(u).unwrap_or_else(|| {
                used = true;
                default
            })
        })
        .collect();
    (v, used)
}

/// Read an ANSI string argument with explicit length (`-1` = NUL-terminated, NUL included).
fn read_a(m: &Machine, p: u32, n: i32) -> Vec<u8> {
    if p == 0 {
        return Vec::new();
    }
    if n < 0 {
        let mut v = m.mem.read_cstr_bytes(p);
        v.push(0);
        v
    } else {
        m.mem.read_bytes(p, n as usize)
    }
}

/// Read a UTF-16 string argument with explicit length (`-1` = NUL-terminated, NUL included).
fn read_w(m: &Machine, p: u32, n: i32) -> Vec<u16> {
    if p == 0 {
        return Vec::new();
    }
    if n < 0 {
        let mut v = Vec::new();
        let mut a = p;
        loop {
            let u = m.mem.read_u16(a);
            v.push(u);
            if u == 0 || v.len() > 0x10_0000 {
                break;
            }
            a += 2;
        }
        v
    } else {
        (0..n as u32).map(|i| m.mem.read_u16(p + 2 * i)).collect()
    }
}

fn write_w(m: &mut Machine, p: u32, units: &[u16]) {
    for (i, &u) in units.iter().enumerate() {
        m.mem.write_u16(p + 2 * i as u32, u);
    }
}

fn get_cp_info(m: &mut Machine) -> ThunkResult {
    let (cp, info) = (m.arg_u32(0), m.arg_u32(1));
    if !valid_cp(cp) || info == 0 {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(0, 2);
    }
    // CPINFO { UINT MaxCharSize; BYTE DefaultChar[2]; BYTE LeadByte[12]; }
    m.mem.fill(info, 20, 0);
    m.mem.write_u32(info, if cp == CP_UTF8 { 4 } else { 1 });
    m.mem.write_u8(info + 4, b'?');
    ThunkResult::stdcall(1, 2)
}

fn valid_cp(cp: u32) -> bool {
    matches!(cp, 0 | 1 | 2 | 3 | 437 | 850 | 1252 | 28591 | 20127 | CP_UTF8)
}

fn is_valid_code_page(m: &mut Machine) -> ThunkResult {
    let cp = m.arg_u32(0);
    ThunkResult::stdcall(valid_cp(cp) as u32, 1)
}

fn enum_system_locales_a(m: &mut Machine) -> ThunkResult {
    let cb = m.arg_u32(0);
    if cb != 0 {
        let s = m.mem.alloc(16);
        m.mem.write_cstr(s, "00000409");
        m.call_guest(cb, &[s]);
        m.mem.free(s);
    }
    ThunkResult::stdcall(1, 2)
}

// ---------------------------------------------------------------------------------------------- locale info

const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

/// `GetLocaleInfo` data for en-US (LCTYPE without flags).
pub fn locale_info(lctype: u32) -> Option<String> {
    let s = match lctype {
        0x01 => "0409",                        // LOCALE_ILANGUAGE
        0x02 => "English (United States)",     // LOCALE_SLANGUAGE
        0x03 => "ENU",                         // LOCALE_SABBREVLANGNAME
        0x04 => "English",                     // LOCALE_SNATIVELANGNAME
        0x05 => "1",                           // LOCALE_ICOUNTRY
        0x06 => "United States",               // LOCALE_SCOUNTRY
        0x07 => "USA",                         // LOCALE_SABBREVCTRYNAME
        0x08 => "United States",               // LOCALE_SNATIVECTRYNAME
        0x09 => "0409",                        // LOCALE_IDEFAULTLANGUAGE
        0x0A => "1",                           // LOCALE_IDEFAULTCOUNTRY
        0x0B => "437",                         // LOCALE_IDEFAULTCODEPAGE (OEM)
        0x0C => ",",                           // LOCALE_SLIST
        0x0D => "1",                           // LOCALE_IMEASURE (US)
        0x0E => ".",                           // LOCALE_SDECIMAL
        0x0F => ",",                           // LOCALE_STHOUSAND
        0x10 => "3;0",                         // LOCALE_SGROUPING
        0x11 => "2",                           // LOCALE_IDIGITS
        0x12 => "1",                           // LOCALE_ILZERO
        0x13 => "0123456789",                  // LOCALE_SNATIVEDIGITS
        0x14 => "$",                           // LOCALE_SCURRENCY
        0x15 => "USD",                         // LOCALE_SINTLSYMBOL
        0x16 => ".",                           // LOCALE_SMONDECIMALSEP
        0x17 => ",",                           // LOCALE_SMONTHOUSANDSEP
        0x18 => "3;0",                         // LOCALE_SMONGROUPING
        0x19 => "2",                           // LOCALE_ICURRDIGITS
        0x1A => "2",                           // LOCALE_IINTLCURRDIGITS
        0x1B => "0",                           // LOCALE_ICURRENCY
        0x1C => "0",                           // LOCALE_INEGCURR
        0x1D => "/",                           // LOCALE_SDATE
        0x1E => ":",                           // LOCALE_STIME
        0x1F => "M/d/yyyy",                    // LOCALE_SSHORTDATE
        0x20 => "dddd, MMMM dd, yyyy",         // LOCALE_SLONGDATE
        0x21 => "0",                           // LOCALE_IDATE
        0x22 => "0",                           // LOCALE_ILDATE
        0x23 => "0",                           // LOCALE_ITIME
        0x24 => "1",                           // LOCALE_ICENTURY
        0x25 => "0",                           // LOCALE_ITLZERO
        0x26 => "0",                           // LOCALE_IDAYLZERO
        0x27 => "0",                           // LOCALE_IMONLZERO
        0x28 => "AM",                          // LOCALE_S1159
        0x29 => "PM",                          // LOCALE_S2359
        0x2A..=0x30 => DAYS[(lctype - 0x2A) as usize],
        0x31..=0x37 => &DAYS[(lctype - 0x31) as usize][..3],
        0x38..=0x43 => MONTHS[(lctype - 0x38) as usize],
        0x44..=0x4F => &MONTHS[(lctype - 0x44) as usize][..3],
        0x50 => "",                            // LOCALE_SPOSITIVESIGN
        0x51 => "-",                           // LOCALE_SNEGATIVESIGN
        0x52..=0x57 => "0",                    // sign positions etc.
        0x1001 => "English",                   // LOCALE_SENGLANGUAGE
        0x1002 => "United States",             // LOCALE_SENGCOUNTRY
        0x1003 => "h:mm:ss tt",                // LOCALE_STIMEFORMAT
        0x1004 => "1252",                      // LOCALE_IDEFAULTANSICODEPAGE
        0x1005 => "0",                         // LOCALE_ITIMEMARKPOSN
        0x1007 => "Dollar",                    // LOCALE_SENGCURRNAME
        0x1008 => "US Dollar",                 // LOCALE_SNATIVECURRNAME
        0x1009 => "1",                         // LOCALE_ICALENDARTYPE
        0x100A => "0",                         // LOCALE_IOPTIONALCALENDAR
        0x100B => "0",                         // LOCALE_IFIRSTDAYOFWEEK (Monday=0... en-US: 6) — keep 6 below
        0x100C => "0",                         // LOCALE_IFIRSTWEEKOFYEAR
        0x1014 => "-",                         // LOCALE_SNEGATIVESIGN (alt)
        0x1016 => "MMMM, yyyy",                // LOCALE_SYEARMONTH
        0x1018 => "1",                         // LOCALE_IDEFAULTEBCDICCODEPAGE (unused)
        0x1059 | 0x105A => "",                 // abbreviated names (Vista+)
        _ => return None,
    };
    if lctype == 0x100B {
        return Some("6".into());
    }
    Some(s.to_string())
}

const LOCALE_RETURN_NUMBER: u32 = 0x2000_0000;

fn get_locale_info(m: &mut Machine, wide: bool) -> ThunkResult {
    let (_lcid, lctype, buf, n) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3));
    let ty = lctype & 0x0FFF_FFFF & !0x8000_0000;
    let Some(s) = locale_info(ty & 0xFFFF) else {
        m.log(s3_runtime::LogLevel::Debug, &format!("GetLocaleInfo: unknown LCTYPE {lctype:#x}"));
        m.set_last_error(ERROR_INVALID_FLAGS);
        return ThunkResult::stdcall(0, 4);
    };
    if lctype & LOCALE_RETURN_NUMBER != 0 {
        let v: u32 = s.parse().unwrap_or(0);
        let need = if wide { 2 } else { 4 };
        if n == 0 {
            return ThunkResult::stdcall(need, 4);
        }
        if (n as u32) < need {
            m.set_last_error(ERROR_INSUFFICIENT_BUFFER);
            return ThunkResult::stdcall(0, 4);
        }
        m.mem.write_u32(buf, v);
        return ThunkResult::stdcall(need, 4);
    }
    let need = s.chars().count() as u32 + 1;
    if n == 0 {
        return ThunkResult::stdcall(need, 4);
    }
    if (n as u32) < need {
        m.set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return ThunkResult::stdcall(0, 4);
    }
    if wide {
        m.mem.write_wstr(buf, &s);
    } else {
        m.mem.write_cstr(buf, &s);
    }
    ThunkResult::stdcall(need, 4)
}

fn get_locale_info_a(m: &mut Machine) -> ThunkResult {
    get_locale_info(m, false)
}

fn get_locale_info_w(m: &mut Machine) -> ThunkResult {
    get_locale_info(m, true)
}

// ---------------------------------------------------------------------------------------------- conversions

/// `MultiByteToWideChar(cp, flags, src, cb, dst, cch)`.
fn multi_byte_to_wide_char(m: &mut Machine) -> ThunkResult {
    let (cp, _flags, src, cb, dst, cch) =
        (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3), m.arg_u32(4), m.arg_i32(5));
    if src == 0 || cb == 0 || cch < 0 {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(0, 6);
    }
    let units = decode(cp, &read_a(m, src, cb));
    if cch == 0 {
        return ThunkResult::stdcall(units.len() as u32, 6);
    }
    if units.len() > cch as usize {
        write_w(m, dst, &units[..cch as usize]);
        m.set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return ThunkResult::stdcall(0, 6);
    }
    write_w(m, dst, &units);
    ThunkResult::stdcall(units.len() as u32, 6)
}

/// `WideCharToMultiByte(cp, flags, src, cch, dst, cb, default_char, used_default)`.
fn wide_char_to_multi_byte(m: &mut Machine) -> ThunkResult {
    let (cp, _flags, src, cch, dst, cb, defp, usedp) = (
        m.arg_u32(0),
        m.arg_u32(1),
        m.arg_u32(2),
        m.arg_i32(3),
        m.arg_u32(4),
        m.arg_i32(5),
        m.arg_u32(6),
        m.arg_u32(7),
    );
    if src == 0 || cch == 0 || cb < 0 {
        m.set_last_error(ERROR_INVALID_PARAMETER);
        return ThunkResult::stdcall(0, 8);
    }
    let default = if defp != 0 { m.mem.read_u8(defp) } else { b'?' };
    let (bytes, used) = encode(cp, &read_w(m, src, cch), default);
    if usedp != 0 {
        m.mem.write_u32(usedp, used as u32);
    }
    if cb == 0 {
        return ThunkResult::stdcall(bytes.len() as u32, 8);
    }
    if bytes.len() > cb as usize {
        m.mem.write_bytes(dst, &bytes[..cb as usize]);
        m.set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return ThunkResult::stdcall(0, 8);
    }
    m.mem.write_bytes(dst, &bytes);
    ThunkResult::stdcall(bytes.len() as u32, 8)
}

// ---------------------------------------------------------------------------------------------- char types

const C1_UPPER: u16 = 0x1;
const C1_LOWER: u16 = 0x2;
const C1_DIGIT: u16 = 0x4;
const C1_SPACE: u16 = 0x8;
const C1_PUNCT: u16 = 0x10;
const C1_CNTRL: u16 = 0x20;
const C1_BLANK: u16 = 0x40;
const C1_XDIGIT: u16 = 0x80;
const C1_ALPHA: u16 = 0x100;

/// CT_CTYPE1 flags of a UTF-16 unit.
pub fn ctype1(u: u16) -> u16 {
    let Some(c) = char::from_u32(u as u32) else { return 0 };
    let mut f = 0;
    if c.is_uppercase() {
        f |= C1_UPPER | C1_ALPHA;
    }
    if c.is_lowercase() {
        f |= C1_LOWER | C1_ALPHA;
    }
    if c.is_alphabetic() {
        f |= C1_ALPHA;
    }
    if c.is_ascii_digit() {
        f |= C1_DIGIT;
    }
    if c.is_ascii_hexdigit() {
        f |= C1_XDIGIT;
    }
    if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') || u == 0xA0 {
        f |= C1_SPACE;
    }
    if c == ' ' || c == '\t' || u == 0xA0 {
        f |= C1_BLANK;
    }
    if c.is_control() {
        f |= C1_CNTRL;
    }
    if !c.is_control() && !c.is_alphanumeric() && !c.is_whitespace() && u != 0xA0 && u != 0 {
        f |= C1_PUNCT;
    }
    f
}

fn ctype(info: u32, u: u16) -> u16 {
    match info {
        1 => ctype1(u),
        2 => {
            // CT_CTYPE2: C2_LEFTTORIGHT for letters/digits, C2_WHITESPACE, C2_OTHERNEUTRAL
            let c1 = ctype1(u);
            if c1 & (C1_ALPHA | C1_DIGIT) != 0 {
                1
            } else if c1 & C1_SPACE != 0 {
                0xA
            } else {
                0xB
            }
        }
        _ => 0,
    }
}

/// `GetStringTypeA(lcid, info, src, cch, out)`.
fn get_string_type_a(m: &mut Machine) -> ThunkResult {
    let (_lcid, info, src, cch, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3), m.arg_u32(4));
    let mut bytes = read_a(m, src, cch);
    if cch < 0 {
        bytes.pop();
        bytes.push(0);
    }
    for (i, b) in bytes.iter().enumerate() {
        let v = ctype(info, cp1252_to_u16(*b));
        m.mem.write_u16(out + 2 * i as u32, v);
    }
    ThunkResult::stdcall(1, 5)
}

/// `GetStringTypeW(info, src, cch, out)`.
fn get_string_type_w(m: &mut Machine) -> ThunkResult {
    let (info, src, cch, out) = (m.arg_u32(0), m.arg_u32(1), m.arg_i32(2), m.arg_u32(3));
    let units = read_w(m, src, cch);
    for (i, u) in units.iter().enumerate() {
        let v = ctype(info, *u);
        m.mem.write_u16(out + 2 * i as u32, v);
    }
    ThunkResult::stdcall(1, 4)
}

// ---------------------------------------------------------------------------------------------- LCMapString

const LCMAP_LOWERCASE: u32 = 0x100;
const LCMAP_UPPERCASE: u32 = 0x200;
const LCMAP_SORTKEY: u32 = 0x400;
const NORM_IGNORECASE: u32 = 0x1;

fn upper(u: u16) -> u16 {
    match char::from_u32(u as u32) {
        Some(c) => {
            let mut it = c.to_uppercase();
            match (it.next(), it.next()) {
                (Some(x), None) if (x as u32) < 0x10000 => x as u16,
                _ => u,
            }
        }
        None => u,
    }
}

fn lower(u: u16) -> u16 {
    match char::from_u32(u as u32) {
        Some(c) => {
            let mut it = c.to_lowercase();
            match (it.next(), it.next()) {
                (Some(x), None) if (x as u32) < 0x10000 => x as u16,
                _ => u,
            }
        }
        None => u,
    }
}

/// Map units; for LCMAP_SORTKEY returns a byte key (case-insensitive when NORM_IGNORECASE).
fn lcmap(flags: u32, units: &[u16]) -> Result<Vec<u16>, Vec<u8>> {
    if flags & LCMAP_SORTKEY != 0 {
        let mut key = Vec::new();
        for &u in units {
            if u == 0 {
                break;
            }
            let k = if flags & NORM_IGNORECASE != 0 { upper(u) } else { u };
            key.push((k >> 8) as u8 + 1);
            key.push(k as u8);
        }
        key.push(0);
        return Err(key);
    }
    Ok(units
        .iter()
        .map(|&u| {
            if flags & LCMAP_UPPERCASE != 0 {
                upper(u)
            } else if flags & LCMAP_LOWERCASE != 0 {
                lower(u)
            } else {
                u
            }
        })
        .collect())
}

/// `LCMapStringA(lcid, flags, src, cch, dst, cchDest)`.
fn lc_map_string_a(m: &mut Machine) -> ThunkResult {
    let (_lcid, flags, src, cch, dst, cdst) =
        (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3), m.arg_u32(4), m.arg_i32(5));
    let units = decode(ACP, &read_a(m, src, cch));
    let out: Vec<u8> = match lcmap(flags, &units) {
        Ok(u) => encode(ACP, &u, b'?').0,
        Err(key) => key,
    };
    finish_bytes(m, dst, cdst, &out, 6)
}

fn finish_bytes(m: &mut Machine, dst: u32, cdst: i32, out: &[u8], nargs: u32) -> ThunkResult {
    if cdst == 0 {
        return ThunkResult::stdcall(out.len() as u32, nargs);
    }
    if out.len() > cdst as usize {
        m.set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return ThunkResult::stdcall(0, nargs);
    }
    m.mem.write_bytes(dst, out);
    ThunkResult::stdcall(out.len() as u32, nargs)
}

/// `LCMapStringW(lcid, flags, src, cch, dst, cchDest)` (sort keys are bytes, `cchDest` counts bytes then).
fn lc_map_string_w(m: &mut Machine) -> ThunkResult {
    let (_lcid, flags, src, cch, dst, cdst) =
        (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3), m.arg_u32(4), m.arg_i32(5));
    let units = read_w(m, src, cch);
    match lcmap(flags, &units) {
        Err(key) => finish_bytes(m, dst, cdst, &key, 6),
        Ok(u) => {
            if cdst == 0 {
                return ThunkResult::stdcall(u.len() as u32, 6);
            }
            if u.len() > cdst as usize {
                m.set_last_error(ERROR_INSUFFICIENT_BUFFER);
                return ThunkResult::stdcall(0, 6);
            }
            write_w(m, dst, &u);
            ThunkResult::stdcall(u.len() as u32, 6)
        }
    }
}

// ---------------------------------------------------------------------------------------------- compare

fn compare(flags: u32, a: &[u16], b: &[u16]) -> u32 {
    let strip = |v: &[u16]| -> Vec<u16> {
        let end = v.iter().position(|&u| u == 0).unwrap_or(v.len());
        v[..end].iter().map(|&u| if flags & NORM_IGNORECASE != 0 { upper(u) } else { u }).collect()
    };
    match strip(a).cmp(&strip(b)) {
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Equal => 2,
        std::cmp::Ordering::Greater => 3,
    }
}

/// `CompareStringA(lcid, flags, s1, c1, s2, c2)` -> CSTR_LESS_THAN 1 / CSTR_EQUAL 2 / CSTR_GREATER_THAN 3.
fn compare_string_a(m: &mut Machine) -> ThunkResult {
    let (_l, flags, s1, c1, s2, c2) =
        (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3), m.arg_u32(4), m.arg_i32(5));
    let a = decode(ACP, &read_a(m, s1, c1));
    let b = decode(ACP, &read_a(m, s2, c2));
    ThunkResult::stdcall(compare(flags, &a, &b), 6)
}

fn compare_string_w(m: &mut Machine) -> ThunkResult {
    let (_l, flags, s1, c1, s2, c2) =
        (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_i32(3), m.arg_u32(4), m.arg_i32(5));
    let a = read_w(m, s1, c1);
    let b = read_w(m, s2, c2);
    ThunkResult::stdcall(compare(flags, &a, &b), 6)
}

// ---------------------------------------------------------------------------------------------- lstr*

fn lstrlen_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let n = if p < 0x1_0000 { 0 } else { m.mem.strlen(p, 0x100_0000) as u32 };
    ThunkResult::stdcall(n, 1)
}

fn lstrlen_w(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let n = if p < 0x1_0000 { 0 } else { read_w(m, p, -1).len() as u32 - 1 };
    ThunkResult::stdcall(n, 1)
}

fn lstrcpy_a(m: &mut Machine) -> ThunkResult {
    let (d, s) = (m.arg_u32(0), m.arg_u32(1));
    if d < 0x1_0000 || s < 0x1_0000 {
        return ThunkResult::stdcall(0, 2);
    }
    let n = m.mem.strlen(s, 0x100_0000);
    m.mem.copy_within(s, d, n + 1);
    ThunkResult::stdcall(d, 2)
}

fn lstrcpyn_a(m: &mut Machine) -> ThunkResult {
    let (d, s, n) = (m.arg_u32(0), m.arg_u32(1), m.arg_i32(2));
    if d < 0x1_0000 || s < 0x1_0000 || n <= 0 {
        return ThunkResult::stdcall(if n == 0 { d } else { 0 }, 3);
    }
    let len = m.mem.strlen(s, (n - 1) as usize);
    m.mem.copy_within(s, d, len);
    m.mem.write_u8(d + len as u32, 0);
    ThunkResult::stdcall(d, 3)
}

fn lstrcat_a(m: &mut Machine) -> ThunkResult {
    let (d, s) = (m.arg_u32(0), m.arg_u32(1));
    if d < 0x1_0000 || s < 0x1_0000 {
        return ThunkResult::stdcall(0, 2);
    }
    let dl = m.mem.strlen(d, 0x100_0000) as u32;
    let n = m.mem.strlen(s, 0x100_0000);
    m.mem.copy_within(s, d + dl, n + 1);
    ThunkResult::stdcall(d, 2)
}

fn lstrcmp_impl(m: &mut Machine, flags: u32) -> ThunkResult {
    let (s1, s2) = (m.arg_u32(0), m.arg_u32(1));
    let a = if s1 == 0 { vec![] } else { decode(ACP, &m.mem.read_cstr_bytes(s1)) };
    let b = if s2 == 0 { vec![] } else { decode(ACP, &m.mem.read_cstr_bytes(s2)) };
    let r = compare(flags, &a, &b) as i32 - 2;
    ThunkResult::stdcall(r as u32, 2)
}

fn lstrcmp_a(m: &mut Machine) -> ThunkResult {
    lstrcmp_impl(m, 0)
}

fn lstrcmpi_a(m: &mut Machine) -> ThunkResult {
    lstrcmp_impl(m, NORM_IGNORECASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp1252_roundtrip() {
        for b in 0u8..=255 {
            assert_eq!(u16_to_cp1252(cp1252_to_u16(b)), Some(b), "byte {b:#x}");
        }
        assert_eq!(cp1252_to_u16(0x80), 0x20AC);
        assert_eq!(u16_to_cp1252(0x4E00), None);
    }

    #[test]
    fn ctype_flags() {
        assert_eq!(ctype1(b'A' as u16) & (C1_UPPER | C1_ALPHA | C1_XDIGIT), C1_UPPER | C1_ALPHA | C1_XDIGIT);
        assert_eq!(ctype1(b'7' as u16), C1_DIGIT | C1_XDIGIT);
        assert_eq!(ctype1(b' ' as u16), C1_SPACE | C1_BLANK);
        assert_eq!(ctype1(b',' as u16), C1_PUNCT);
        assert_eq!(ctype1(0xE4) & C1_LOWER, C1_LOWER); // ä
        assert_eq!(compare(NORM_IGNORECASE, &[b'a' as u16], &[b'A' as u16]), 2);
        assert_eq!(locale_info(0x1004).as_deref(), Some("1252"));
    }
}
