//! The game's ANSI code page: which Windows code page its text bytes are in.
//!
//! S3 ships one string table per language and draws its text with GDI. The bytes are in the code page of the
//! Windows the localised version was made for: Windows-1252 for German/English/French/Italian/Spanish,
//! Windows-1250 for Polish/Czech/Hungarian, Windows-1251 for Russian. Decoding a Polish string as 1252 turns
//! `ą ł ż` into `¹ ³ ¿` (GitHub issue #1).
//!
//! The registry value `Language` does not say which one: the GOG `S3.EXE` maps only 7/9/12/15 to text slots
//! 0/1/3/4 (`0x40f2ab`), so a localised install may put its own language into one of those slots. So the code
//! page is **detected from the text the game draws** ([`observe`], fed by the GDI text functions and the mods'
//! string reads), with the registry LANGID as the starting guess ([`set_language_hint`]) and `S3_CODEPAGE=<n>` as
//! an override. Detection:
//! - mostly bytes >= 0xC0 instead of ASCII letters -> 1251 (Cyrillic);
//! - at least [`EV_MIN`] bytes inside words that are letters in 1250 but symbols or rare letters in 1252
//!   (`¹ ³ ¥ £ ¯ Ÿ ¾ ¼ ø õ` = `ą ł Ą Ł Ż ź ľ Ľ ř ő`, plus 0x8D/0x8F which 1252 leaves undefined) -> 1250;
//! - otherwise the hint (default 1252).
//!
//! All state is process-global (one game per process). It only changes how text is decoded for display and for
//! UTF-16 conversions, never game logic.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::collections::BTreeSet;
use std::sync::Mutex;

/// Code pages with a table here.
pub const SUPPORTED: [u32; 5] = [1250, 1251, 1252, 1253, 1254];

/// Evidence bytes needed before the text is taken as 1250.
pub const EV_MIN: u32 = 3;

static ACTIVE: AtomicU32 = AtomicU32::new(1252);
static HINT: AtomicU32 = AtomicU32::new(1252);
/// `S3_CODEPAGE` (0 = not read yet, 1 = none).
static FORCED: AtomicU32 = AtomicU32::new(0);
/// Bytes seen so far (to stop counting after enough text).
static SEEN: AtomicU64 = AtomicU64::new(0);

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub ascii_letters: u32,
    /// Bytes 0xC0..=0xFF.
    pub upper_high: u32,
    /// 1250 evidence bytes inside words.
    pub ev1250: u32,
}

static STATS: Mutex<Stats> = Mutex::new(Stats { ascii_letters: 0, upper_high: 0, ev1250: 0 });
/// Hashes of strings already counted (the game redraws the same text every frame).
static SEEN_STRINGS: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

/// Stop counting after this many bytes of distinct strings (the decision is settled long before).
const SEEN_LIMIT: u64 = 200_000;

fn forced() -> Option<u32> {
    let f = FORCED.load(Ordering::Relaxed);
    let f = if f == 0 {
        let v = std::env::var("S3_CODEPAGE").ok().and_then(|s| s.trim().parse::<u32>().ok()).filter(|c| SUPPORTED.contains(c));
        let v = v.unwrap_or(1);
        FORCED.store(v, Ordering::Relaxed);
        v
    } else {
        f
    };
    (f != 1).then_some(f)
}

/// The code page text is decoded with now.
pub fn active() -> u32 {
    forced().unwrap_or_else(|| ACTIVE.load(Ordering::Relaxed))
}

/// Code page for a Windows LANGID (primary language in the low 10 bits), if it is not 1252.
pub fn for_langid(langid: u32) -> u32 {
    match langid & 0x3ff {
        0x05 | 0x0E | 0x15 | 0x18 | 0x1A | 0x1B | 0x1C | 0x24 => 1250, // cs hu pl ro hr sk sq sl
        0x02 | 0x19 | 0x1C0 | 0x22 | 0x23 | 0x2F => 1251,              // bg ru uk be mk
        0x08 => 1253,                                                  // el
        0x1F => 1254,                                                  // tr
        _ => 1252,
    }
}

