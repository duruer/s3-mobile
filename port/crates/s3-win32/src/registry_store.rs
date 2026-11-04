//! Fake Windows registry for advapi32: an in-memory tree, prefilled with the game's defaults, persisted as JSON in
//! `data_dir/registry.json`.
//!
//! Load order (later wins): built-in defaults (docs/reverse-engineering/registry.md, no serial number) ->
//! `regs.cmd` of the user's GOG install parsed at runtime (gives `SingleNumber`; we never embed the serial) ->
//! `data_dir/registry.json` (values the game wrote earlier).
//!
//! Special rule (docs/reverse-engineering/wine-trace.md): the game rewrites `SingleNumber` *encrypted*, but the
//! GOG exe has its decoder NOPed, so a rewrite would break the next start. A write to `General\SingleNumber` is
//! ignored when a value already exists.

use std::collections::BTreeMap;
use std::path::Path;

pub const REG_NONE: u32 = 0;
pub const REG_SZ: u32 = 1;
pub const REG_EXPAND_SZ: u32 = 2;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;
pub const REG_MULTI_SZ: u32 = 7;

pub const HKEY_CLASSES_ROOT: u32 = 0x8000_0000;
pub const HKEY_CURRENT_USER: u32 = 0x8000_0001;
pub const HKEY_LOCAL_MACHINE: u32 = 0x8000_0002;
pub const HKEY_USERS: u32 = 0x8000_0003;
pub const HKEY_CURRENT_CONFIG: u32 = 0x8000_0005;

/// Path of the game's main settings key.
pub const GENERAL_KEY: &str = r"HKEY_LOCAL_MACHINE\Software\BlueByte\Siedler3\1.0\General";
/// The malformed CLSID key that holds the serial XOR `Seed`.
pub const SEED_KEY: &str = r"HKEY_CLASSES_ROOT\CLSID\{0e9f8400-d9a5-11cf-8f4d00804880b189}";

/// One value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    pub name: String,
    pub kind: u32,
    pub data: Vec<u8>,
}

/// One key.
#[derive(Debug, Clone, Default)]
pub struct RegKey {
    pub name: String,
    /// Keyed by lower-case value name ("" = default value).
    pub values: BTreeMap<String, RegValue>,
    /// Keyed by lower-case subkey name.
    pub subkeys: BTreeMap<String, RegKey>,
}

/// Root key name for a predefined HKEY.
pub fn root_name(hkey: u32) -> Option<&'static str> {
    Some(match hkey {
        HKEY_CLASSES_ROOT => "HKEY_CLASSES_ROOT",
        HKEY_CURRENT_USER => "HKEY_CURRENT_USER",
        HKEY_LOCAL_MACHINE => "HKEY_LOCAL_MACHINE",
        HKEY_USERS => "HKEY_USERS",
        HKEY_CURRENT_CONFIG => "HKEY_CURRENT_CONFIG",
        _ => return None,
    })
}

fn canon_root(s: &str) -> String {
    match s.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => "HKEY_LOCAL_MACHINE".into(),
        "HKCU" | "HKEY_CURRENT_USER" => "HKEY_CURRENT_USER".into(),
        "HKCR" | "HKEY_CLASSES_ROOT" => "HKEY_CLASSES_ROOT".into(),
        "HKU" | "HKEY_USERS" => "HKEY_USERS".into(),
        "HKCC" | "HKEY_CURRENT_CONFIG" => "HKEY_CURRENT_CONFIG".into(),
        _ => s.to_string(),
    }
}

/// Split a key path into components (backslashes, empty parts dropped, root abbreviations expanded).
/// `Software\Wow6432Node\X` is folded to `Software\X` (32-bit view).
pub fn split_path(p: &str) -> Vec<String> {
    let mut v: Vec<String> = p.split('\\').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    if let Some(first) = v.first_mut() {
        *first = canon_root(first);
    }
    if v.len() >= 3 && v[1].eq_ignore_ascii_case("software") && v[2].eq_ignore_ascii_case("wow6432node") {
        v.remove(2);
    }
    v
}

