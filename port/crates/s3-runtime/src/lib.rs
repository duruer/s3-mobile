//! Runtime for the Settlers III port: the [`Machine`] (guest memory + guest threads + scheduler), the thunk
//! [`Registry`] that connects guest imports to Rust HLE handlers, the handle table, SEH, COM helpers, VFS, and
//! the [`Host`] trait implemented by frontends.
//!
//! See `docs/porting/architecture.md` for the big picture and how to add an HLE function.

pub mod audio;
pub mod com;
pub mod cursor;
pub mod ext;
pub mod gdi;
pub mod handles;
pub mod host;
pub mod hosttime;
pub mod input;
pub mod machine;
pub mod mapfile;
pub mod mt;
pub mod process;
pub mod registry;
pub mod resources;
pub mod seh;
pub mod vfs;

#[cfg(test)]
mod tests;

pub use com::Guid;
pub use gdi::DcTarget;
pub use handles::{HandleTable, KernelObject};
pub use ext::{HookFlow, MachineExt};
pub use host::{FrameBuffer, Host, HostEvent, LogLevel, MouseButton, NullHost, PointerPhase};
pub use machine::{Machine, ProcessInfo, Service, Shared, SliceResult, Stats, StopReason, Thread, ThreadState, WaitPoll};
pub use registry::{normalize_dll, Handler, Registry, ThunkEntry, ThunkResult};
pub use vfs::{Vfs, GUEST_GAME_DIR};

pub use s3_cpu as cpu;
pub use s3_mem as mem;