/// Starting guess from the registry value `Language` (a LANGID). Text seen later can override it.
pub fn set_language_hint(langid: u32) {
    HINT.store(for_langid(langid), Ordering::Relaxed);
    let s = *STATS.lock().unwrap();
    ACTIVE.store(decide(&s, HINT.load(Ordering::Relaxed)), Ordering::Relaxed);
}

/// Pure decision from the statistics.
pub fn decide(s: &Stats, hint: u32) -> u32 {
    if s.upper_high >= 20 && s.upper_high > s.ascii_letters * 2 {
        return 1251;
    }
    if s.ev1250 >= EV_MIN {
        return 1250;
    }
    hint
}

fn is_ev1250(b: u8) -> bool {
    matches!(b, 0xB9 | 0xB3 | 0xA5 | 0xA3 | 0xAF | 0x9F | 0x8F | 0x8D | 0xBE | 0xBC | 0xF8 | 0xD8 | 0xF5 | 0xD5)
}

fn is_wordish(b: u8) -> bool {
    b.is_ascii_alphabetic() || b >= 0xC0
}

/// Count the evidence in one string (pure, for tests).
pub fn count(bytes: &[u8], s: &mut Stats) {
    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_alphabetic() {
            s.ascii_letters += 1;
        } else if b >= 0xC0 {
            s.upper_high += 1;
        }
        if is_ev1250(b) {
            let prev = i.checked_sub(1).map(|j| bytes[j]);
            let next = bytes.get(i + 1).copied();
            if prev.is_some_and(is_wordish) || next.is_some_and(is_wordish) {
                s.ev1250 += 1;
            }
        }
    }
}

/// Feed game text (bytes as the game has them) into the detection.
pub fn observe(bytes: &[u8]) {
    if bytes.is_empty() || SEEN.load(Ordering::Relaxed) > SEEN_LIMIT {
        return;
    }
    // FNV-1a: count each distinct string once.
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    if !SEEN_STRINGS.lock().unwrap().insert(h) {
        return;
    }
    SEEN.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    let mut s = STATS.lock().unwrap();
    count(bytes, &mut s);
    let cp = decide(&s, HINT.load(Ordering::Relaxed));
    drop(s);
    let old = ACTIVE.swap(cp, Ordering::Relaxed);
    if old != cp && forced().is_none() {
        eprintln!("codepage: game text looks like Windows-{cp} (was {old})");
    }
}

/// Forget everything (tests, or a new game in the same process).
pub fn reset() {
    *STATS.lock().unwrap() = Stats::default();
    SEEN_STRINGS.lock().unwrap().clear();
    SEEN.store(0, Ordering::Relaxed);
    HINT.store(1252, Ordering::Relaxed);
    ACTIVE.store(1252, Ordering::Relaxed);
}

fn table(cp: u32) -> &'static [u16; 128] {
    match cp {
        1250 => &CP1250,
        1251 => &CP1251,
        1253 => &CP1253,
        1254 => &CP1254,
        _ => &CP1252,
    }
}

/// One byte in code page `cp` -> UTF-16 unit (bytes the code page leaves undefined map to U+0080..U+00FF of the
/// same value, like Windows).
pub fn byte_to_u16_in(cp: u32, b: u8) -> u16 {
    if b < 0x80 {
        b as u16
    } else {
        table(cp)[(b - 0x80) as usize]
    }
}

/// One UTF-16 unit -> byte in code page `cp` (`None` if not representable).
pub fn u16_to_byte_in(cp: u32, u: u16) -> Option<u8> {
    if u < 0x80 {
        return Some(u as u8);
    }
    table(cp).iter().position(|&c| c == u).map(|i| 0x80 + i as u8)
}

