//! Win32 handle table. Handles are small multiples of 4 (like NT). Several handles can refer to one object
//! (`DuplicateHandle`, named objects); the object dies when its last handle is closed.

use std::any::Any;
use std::collections::HashMap;

/// Pseudo handle returned by `GetCurrentProcess`.
pub const CURRENT_PROCESS: u32 = 0xFFFF_FFFF;
/// Pseudo handle returned by `GetCurrentThread`.
pub const CURRENT_THREAD: u32 = 0xFFFF_FFFE;
/// `INVALID_HANDLE_VALUE`.
pub const INVALID_HANDLE_VALUE: u32 = 0xFFFF_FFFF;

/// A kernel object. Waitable kinds are understood by `Machine::is_signaled` / `Machine::try_acquire`.
pub enum KernelObject {
    Event { manual_reset: bool, signaled: bool },
    Mutex { owner: Option<u32>, recursion: u32 },
    Semaphore { count: i32, max: i32 },
    /// A guest thread (signaled when it has exited).
    Thread { tid: u32 },
    Process,
    /// Waitable timer (`CreateWaitableTimerA`). Signaled once host time reaches `due_ms`; `period_ms > 0`
    /// re-arms it (see `Machine::is_signaled` / `try_acquire`).
    Timer { manual_reset: bool, signaled: bool, due_ms: Option<u64>, period_ms: u32 },
    /// Anything owned by an HLE module (files, find handles, file mappings, MCI devices, ...).
    /// The string is a short type tag for debugging.
    Other(&'static str, Box<dyn Any + Send>),
}

impl std::fmt::Debug for KernelObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KernelObject::Event { manual_reset, signaled } => write!(f, "Event(manual={manual_reset}, set={signaled})"),
            KernelObject::Mutex { owner, recursion } => write!(f, "Mutex(owner={owner:?}, rec={recursion})"),
            KernelObject::Semaphore { count, max } => write!(f, "Semaphore({count}/{max})"),
            KernelObject::Thread { tid } => write!(f, "Thread({tid:#x})"),
            KernelObject::Process => write!(f, "Process"),
            KernelObject::Timer { due_ms, period_ms, .. } => write!(f, "Timer(due={due_ms:?}, period={period_ms})"),
            KernelObject::Other(t, _) => write!(f, "{t}"),
        }
    }
}

struct ObjEntry {
    obj: KernelObject,
    name: Option<String>,
    refs: u32,
}

/// The process handle table.
#[derive(Default)]
pub struct HandleTable {
    next_handle: u32,
    next_obj: u32,
    handles: HashMap<u32, u32>,
    objects: HashMap<u32, ObjEntry>,
}

impl HandleTable {
    pub fn new() -> Self {
        HandleTable { next_handle: 0x100, next_obj: 1, ..Default::default() }
    }

    fn alloc_handle(&mut self, obj: u32) -> u32 {
        let h = self.next_handle;
        self.next_handle += 4;
        self.handles.insert(h, obj);
        h
    }

    /// Insert a new object and return a handle to it.
    pub fn insert(&mut self, obj: KernelObject) -> u32 {
        self.insert_named(obj, None)
    }

    /// Insert a new (optionally named) object.
    pub fn insert_named(&mut self, obj: KernelObject, name: Option<String>) -> u32 {
        let id = self.next_obj;
        self.next_obj += 1;
        self.objects.insert(id, ObjEntry { obj, name, refs: 1 });
        self.alloc_handle(id)
    }

    /// New handle to an existing named object (for `Create*`/`Open*` with an existing name).
    pub fn open_named(&mut self, name: &str) -> Option<u32> {
        let id = *self.objects.iter().find(|(_, e)| e.name.as_deref() == Some(name))?.0;
        self.objects.get_mut(&id)?.refs += 1;
        Some(self.alloc_handle(id))
    }

    /// The object behind a handle.
    pub fn get(&self, h: u32) -> Option<&KernelObject> {
        self.objects.get(self.handles.get(&h)?).map(|e| &e.obj)
    }

    /// Mutable object behind a handle.
    pub fn get_mut(&mut self, h: u32) -> Option<&mut KernelObject> {
        let id = *self.handles.get(&h)?;
        self.objects.get_mut(&id).map(|e| &mut e.obj)
    }

    /// Downcast an `Other` object.
    pub fn get_other<T: 'static>(&self, h: u32) -> Option<&T> {
        match self.get(h)? {
            KernelObject::Other(_, b) => b.downcast_ref(),
            _ => None,
        }
    }

    /// Mutable downcast of an `Other` object.
    pub fn get_other_mut<T: 'static>(&mut self, h: u32) -> Option<&mut T> {
        match self.get_mut(h)? {
            KernelObject::Other(_, b) => b.downcast_mut(),
            _ => None,
        }
    }

    /// `DuplicateHandle`.
    pub fn duplicate(&mut self, h: u32) -> Option<u32> {
        let id = *self.handles.get(&h)?;
        self.objects.get_mut(&id)?.refs += 1;
        Some(self.alloc_handle(id))
    }

    /// `CloseHandle`. Returns the object if this was its last handle.
    pub fn close(&mut self, h: u32) -> Result<Option<KernelObject>, ()> {
        let id = self.handles.remove(&h).ok_or(())?;
        let e = self.objects.get_mut(&id).ok_or(())?;
        e.refs -= 1;
        if e.refs == 0 {
            return Ok(self.objects.remove(&id).map(|e| e.obj));
        }
        Ok(None)
    }

    /// Identity of the object behind a handle (equal for duplicated / re-opened handles).
    pub fn object_id(&self, h: u32) -> Option<u32> {
        self.handles.get(&h).copied()
    }

    /// Name of the object behind a handle, if it was created named.
    pub fn name_of(&self, h: u32) -> Option<&str> {
        self.objects.get(self.handles.get(&h)?)?.name.as_deref()
    }

    /// Whether `h` is a live handle.
    pub fn contains(&self, h: u32) -> bool {
        self.handles.contains_key(&h)
    }

    /// Number of open handles.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}