impl RegValue {
    pub fn dword(name: &str, v: u32) -> Self {
        RegValue { name: name.into(), kind: REG_DWORD, data: v.to_le_bytes().to_vec() }
    }
    pub fn sz(name: &str, s: &str) -> Self {
        let mut d = s.as_bytes().to_vec();
        d.push(0);
        RegValue { name: name.into(), kind: REG_SZ, data: d }
    }
    pub fn binary(name: &str, d: Vec<u8>) -> Self {
        RegValue { name: name.into(), kind: REG_BINARY, data: d }
    }
    /// As DWORD (if 4 bytes).
    pub fn as_dword(&self) -> Option<u32> {
        if self.data.len() >= 4 {
            Some(u32::from_le_bytes([self.data[0], self.data[1], self.data[2], self.data[3]]))
        } else {
            None
        }
    }
    /// As string (strips the terminating NUL).
    pub fn as_str(&self) -> String {
        let end = self.data.iter().position(|&b| b == 0).unwrap_or(self.data.len());
        String::from_utf8_lossy(&self.data[..end]).into_owned()
    }
}

/// The registry tree (a synthetic root whose subkeys are `HKEY_*`).
#[derive(Debug, Clone, Default)]
pub struct RegStore {
    pub root: RegKey,
    /// Set when something changed since the last save.
    pub dirty: bool,
}

impl RegStore {
    /// Empty store.
    pub fn new() -> Self {
        RegStore::default()
    }

    /// Store with the built-in defaults.
    pub fn with_defaults() -> Self {
        let mut s = RegStore::new();
        s.apply_defaults();
        s.dirty = false;
        s
    }

    pub fn key(&self, path: &[String]) -> Option<&RegKey> {
        let mut k = &self.root;
        for p in path {
            k = k.subkeys.get(&p.to_ascii_lowercase())?;
        }
        Some(k)
    }

    pub fn key_mut(&mut self, path: &[String]) -> Option<&mut RegKey> {
        let mut k = &mut self.root;
        for p in path {
            k = k.subkeys.get_mut(&p.to_ascii_lowercase())?;
        }
        Some(k)
    }

    /// Create (or open) a key; returns true if it was created.
    pub fn create_key(&mut self, path: &[String]) -> bool {
        let mut k = &mut self.root;
        let mut created = false;
        for p in path {
            let lk = p.to_ascii_lowercase();
            if !k.subkeys.contains_key(&lk) {
                created = true;
                k.subkeys.insert(lk.clone(), RegKey { name: p.clone(), ..Default::default() });
            }
            k = k.subkeys.get_mut(&lk).unwrap();
        }
        if created {
            self.dirty = true;
        }
        created
    }

    /// Delete a key (and its subtree). Returns false if missing.
    pub fn delete_key(&mut self, path: &[String]) -> bool {
        let Some((last, parent)) = path.split_last() else { return false };
        let removed = self.key_mut(parent).and_then(|k| k.subkeys.remove(&last.to_ascii_lowercase())).is_some();
        self.dirty |= removed;
        removed
    }

    pub fn get_value(&self, path: &[String], name: &str) -> Option<&RegValue> {
        self.key(path)?.values.get(&name.to_ascii_lowercase())
    }

    /// Set a value, creating the key.
    pub fn set_value(&mut self, path: &[String], v: RegValue) {
        self.create_key(path);
        let k = self.key_mut(path).unwrap();
        let lk = v.name.to_ascii_lowercase();
        if k.values.get(&lk) != Some(&v) {
            k.values.insert(lk, v);
            self.dirty = true;
        }
    }

    pub fn delete_value(&mut self, path: &[String], name: &str) -> bool {
        let r = self.key_mut(path).and_then(|k| k.values.remove(&name.to_ascii_lowercase())).is_some();
        self.dirty |= r;
        r
    }

    fn setp(&mut self, path: &str, v: RegValue) {
        self.set_value(&split_path(path), v);
    }