/// One byte of game text -> char, in the active code page.
pub fn decode_byte(b: u8) -> char {
    char::from_u32(byte_to_u16_in(active(), b) as u32).unwrap_or('?')
}

/// Game text bytes -> String, in the active code page.
pub fn decode(bytes: &[u8]) -> String {
    let cp = active();
    bytes.iter().map(|&b| char::from_u32(byte_to_u16_in(cp, b) as u32).unwrap_or('?')).collect()
}

/// A typed character -> game byte in the active code page (`None` if the game cannot show it).
pub fn encode_char(c: char) -> Option<u8> {
    let u = c as u32;
    if u > 0xFFFF {
        return None;
    }
    u16_to_byte_in(active(), u as u16)
}

const CP1250: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0083, 0x201E, 0x2026, 0x2020, 0x2021, 0x0088, 0x2030, 0x0160, 0x2039, 0x015A, 0x0164, 0x017D, 0x0179,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x0098, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A,
    0x00A0, 0x02C7, 0x02D8, 0x0141, 0x00A4, 0x0104, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x0105, 0x015F, 0x00BB, 0x013D, 0x02DD, 0x013E, 0x017C,
    0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7, 0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E,
    0x0110, 0x0143, 0x0147, 0x00D3, 0x00D4, 0x0150, 0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB, 0x011B, 0x00ED, 0x00EE, 0x010F,
    0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7, 0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];
const CP1251: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F,
    0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x0098, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F,
    0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417, 0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F,
    0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427, 0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447, 0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];
