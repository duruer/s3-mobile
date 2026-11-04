//! Thunk registry: every HLE function (Win32 export, COM method, internal helper) gets one address in the
//! thunk page `[THUNK_BASE, THUNK_END)`. Guest code calls/jumps there; the CPU exits with
//! `CpuExit::Thunk(addr)` and the machine runs the handler.

use std::collections::HashMap;

use s3_mem::layout::{FAKE_MODULE_BASE, THUNK_BASE, THUNK_END};

use crate::Machine;

/// An HLE function. Read arguments with `m.arg_u32(i)` (`i` = 0 for the first argument) and return a
/// [`ThunkResult`]. A handler that may return [`ThunkResult::Block`] must not have side effects before deciding
/// to block: it is simply called again later.
pub type Handler = fn(&mut Machine) -> ThunkResult;

/// What a handler wants to happen next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThunkResult {
    /// Return to the guest caller: EAX = `eax`, pop the return address and then `pop_bytes` of arguments
    /// (stdcall: 4 * nargs; cdecl: 0).
    Return { eax: u32, pop_bytes: u32 },
    /// Like `Return` but also sets EDX (64-bit results).
    Return64 { eax: u32, edx: u32, pop_bytes: u32 },
    /// The handler already set EIP/ESP/registers itself (e.g. it redirected into guest code); do nothing.
    Done,
    /// The calling thread waits; the handler is called again when the scheduler retries this thread.
    Block,
    /// Terminate the process with this exit code.
    Exit(u32),
    /// Terminate the calling thread with this exit code.
    ExitThread(u32),
    /// Stop the machine with an error (bug or unsupported situation).
    Error(String),
}

impl ThunkResult {
    /// stdcall return: callee pops `nargs` dword arguments.
    pub fn stdcall(eax: u32, nargs: u32) -> Self {
        ThunkResult::Return { eax, pop_bytes: nargs * 4 }
    }
    /// cdecl return: caller pops the arguments.
    pub fn cdecl(eax: u32) -> Self {
        ThunkResult::Return { eax, pop_bytes: 0 }
    }
}

/// One thunk.
#[derive(Debug, Clone)]
pub struct ThunkEntry {
    /// Guest address of the thunk.
    pub addr: u32,
    /// Lower-case DLL name with extension, e.g. `kernel32.dll`. COM methods use the DLL that implements them.
    pub dll: String,
    /// Export name, or `Interface::Method` for COM methods, or `#123` for ordinal-only imports.
    pub name: String,
    pub ordinal: Option<u16>,
    /// `None` = not implemented: calling it stops the machine with `UNIMPLEMENTED dll!name`.
    pub handler: Option<Handler>,
    /// Created automatically while resolving an import nobody declared.
    pub auto: bool,
    /// Number of calls so far.
    pub calls: u64,
}

/// Normalize a DLL name: lower case, `.dll` appended when there is no extension, path stripped.
pub fn normalize_dll(dll: &str) -> String {
    let base = dll.rsplit(['\\', '/']).next().unwrap_or(dll);
    let mut s = base.to_ascii_lowercase();
    if !s.contains('.') {
        s.push_str(".dll");
    }
    s
}

