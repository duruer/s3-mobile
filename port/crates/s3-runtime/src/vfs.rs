//! Guest path -> host path mapping.
//!
//! - Guest paths are case-insensitive, use `\` or `/`, may carry a drive letter and may be relative to the guest
//!   current directory.
//! - The guest sees the game installed at [`GUEST_GAME_DIR`] (`C:\S3`). Any absolute path under it, and any other
//!   drive root (CD paths like `D:\GFX\...`), maps onto the host `game_dir`.
//! - Writes go to an overlay in `data_dir/overlay/` mirroring the guest-relative path; reads check the overlay
//!   first, then the game dir. The game dir is never modified.
//! - Deleting / renaming a file that exists only in the game dir records a *whiteout* (persisted in
//!   `data_dir/overlay-whiteouts.txt`): the game-dir copy is hidden from then on.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Where the guest believes the game is installed (no trailing backslash).
pub const GUEST_GAME_DIR: &str = "C:\\S3";

/// The mapping.
#[derive(Debug, Clone)]
pub struct Vfs {
    pub game_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Guest current directory, absolute, e.g. `C:\S3`.
    pub cwd: String,
    /// Lower-case game-relative paths (`gfx\x.dat`) hidden from the game dir (deleted/renamed by the guest).
    pub whiteouts: BTreeSet<String>,
}

/// Win32 error codes returned by the VFS file operations.
pub mod err {
    pub const FILE_NOT_FOUND: u32 = 2;
    pub const PATH_NOT_FOUND: u32 = 3;
    pub const ACCESS_DENIED: u32 = 5;
    pub const FILE_EXISTS: u32 = 80;
    pub const DIR_NOT_EMPTY: u32 = 145;
    pub const ALREADY_EXISTS: u32 = 183;
}

/// A directory entry from [`Vfs::list_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Name as found on disk.
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub host_path: PathBuf,
}

impl Vfs {
    pub fn new(game_dir: PathBuf, data_dir: PathBuf) -> Self {
        let mut whiteouts = BTreeSet::new();
        if let Ok(t) = fs::read_to_string(data_dir.join("overlay-whiteouts.txt")) {
            whiteouts.extend(t.lines().filter(|l| !l.is_empty()).map(|l| l.to_string()));
        }
        Vfs { game_dir, data_dir, cwd: GUEST_GAME_DIR.to_string(), whiteouts }
    }

    fn key(parts: &[String]) -> String {
        parts.join("\\").to_ascii_lowercase()
    }

    /// Is the game-dir copy of `parts` (or one of its parent directories) hidden?
    pub fn is_whited_out(&self, parts: &[String]) -> bool {
        (1..=parts.len()).any(|n| self.whiteouts.contains(&Self::key(&parts[..n])))
    }

    fn save_whiteouts(&self) {
        let _ = fs::create_dir_all(&self.data_dir);
        let t: String = self.whiteouts.iter().map(|w| format!("{w}\n")).collect();
        let _ = fs::write(self.data_dir.join("overlay-whiteouts.txt"), t);
    }

    fn set_whiteout(&mut self, parts: &[String], on: bool) {
        let k = Self::key(parts);
        let changed = if on { self.whiteouts.insert(k) } else { self.whiteouts.remove(&k) };
        if changed {
            self.save_whiteouts();
        }
    }

    /// Game-dir path of `parts` unless whited out.
    fn game_lookup(&self, parts: &[String]) -> Option<PathBuf> {
        if self.is_whited_out(parts) {
            return None;
        }
        Self::lookup_ci(&self.game_dir, parts)
    }

    /// Overlay root for written files.
    pub fn overlay_dir(&self) -> PathBuf {
        self.data_dir.join("overlay")
    }