const CP1252: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7, 0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7, 0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];
const CP1253: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x0088, 0x2030, 0x008A, 0x2039, 0x008C, 0x008D, 0x008E, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x0098, 0x2122, 0x009A, 0x203A, 0x009C, 0x009D, 0x009E, 0x009F,
    0x00A0, 0x0385, 0x0386, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x2015,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x0384, 0x00B5, 0x00B6, 0x00B7, 0x0388, 0x0389, 0x038A, 0x00BB, 0x038C, 0x00BD, 0x038E, 0x038F,
    0x0390, 0x0391, 0x0392, 0x0393, 0x0394, 0x0395, 0x0396, 0x0397, 0x0398, 0x0399, 0x039A, 0x039B, 0x039C, 0x039D, 0x039E, 0x039F,
    0x03A0, 0x03A1, 0x00D2, 0x03A3, 0x03A4, 0x03A5, 0x03A6, 0x03A7, 0x03A8, 0x03A9, 0x03AA, 0x03AB, 0x03AC, 0x03AD, 0x03AE, 0x03AF,
    0x03B0, 0x03B1, 0x03B2, 0x03B3, 0x03B4, 0x03B5, 0x03B6, 0x03B7, 0x03B8, 0x03B9, 0x03BA, 0x03BB, 0x03BC, 0x03BD, 0x03BE, 0x03BF,
    0x03C0, 0x03C1, 0x03C2, 0x03C3, 0x03C4, 0x03C5, 0x03C6, 0x03C7, 0x03C8, 0x03C9, 0x03CA, 0x03CB, 0x03CC, 0x03CD, 0x03CE, 0x00FF,
];
const CP1254: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x008E, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x009E, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7, 0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x011E, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7, 0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(texts: &[&[u8]]) -> Stats {
        let mut s = Stats::default();
        for t in texts {
            count(t, &mut s);
        }
        s
    }

    #[test]
    fn polish_issue_1_text_is_1250() {
        // "To co widzisz to typowa sytuacja na początku misji. ... kwatera główna. Wokół kręcą się osadnicy i żołnierze."
        let pl: &[u8] = b"To co widzisz to typowa sytuacja na pocz\xb9tku misji. Twoja kwatera g\xb3\xf3wna. Wok\xf3\xb3 kr\xeac\xb9 si\xea osadnicy i \xbfo\xb3nierze.";
        let s = stats(&[pl]);
        assert!(s.ev1250 >= EV_MIN, "{s:?}");
        assert_eq!(decide(&s, 1252), 1250);
        let txt: String = pl.iter().map(|&b| char::from_u32(byte_to_u16_in(1250, b) as u32).unwrap()).collect();
        assert!(txt.contains("początku") && txt.contains("główna") && txt.contains("żołnierze") && txt.contains("kręcą się"));
    }

    #[test]
    fn czech_and_hungarian_are_1250() {
        let cs: &[u8] = b"P\xf8\xedli\x9a \x9elu\x9dou\xe8k\xfd k\xf9\xf2 \xfap\xecl \xef\xe1belsk\xe9 \xf3dy. Dob\xf8e, h\xf8bitov, mo\xf8e.";
        assert_eq!(decide(&stats(&[cs]), 1252), 1250);
        let hu: &[u8] = b"Az \xfaj \xe9p\xfcletek \xe9p\xedt\xe9se el\xf5tt, a k\xf5b\xe1ny\xe1szt \xe9s a f\xf5v\xe1rost.";
        assert_eq!(decide(&stats(&[hu]), 1252), 1250);
    }

    #[test]
    fn western_text_stays_1252() {
        let de: &[u8] = b"Holzf\xe4llerh\xfctte, S\xe4gewerk, Gro\xdfe Stra\xdfe. M\xfchle und B\xe4ckerei f\xfcr das Volk.";
        let fr: &[u8] = b"Cabane de b\xfbcheron, sc\xe9rie, \xe9cole. Le march\xe9 est pr\xeat, tr\xe8s bien, o\xf9 \xe7a? \xc9glise. \x9cuvre.";
        let es: &[u8] = b"\xbfD\xf3nde est\xe1 el le\xf1ador? \xa1Construye una panader\xeda! Ma\xf1ana.";
        let it: &[u8] = b"Capanna del taglialegna, citt\xe0, perch\xe9, pi\xf9, cos\xec. Gi\xe0 fatto.";
        let s = stats(&[de, fr, es, it]);
        assert_eq!(decide(&s, 1252), 1252, "{s:?}");
    }

    #[test]
    fn russian_is_1251() {
        // "Хижина лесоруба, лесопилка" in 1251.
        let ru: &[u8] = b"\xd5\xe8\xe6\xe8\xed\xe0 \xeb\xe5\xf1\xee\xf0\xf3\xe1\xe0, \xeb\xe5\xf1\xee\xef\xe8\xeb\xea\xe0. \xcf\xee\xf1\xe5\xeb\xe5\xed\xf6\xfb";
        assert_eq!(decide(&stats(&[ru]), 1252), 1251);
        let txt: String = ru.iter().map(|&b| char::from_u32(byte_to_u16_in(1251, b) as u32).unwrap()).collect();
        assert!(txt.starts_with("Хижина лесоруба"));
    }

    #[test]
    fn hint_and_tables() {
        assert_eq!(for_langid(9), 1252);
        assert_eq!(for_langid(7), 1252);
        assert_eq!(for_langid(0x15), 1250);
        assert_eq!(for_langid(0x0415), 1250);
        assert_eq!(for_langid(0x19), 1251);
        assert_eq!(decide(&Stats::default(), 1250), 1250);
        for cp in SUPPORTED {
            for b in 0u8..=255 {
                let u = byte_to_u16_in(cp, b);
                assert_eq!(u16_to_byte_in(cp, u), Some(b), "cp {cp} byte {b:#x}");
            }
        }
        assert_eq!(u16_to_byte_in(1250, 'ł' as u16), Some(0xB3));
        assert_eq!(u16_to_byte_in(1252, 'ł' as u16), None);
        assert_eq!(byte_to_u16_in(1252, 0x80), 0x20AC);
    }
}