/// All thunks plus the list of HLE modules.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Vec<ThunkEntry>,
    by_name: HashMap<(String, String), usize>,
    by_ordinal: HashMap<(String, u16), usize>,
    aliases: HashMap<String, String>,
    modules: Vec<String>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make `alias` (e.g. `_inmm.dll`) resolve to `target` (e.g. `winmm.dll`).
    pub fn alias(&mut self, alias: &str, target: &str) {
        self.aliases.insert(normalize_dll(alias), normalize_dll(target));
    }

    fn canon(&self, dll: &str) -> String {
        let d = normalize_dll(dll);
        self.aliases.get(&d).cloned().unwrap_or(d)
    }

    fn ensure_module(&mut self, dll: &str) {
        if !self.modules.iter().any(|m| m == dll) {
            self.modules.push(dll.to_string());
        }
    }

    fn add(&mut self, dll: String, name: String, handler: Option<Handler>, auto: bool) -> u32 {
        let idx = self.entries.len();
        let addr = THUNK_BASE + idx as u32;
        assert!(addr < THUNK_END, "thunk page full");
        self.ensure_module(&dll);
        self.by_name.insert((dll.clone(), name.clone()), idx);
        self.entries.push(ThunkEntry { addr, dll, name, ordinal: None, handler, auto, calls: 0 });
        addr
    }

    /// Register (or replace) the handler of `dll!name`. Returns the thunk address.
    pub fn register(&mut self, dll: &str, name: &str, handler: Handler) -> u32 {
        let dll = self.canon(dll);
        if let Some(&i) = self.by_name.get(&(dll.clone(), name.to_string())) {
            self.entries[i].handler = Some(handler);
            self.entries[i].auto = false;
            return self.entries[i].addr;
        }
        self.add(dll, name.to_string(), Some(handler), false)
    }

    /// Declare `dll!name` without an implementation (keeps an existing handler). Returns the thunk address.
    pub fn declare(&mut self, dll: &str, name: &str) -> u32 {
        let dll = self.canon(dll);
        if let Some(&i) = self.by_name.get(&(dll.clone(), name.to_string())) {
            return self.entries[i].addr;
        }
        self.add(dll, name.to_string(), None, false)
    }

    /// Declare several names at once.
    pub fn declare_all(&mut self, dll: &str, names: &[&str]) {
        for n in names {
            self.declare(dll, n);
        }
    }

    /// Attach an export ordinal to `dll!name` (declaring it if needed).
    pub fn set_ordinal(&mut self, dll: &str, name: &str, ordinal: u16) {
        let addr = self.declare(dll, name);
        let i = (addr - THUNK_BASE) as usize;
        let dll = self.canon(dll);
        self.entries[i].ordinal = Some(ordinal);
        self.by_ordinal.insert((dll, ordinal), i);
    }

    /// Address of `dll!name` if known.
    pub fn lookup(&self, dll: &str, name: &str) -> Option<u32> {
        let dll = self.canon(dll);
        self.by_name.get(&(dll, name.to_string())).map(|&i| self.entries[i].addr)
    }

    /// Address of `dll!#ordinal` if known.
    pub fn lookup_ordinal(&self, dll: &str, ordinal: u16) -> Option<u32> {
        let dll = self.canon(dll);
        self.by_ordinal.get(&(dll, ordinal)).map(|&i| self.entries[i].addr)
    }

    /// Resolve an import by name, creating an auto stub when nobody declared it.
    pub fn resolve_import(&mut self, dll: &str, name: &str) -> u32 {
        if let Some(a) = self.lookup(dll, name) {
            return a;
        }
        let dll = self.canon(dll);
        self.add(dll, name.to_string(), None, true)
    }

    /// Resolve an import by ordinal, creating an auto stub `#ordinal` when unknown.
    pub fn resolve_import_ordinal(&mut self, dll: &str, ordinal: u16) -> u32 {
        if let Some(a) = self.lookup_ordinal(dll, ordinal) {
            return a;
        }
        let name = format!("#{ordinal}");
        let addr = self.resolve_import(dll, &name);
        let i = (addr - THUNK_BASE) as usize;
        self.entries[i].ordinal = Some(ordinal);
        let dll = self.canon(dll);
        self.by_ordinal.insert((dll, ordinal), i);
        addr
    }

    /// Entry for a thunk address.
    pub fn entry(&self, addr: u32) -> Option<&ThunkEntry> {
        self.entries.get(addr.checked_sub(THUNK_BASE)? as usize)
    }

    pub(crate) fn entry_mut(&mut self, addr: u32) -> Option<&mut ThunkEntry> {
        self.entries.get_mut(addr.checked_sub(THUNK_BASE)? as usize)
    }

    /// All entries.
    pub fn entries(&self) -> &[ThunkEntry] {
        &self.entries
    }

    /// `dll!name` for display.
    pub fn describe(&self, addr: u32) -> String {
        match self.entry(addr) {
            Some(e) => format!("{}!{}", e.dll, e.name),
            None => format!("<thunk {addr:#x}>"),
        }
    }

    /// Fake HMODULE for an HLE DLL (`LoadLibraryA`/`GetModuleHandleA`), or None if no function of that DLL
    /// was ever declared.
    pub fn module_handle(&self, dll: &str) -> Option<u32> {
        let dll = self.canon(dll);
        self.modules.iter().position(|m| *m == dll).map(|i| FAKE_MODULE_BASE + i as u32 * 0x1_0000)
    }

    /// Module name of a fake HMODULE.
    pub fn module_name(&self, hmodule: u32) -> Option<&str> {
        let off = hmodule.checked_sub(FAKE_MODULE_BASE)?;
        if off % 0x1_0000 != 0 {
            return None;
        }
        self.modules.get((off / 0x1_0000) as usize).map(|s| s.as_str())
    }
}