    /// Built-in defaults (from GOG `regs.cmd`, without the serial number) plus the serial XOR `Seed`.
    pub fn apply_defaults(&mut self) {
        let g = GENERAL_KEY;
        for (n, v) in [
            ("Resolution", 0),
            ("NoAlpha", 0),
            ("FogSpeed", 1),
            ("GDIMouse", 1),
            ("WaitVBlank", 1),
            ("ScrollMode", 0),
            ("ScrollSpeed", 5),
            ("SoundFormat", 1),
            ("CDAudio", 2),
            ("VideoFormat", 1),
            ("MessageLevel", 9),
            ("Intro", 1),
            ("Language", 9),
            ("Tips&Tricks", 1),
            ("Plus", 1),
            ("ClanFlag", 0),
            ("Mission", 1),
            ("FontSize", 15),
            ("Newbie", 0),
            ("Gold", 1),
            ("BuildHelp", 1),
            ("GameZoneSelection", 511),
            ("FirstLogon", 1),
            ("BannerId", 0),
            ("BannerSubId", 0),
        ] {
            self.setp(g, RegValue::dword(n, v));
        }
        self.setp(g, RegValue::sz("Font", "MS Sans Serif"));
        self.setp(g, RegValue::sz("Playername", "Player"));
        self.setp(r"HKEY_LOCAL_MACHINE\Software\BlueByte\Siedler3\1.0\Patches", RegValue::sz("Convert", "0"));
        // Any DWORD works (wine-trace.md); the serial dialog wrote 0x6a0f in the reference run.
        self.setp(SEED_KEY, RegValue::dword("Seed", 0x6a0f));
        self.create_key(&split_path(r"HKEY_CURRENT_USER\Software"));
    }

    /// Apply `REG ADD` lines of a GOG `regs.cmd` (variables `%regpath%`, `%locpath%` are expanded; the install dir
    /// becomes `guest_game_dir`). Returns the number of values applied.
    pub fn apply_regs_cmd(&mut self, text: &str, guest_game_dir: &str) -> usize {
        let mut regpath = String::new();
        let mut n = 0;
        for raw in text.lines() {
            let line = raw.trim();
            let upper = line.to_ascii_uppercase();
            if let Some(rest) = upper.strip_prefix("SET REGPATH=").map(|_| &line[12..]) {
                regpath = rest.trim().trim_matches('"').to_string();
                continue;
            }
            if !upper.starts_with("REG ADD ") {
                continue;
            }
            let toks = tokenize(&line[8..]);
            if toks.is_empty() {
                continue;
            }
            let key = toks[0].replace("%regpath%", &regpath);
            let (mut name, mut kind, mut data) = (None::<String>, "REG_SZ".to_string(), String::new());
            let mut i = 1;
            while i < toks.len() {
                match toks[i].to_ascii_lowercase().as_str() {
                    "/v" if i + 1 < toks.len() => {
                        name = Some(toks[i + 1].clone());
                        i += 1;
                    }
                    "/ve" => name = Some(String::new()),
                    "/t" if i + 1 < toks.len() => {
                        kind = toks[i + 1].to_ascii_uppercase();
                        i += 1;
                    }
                    "/d" if i + 1 < toks.len() => {
                        data = toks[i + 1].clone();
                        i += 1;
                    }
                    _ => {}
                }
                i += 1;
            }
            let Some(name) = name else { continue };
            let data = data.replace("%locpath%", guest_game_dir).replace("\\\\", "\\");
            let v = match kind.as_str() {
                "REG_DWORD" => {
                    let d = data.trim();
                    let parsed = if let Some(h) = d.strip_prefix("0x") { u32::from_str_radix(h, 16).ok() } else { d.parse().ok() };
                    RegValue::dword(&name, parsed.unwrap_or(0))
                }
                "REG_BINARY" => RegValue::binary(&name, hex_decode(&data)),
                _ => RegValue::sz(&name, &data),
            };
            self.setp(&key, v);
            n += 1;
        }
        n
    }