    /// Normalize a guest path to components relative to the game root (`..` and `.` resolved).
    /// `"C:\\S3\\Gfx\\x.dat"` -> `["Gfx", "x.dat"]`; relative paths are resolved against `cwd`.
    pub fn components(&self, guest: &str) -> Vec<String> {
        let p = guest.replace('/', "\\");
        let abs = if p.len() >= 2 && p.as_bytes()[1] == b':' {
            p
        } else if p.starts_with('\\') {
            format!("{}{}", &self.cwd[..2.min(self.cwd.len())], p)
        } else {
            format!("{}\\{}", self.cwd, p)
        };
        // Strip the drive letter.
        let rest = &abs[2..];
        let mut parts: Vec<String> = Vec::new();
        for c in rest.split('\\') {
            match c {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                c => parts.push(c.to_string()),
            }
        }
        // Strip the guest install dir prefix (C:\S3\...) when present.
        let prefix: Vec<&str> = GUEST_GAME_DIR[3..].split('\\').collect();
        let drive_c = abs[..1].eq_ignore_ascii_case("c");
        if drive_c
            && parts.len() >= prefix.len()
            && parts.iter().zip(prefix.iter()).all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            parts.drain(..prefix.len());
        }
        parts
    }

    /// Absolute guest path for components (inverse of [`Vfs::components`]).
    pub fn guest_path(parts: &[String]) -> String {
        let mut s = GUEST_GAME_DIR.to_string();
        for p in parts {
            s.push('\\');
            s.push_str(p);
        }
        s
    }

    /// Case-insensitive lookup of `parts` under `root`. Returns the existing host path.
    pub fn lookup_ci(root: &Path, parts: &[String]) -> Option<PathBuf> {
        let mut cur = root.to_path_buf();
        for part in parts {
            let exact = cur.join(part);
            if exact.exists() {
                cur = exact;
                continue;
            }
            let rd = fs::read_dir(&cur).ok()?;
            let mut found = None;
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().eq_ignore_ascii_case(part) {
                    found = Some(e.path());
                    break;
                }
            }
            cur = found?;
        }
        Some(cur)
    }

    /// Host path to read `guest` from (overlay first, then game dir). None if it does not exist.
    pub fn resolve_read(&self, guest: &str) -> Option<PathBuf> {
        let parts = self.components(guest);
        Self::lookup_ci(&self.overlay_dir(), &parts).or_else(|| self.game_lookup(&parts))
    }

    /// Does `guest` exist in the overlay (as opposed to only in the game dir)?
    pub fn in_overlay(&self, guest: &str) -> bool {
        Self::lookup_ci(&self.overlay_dir(), &self.components(guest)).is_some()
    }

    /// `DeleteFileA`. Overlay copies are removed; a game-dir file is hidden with a whiteout.
    pub fn remove_file(&mut self, guest: &str) -> Result<(), u32> {
        let parts = self.components(guest);
        let ov = Self::lookup_ci(&self.overlay_dir(), &parts);
        let game = self.game_lookup(&parts);
        if ov.is_none() && game.is_none() {
            return Err(err::FILE_NOT_FOUND);
        }
        if ov.as_ref().or(game.as_ref()).map(|p| p.is_dir()).unwrap_or(false) {
            return Err(err::ACCESS_DENIED);
        }
        if let Some(p) = ov {
            fs::remove_file(p).map_err(|_| err::ACCESS_DENIED)?;
        }
        if game.is_some() {
            self.set_whiteout(&parts, true);
        }
        Ok(())
    }

    /// `MoveFileA` (files; directories only when they live in the overlay alone).
    pub fn rename(&mut self, src: &str, dst: &str) -> Result<(), u32> {
        if self.resolve_read(dst).is_some() {
            return Err(err::ALREADY_EXISTS);
        }
        let sparts = self.components(src);
        let ov = Self::lookup_ci(&self.overlay_dir(), &sparts);
        let game = self.game_lookup(&sparts);
        let Some(from) = ov.clone().or(game.clone()) else { return Err(err::FILE_NOT_FOUND) };
        if from.is_dir() && game.is_some() {
            return Err(err::ACCESS_DENIED);
        }
        let to = self.resolve_write(dst).map_err(|_| err::PATH_NOT_FOUND)?;
        if ov.is_some() {
            fs::rename(&from, &to).map_err(|_| err::ACCESS_DENIED)?;
        } else {
            fs::copy(&from, &to).map_err(|_| err::ACCESS_DENIED)?;
        }
        if game.is_some() {
            self.set_whiteout(&sparts, true);
        }
        let dparts = self.components(dst);
        self.set_whiteout(&dparts, false);
        Ok(())
    }

    /// `CopyFileA`.
    pub fn copy(&mut self, src: &str, dst: &str, fail_if_exists: bool) -> Result<(), u32> {
        let from = self.resolve_read(src).ok_or(err::FILE_NOT_FOUND)?;
        if from.is_dir() {
            return Err(err::ACCESS_DENIED);
        }
        if fail_if_exists && self.resolve_read(dst).is_some() {
            return Err(err::FILE_EXISTS);
        }
        let to = self.resolve_write(dst).map_err(|_| err::PATH_NOT_FOUND)?;
        fs::copy(&from, &to).map_err(|_| err::ACCESS_DENIED)?;
        let dparts = self.components(dst);
        self.set_whiteout(&dparts, false);
        Ok(())
    }

    /// `CreateDirectoryA` (in the overlay).
    pub fn create_dir(&mut self, guest: &str) -> Result<(), u32> {
        if self.resolve_read(guest).is_some() {
            return Err(err::ALREADY_EXISTS);
        }
        let parts = self.components(guest);
        if parts.is_empty() {
            return Err(err::ALREADY_EXISTS);
        }
        let parent = Self::guest_path(&parts[..parts.len() - 1]);
        if !parts[..parts.len() - 1].is_empty() && !self.is_dir(&parent) {
            return Err(err::PATH_NOT_FOUND);
        }
        let p = self.resolve_write(guest).map_err(|_| err::PATH_NOT_FOUND)?;
        fs::create_dir_all(p).map_err(|_| err::ACCESS_DENIED)?;
        self.set_whiteout(&parts, false);
        Ok(())
    }

    /// `RemoveDirectoryA` (must be empty as seen by the guest).
    pub fn remove_dir(&mut self, guest: &str) -> Result<(), u32> {
        if !self.is_dir(guest) {
            return Err(err::PATH_NOT_FOUND);
        }
        if !self.list_dir(guest).is_empty() {
            return Err(err::DIR_NOT_EMPTY);
        }
        let parts = self.components(guest);
        if let Some(p) = Self::lookup_ci(&self.overlay_dir(), &parts) {
            let _ = fs::remove_dir_all(p);
        }
        if Self::lookup_ci(&self.game_dir, &parts).is_some() {
            self.set_whiteout(&parts, true);
        }
        Ok(())
    }

    /// Host path to write `guest` to (in the overlay). Parent directories are created. Existing overlay
    /// entries are matched case-insensitively so the same file is not duplicated with different case.
    pub fn resolve_write(&self, guest: &str) -> std::io::Result<PathBuf> {
        let parts = self.components(guest);
        let mut cur = self.overlay_dir();
        fs::create_dir_all(&cur)?;
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let next = Self::lookup_ci(&cur, std::slice::from_ref(part)).unwrap_or_else(|| cur.join(part));
            if !last && !next.exists() {
                fs::create_dir_all(&next)?;
            }
            cur = next;
        }
        Ok(cur)
    }

    /// Like [`Vfs::resolve_write`] for a file being created/truncated: also drops a whiteout on that path.
    pub fn create_path(&mut self, guest: &str) -> std::io::Result<PathBuf> {
        let p = self.resolve_write(guest)?;
        let parts = self.components(guest);
        self.set_whiteout(&parts, false);
        Ok(p)
    }

    /// Copy a game-dir file into the overlay (for read/write opens of existing files). Returns the overlay path.
    pub fn copy_up(&self, guest: &str) -> std::io::Result<PathBuf> {
        let dst = self.resolve_write(guest)?;
        if !dst.exists() {
            if let Some(src) = self.game_lookup(&self.components(guest)) {
                if src.is_file() {
                    fs::copy(&src, &dst)?;
                }
            }
        }
        Ok(dst)
    }

    /// Is `guest` a directory (in overlay or game dir)?
    pub fn is_dir(&self, guest: &str) -> bool {
        self.resolve_read(guest).map(|p| p.is_dir()).unwrap_or(false)
    }

    /// Merged listing of a guest directory (overlay wins on name clashes, compared case-insensitively).
    pub fn list_dir(&self, guest_dir: &str) -> Vec<DirEntry> {
        let parts = self.components(guest_dir);
        let mut out: Vec<DirEntry> = Vec::new();
        for (gi, root) in [self.overlay_dir(), self.game_dir.clone()].into_iter().enumerate() {
            if gi == 1 && self.is_whited_out(&parts) {
                continue;
            }
            let Some(dir) = Self::lookup_ci(&root, &parts) else { continue };
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if out.iter().any(|d| d.name.eq_ignore_ascii_case(&name)) {
                    continue;
                }
                if gi == 1 {
                    let mut p = parts.clone();
                    p.push(name.clone());
                    if self.whiteouts.contains(&Self::key(&p)) {
                        continue;
                    }
                }
                let md = e.metadata().ok();
                out.push(DirEntry {
                    name,
                    is_dir: md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                    size: md.map(|m| m.len()).unwrap_or(0),
                    host_path: e.path(),
                });
            }
        }
        out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
        out
    }

    /// Set the guest current directory (`SetCurrentDirectoryA`). Returns false if it does not exist.
    pub fn set_cwd(&mut self, guest: &str) -> bool {
        let parts = self.components(guest);
        let exists = parts.is_empty()
            || Self::lookup_ci(&self.overlay_dir(), &parts).map(|p| p.is_dir()).unwrap_or(false)
            || self.game_lookup(&parts).map(|p| p.is_dir()).unwrap_or(false);
        if exists {
            self.cwd = Self::guest_path(&parts);
        }
        exists
    }
}

/// Simple DOS wildcard match (`*`, `?`), case-insensitive. `*.*` matches names without a dot too.
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
    if pattern == "*.*" || pattern == "*" {
        return true;
    }
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let n: Vec<char> = name.to_ascii_lowercase().chars().collect();
    fn rec(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => rec(&p[1..], n) || (!n.is_empty() && rec(p, &n[1..])),
            (Some('?'), Some(_)) => rec(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => rec(&p[1..], &n[1..]),
            _ => false,
        }
    }
    rec(&p, &n)
}
