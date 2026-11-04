//! Engine: one call to get a running S3.EXE.
//!
//! ```no_run
//! # use s3_engine::Engine;
//! # use s3_runtime::NullHost;
//! let host = NullHost::new("/path/to/game".into(), "/path/to/data".into());
//! let mut e = Engine::new(Box::new(host), None).unwrap();
//! while e.run_for_ms(16) != s3_runtime::SliceResult::Stopped {}
//! println!("{}", e.summary());
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use s3_pe::LoadedImage;
use s3_runtime::{Host, LogLevel, Machine, SliceResult, Vfs};

/// Instructions per `run_slice` call made by [`Engine::run_for_ms`].
pub const SLICE_INSTRUCTIONS: u64 = 1_000_000;

pub struct Engine {
    pub machine: Machine,
    pub image: LoadedImage,
    pub exe_path: PathBuf,
}

/// Find `S3.EXE` in the game dir (case-insensitive).
pub fn find_exe(game_dir: &Path) -> Option<PathBuf> {
    Vfs::lookup_ci(game_dir, &["S3.EXE".to_string()])
}

impl Engine {
    /// Build the machine, register all HLE modules, load the exe (default: `<game_dir>/S3.EXE`) and create
    /// the main thread at the entry point.
    pub fn new(host: Box<dyn Host>, exe_path: Option<&Path>) -> Result<Engine, String> {
        let exe_path = match exe_path {
            Some(p) => p.to_path_buf(),
            None => find_exe(&host.game_dir())
                .ok_or_else(|| format!("S3.EXE not found in {}", host.game_dir().display()))?,
        };
        let bytes = std::fs::read(&exe_path).map_err(|e| format!("read {}: {e}", exe_path.display()))?;
        let mut m = Machine::new(host)?;
        s3_win32::register_all(&mut m.registry);
        s3_dx::register_all(&mut m.registry);
        s3_dx::register_factories(&mut m);
        let image = s3_pe::load(&mut m, &bytes, "S3.EXE").map_err(|e| e.to_string())?;
        let auto: Vec<String> = image.auto_stubbed(&m).map(|i| format!("{}!{}", i.dll, i.name)).collect();
        if !auto.is_empty() {
            m.log(LogLevel::Warn, &format!("{} imports not declared by any HLE module: {}", auto.len(), auto.join(", ")));
        }
        let stack = m.process.stack_reserve;
        m.create_thread(image.entry, 0, stack, false)?;
        m.log(
            LogLevel::Info,
            &format!("loaded {} entry={:#x} imports={}", exe_path.display(), image.entry, image.imports.len()),
        );
        Ok(Engine { machine: m, image, exe_path })
    }

    /// Run up to `max_instructions` guest instructions.
    pub fn run_slice(&mut self, max_instructions: u64) -> SliceResult {
        self.machine.run_slice(max_instructions)
    }

    /// Run for about `ms` wall-clock milliseconds (sleeping when all threads are idle). Returns the last slice
    /// result (`Stopped` as soon as the machine stops).
    pub fn run_for_ms(&mut self, ms: u64) -> SliceResult {
        let start = Instant::now();
        let mut last = SliceResult::Idle;
        while start.elapsed().as_millis() < ms as u128 {
            last = self.machine.run_slice(SLICE_INSTRUCTIONS);
            match last {
                SliceResult::Stopped => return last,
                SliceResult::Idle => self.machine.host.idle(1),
                SliceResult::Ran(_) => {}
            }
        }
        last
    }

    /// Human-readable run summary: instructions, top thunk calls, presents, stop reason.
    pub fn summary(&self) -> String {
        let m = &self.machine;
        let mut s = String::new();
        s.push_str(&format!("instructions executed: {}\n", m.stats.instructions));
        s.push_str(&format!("thunk calls: {}\n", m.stats.thunk_calls));
        s.push_str(&format!("presents: {}\n", m.stats.presents));
        s.push_str(&format!("guest exceptions: {}\n", m.stats.exceptions));
        s.push_str(&format!("threads: {}\n", m.threads.len()));
        let mut calls: Vec<_> = m.registry.entries().iter().filter(|e| e.calls > 0).collect();
        calls.sort_by(|a, b| b.calls.cmp(&a.calls));
        s.push_str(&format!("distinct HLE functions called: {}\n", calls.len()));
        for e in calls.iter().take(40) {
            s.push_str(&format!("  {:>10}  {}!{}\n", e.calls, e.dll, e.name));
        }
        if let Some(t) = m.threads.get(m.cur) {
            s.push_str(&format!("current thread {} eip={:#x} esp={:#x}\n", m.cur, t.cpu.eip, t.cpu.regs[4]));
        }
        match m.stop_reason() {
            Some(r) => s.push_str(&format!("stop: {r}\n")),
            None => s.push_str("stop: (still running)\n"),
        }
        s
    }
}