    /// Serialize to JSON: `{ "KEY\\PATH": { "Value": {"type": "dword"|"sz"|"binary"|<n>, "data": ...} } }`.
    pub fn to_json(&self) -> String {
        let mut obj = serde_json::Map::new();
        fn walk(k: &RegKey, path: &mut Vec<String>, out: &mut serde_json::Map<String, serde_json::Value>) {
            if !path.is_empty() {
                let mut vals = serde_json::Map::new();
                for v in k.values.values() {
                    let j = match v.kind {
                        REG_DWORD if v.data.len() == 4 => serde_json::json!({"type": "dword", "data": v.as_dword().unwrap()}),
                        REG_SZ | REG_EXPAND_SZ => serde_json::json!({"type": if v.kind == REG_SZ {"sz"} else {"expand_sz"}, "data": v.as_str()}),
                        REG_BINARY => serde_json::json!({"type": "binary", "data": hex_encode(&v.data)}),
                        other => serde_json::json!({"type": other, "data": hex_encode(&v.data)}),
                    };
                    vals.insert(v.name.clone(), j);
                }
                out.insert(path.join("\\"), serde_json::Value::Object(vals));
            }
            for sk in k.subkeys.values() {
                path.push(sk.name.clone());
                walk(sk, path, out);
                path.pop();
            }
        }
        walk(&self.root, &mut Vec::new(), &mut obj);
        serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap_or_default()
    }

    /// Merge values from JSON produced by [`RegStore::to_json`].
    pub fn merge_json(&mut self, text: &str) -> Result<(), String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let obj = v.as_object().ok_or("registry json: not an object")?;
        for (key, vals) in obj {
            let path = split_path(key);
            self.create_key(&path);
            let Some(vals) = vals.as_object() else { continue };
            for (name, j) in vals {
                let ty = &j["type"];
                let data = &j["data"];
                let rv = match ty.as_str() {
                    Some("dword") => RegValue::dword(name, data.as_u64().unwrap_or(0) as u32),
                    Some("sz") => RegValue::sz(name, data.as_str().unwrap_or("")),
                    Some("expand_sz") => {
                        let mut r = RegValue::sz(name, data.as_str().unwrap_or(""));
                        r.kind = REG_EXPAND_SZ;
                        r
                    }
                    Some("binary") => RegValue::binary(name, hex_decode(data.as_str().unwrap_or(""))),
                    _ => RegValue {
                        name: name.clone(),
                        kind: ty.as_u64().unwrap_or(REG_BINARY as u64) as u32,
                        data: hex_decode(data.as_str().unwrap_or("")),
                    },
                };
                self.set_value(&path, rv);
            }
        }
        Ok(())
    }

    /// Defaults + `game_dir/regs.cmd` + `data_dir/registry.json`.
    pub fn load(game_dir: &Path, data_dir: &Path, guest_game_dir: &str) -> (RegStore, Vec<String>) {
        let mut notes = Vec::new();
        let mut s = RegStore::with_defaults();
        if let Some(p) = find_ci(game_dir, "regs.cmd") {
            if let Ok(b) = std::fs::read(&p) {
                let n = s.apply_regs_cmd(&String::from_utf8_lossy(&b), guest_game_dir);
                notes.push(format!("registry: applied {n} values from {}", p.display()));
            }
        } else {
            notes.push("registry: no regs.cmd in the game dir (the game will ask for the serial number)".into());
        }
        let jp = data_dir.join("registry.json");
        if let Ok(t) = std::fs::read_to_string(&jp) {
            match s.merge_json(&t) {
                Ok(()) => notes.push(format!("registry: loaded {}", jp.display())),
                Err(e) => notes.push(format!("registry: ignoring broken {}: {e}", jp.display())),
            }
        }
        s.dirty = false;
        (s, notes)
    }

    /// Write `data_dir/registry.json` if dirty.
    pub fn save(&mut self, data_dir: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        std::fs::create_dir_all(data_dir)?;
        let tmp = data_dir.join("registry.json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, data_dir.join("registry.json"))?;
        self.dirty = false;
        Ok(())
    }
}

fn find_ci(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|e| e.path())
}

/// Split a cmd.exe argument string: double-quoted tokens (quotes removed) or whitespace-separated words.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut inq = false;
    let mut have = false;
    for c in s.chars() {
        match c {
            '"' => {
                inq = !inq;
                have = true;
            }
            c if c.is_whitespace() && !inq => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            c => {
                cur.push(c);
                have = true;
            }
        }
    }
    if have {
        out.push(cur);
    }
    out
}

pub fn hex_encode(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hex_decode(s: &str) -> Vec<u8> {
    let s: Vec<u8> = s.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    s.chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_case_insensitive() {
        let s = RegStore::with_defaults();
        let p = split_path(r"HKLM\SOFTWARE\Bluebyte\Siedler3\1.0\GENERAL");
        assert_eq!(s.get_value(&p, "resolution").unwrap().as_dword(), Some(0));
        assert_eq!(s.get_value(&p, "FONT").unwrap().as_str(), "MS Sans Serif");
        assert_eq!(s.get_value(&split_path(SEED_KEY), "Seed").unwrap().as_dword(), Some(0x6a0f));
        // 32-bit view folding.
        let w = split_path(r"HKEY_LOCAL_MACHINE\Software\Wow6432Node\BlueByte\Siedler3\1.0\General");
        assert_eq!(s.get_value(&w, "FontSize").unwrap().as_dword(), Some(15));
    }

    #[test]
    fn regs_cmd_parse() {
        let text = "@echo off\r\nSET regpath=\"HKEY_LOCAL_MACHINE\\SOFTWARE\\Bluebyte\\Siedler3\\1.0\\General\"\r\n\
            REG ADD %regpath% /v \"SingleNumber\" /t REG_SZ /d \"1111-2222\" /f /reg:32\r\n\
            REG ADD %regpath% /t REG_DWORD /v \"Resolution\" /d 2 /f /reg:32\r\n\
            REG ADD %regpath% /t REG_BINARY /v \"Campaigns\" /d 0a0bff /f /reg:32\r\n\
            SET regpath=\"HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\s3.exe\"\r\n\
            REG ADD %regpath% /t REG_SZ /ve /d \"%locpath%\\s3.exe\" /f /reg:32\r\n";
        let mut s = RegStore::with_defaults();
        assert_eq!(s.apply_regs_cmd(text, r"C:\S3"), 4);
        let g = split_path(GENERAL_KEY);
        assert_eq!(s.get_value(&g, "SingleNumber").unwrap().as_str(), "1111-2222");
        assert_eq!(s.get_value(&g, "Resolution").unwrap().as_dword(), Some(2));
        assert_eq!(s.get_value(&g, "Campaigns").unwrap().data, vec![0x0a, 0x0b, 0xff]);
        let ap = split_path(r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\s3.exe");
        assert_eq!(s.get_value(&ap, "").unwrap().as_str(), r"C:\S3\s3.exe");
    }

    #[test]
    fn json_roundtrip() {
        let mut s = RegStore::with_defaults();
        let g = split_path(GENERAL_KEY);
        s.set_value(&g, RegValue::binary("Campaigns", vec![1, 2, 3]));
        s.set_value(&g, RegValue { name: "Multi".into(), kind: REG_MULTI_SZ, data: b"a\0b\0\0".to_vec() });
        let j = s.to_json();
        let mut t = RegStore::new();
        t.merge_json(&j).unwrap();
        assert_eq!(t.get_value(&g, "Campaigns").unwrap().data, vec![1, 2, 3]);
        assert_eq!(t.get_value(&g, "Playername").unwrap().as_str(), "Player");
        assert_eq!(t.get_value(&g, "multi").unwrap().kind, REG_MULTI_SZ);
        assert_eq!(t.get_value(&g, "GameZoneSelection").unwrap().as_dword(), Some(511));
    }
}
