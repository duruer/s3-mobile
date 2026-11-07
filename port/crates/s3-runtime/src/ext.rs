//! Extension points for host-side mods (`s3-mods`): guest-address hooks, an input filter in front of the
//! game, and an overlay compose step on every presented frame. All are plain function pointers that keep
//! their state in [`Machine::state`]; when nothing is installed they cost (almost) nothing.

use std::sync::Arc;

use s3_cpu::HookAddrs;

use crate::host::{FrameBuffer, HostEvent};
use crate::machine::Machine;

/// What the CPU does after a hook ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFlow {
    /// Execute the original code at the hooked EIP (the hook only observed / changed state).
    Continue,
    /// The hook set EIP (and usually ESP/EAX) itself, e.g. emulated a `ret` to skip the original function.
    Redirected,
}

/// Called when EIP reaches a hooked address (`eip`). The current thread is `m.cur`.
pub type HookDispatch = fn(&mut Machine, u32) -> HookFlow;
/// Called for every host event before the game sees it. Push the events the game should see into `out`
/// (none = consumed; the identity filter pushes `e`).
pub type EventFilter = fn(&mut Machine, HostEvent, &mut Vec<HostEvent>);
/// Overlay compose: `src` is the guest frame; draw into `out` (prefilled with a copy of `src`) and return true,
/// or return false to present `src` unchanged (no copy made by the caller beyond the prefill skip, see
/// [`OverlayWanted`]).
pub type OverlayCompose = fn(&mut Machine, &mut FrameBuffer) -> bool;
/// Cheap check before [`OverlayCompose`]: does the overlay draw anything now? (avoids a frame copy).
pub type OverlayWanted = fn(&mut Machine) -> bool;

/// Installed extensions (see [`Machine::ext`]).
#[derive(Default)]
pub struct MachineExt {
    pub hook_dispatch: Option<HookDispatch>,
    pub event_filter: Option<EventFilter>,
    pub overlay_wanted: Option<OverlayWanted>,
    pub overlay_compose: Option<OverlayCompose>,
    /// Hook address set given to every guest CPU (also to threads created later).
    pub(crate) hook_addrs: Option<Arc<HookAddrs>>,
    /// Native fast paths given to every guest CPU (also to threads created later); see `Cpu::set_native_blocks`.
    pub(crate) native_blocks: Option<Arc<s3_cpu::NativeBlocks>>,
    /// Reused frame for overlay composition.
    pub(crate) overlay_buf: Option<FrameBuffer>,
    /// Last guest frame (kept only while an overlay is installed) for overlay-only refreshes.
    pub(crate) last_guest_frame: Option<FrameBuffer>,
    /// Size of the last guest frame (valid also while `last_guest_frame` is taken out for composing).
    pub(crate) last_guest_size: (u32, u32),
}

impl Machine {
    /// Install the guest hook address set in every thread's CPU (clears their decode caches). `None` or an
    /// empty set disables hooks.
    pub fn set_hook_addrs(&mut self, addrs: Option<HookAddrs>) {
        let a = addrs.filter(|s| !s.is_empty()).map(Arc::new);
        for t in &mut self.threads {
            t.cpu.set_hooks(a.clone());
        }
        self.ext.hook_addrs = a;
    }

    /// Install native fast paths (perf-cpu, docs/porting/performance.md) in every thread's CPU (clears their
    /// decode caches). `None` or an empty map disables them.
    pub fn set_native_blocks(&mut self, natives: Option<s3_cpu::NativeBlocks>) {
        let n = natives.filter(|s| !s.is_empty()).map(Arc::new);
        for t in &mut self.threads {
            t.cpu.set_native_blocks(n.clone());
        }
        self.ext.native_blocks = n;
    }

    /// Current native fast paths.
    pub fn native_blocks(&self) -> Option<&s3_cpu::NativeBlocks> {
        self.ext.native_blocks.as_deref()
    }

    /// Size of the last guest frame seen by the overlay step ((0, 0) before the first present).
    pub fn last_frame_size(&self) -> (u32, u32) {
        self.ext.last_guest_size
    }

    /// Current hook address set.
    pub fn hook_addrs(&self) -> Option<&HookAddrs> {
        self.ext.hook_addrs.as_deref()
    }

    /// Run the hook for `eip` on the current thread and prepare the CPU to continue.
    pub(crate) fn dispatch_hook(&mut self, eip: u32) {
        let slot = self.cur;
        self.stats.hooks += 1;
        let flow = match self.ext.hook_dispatch {
            Some(f) => f(self, eip),
            None => HookFlow::Continue,
        };
        self.cur = slot;
        if flow == HookFlow::Continue {
            let cpu = &mut self.threads[slot].cpu;
            if cpu.eip == eip {
                cpu.skip_hook_once(eip);
            }
        }
    }

    /// Feed one host event through the installed filter into `self.input`.
    pub(crate) fn filter_event(&mut self, e: HostEvent) {
        match self.ext.event_filter {
            Some(f) => {
                let mut out = Vec::new();
                f(self, e, &mut out);
                for e in out {
                    self.input.push(e);
                }
            }
            None => self.input.push(e),
        }
    }

    /// Apply the overlay to a guest frame. Returns the frame to present (`None` = present `fb` unchanged).
    pub(crate) fn apply_overlay(&mut self, fb: &FrameBuffer) -> Option<FrameBuffer> {
        self.ext.overlay_compose?;
        // Remember the raw guest frame for overlay-only refreshes.
        let mut last = self.ext.last_guest_frame.take().unwrap_or_default();
        copy_frame(&mut last, fb);
        self.ext.last_guest_size = (fb.width, fb.height);
        self.ext.last_guest_frame = Some(last);
        self.apply_overlay_to(fb)
    }

    /// Overlay over a copy of `fb` (which is already remembered as the last guest frame, or a refresh of it).
    pub(crate) fn apply_overlay_to(&mut self, fb: &FrameBuffer) -> Option<FrameBuffer> {
        let compose = self.ext.overlay_compose?;
        if let Some(w) = self.ext.overlay_wanted {
            if !w(self) {
                return None;
            }
        }
        let mut out = self.ext.overlay_buf.take().unwrap_or_default();
        copy_frame(&mut out, fb);
        if compose(self, &mut out) {
            Some(out)
        } else {
            self.ext.overlay_buf = Some(out);
            None
        }
    }

    /// Give a frame buffer back to the overlay buffer pool.
    pub(crate) fn recycle_overlay(&mut self, fb: FrameBuffer) {
        self.ext.overlay_buf = Some(fb);
    }

    /// Re-present the last guest frame with the current overlay (call when only the overlay changed, e.g.
    /// a HUD animation or touch feedback while the game presents nothing). Not counted in `stats.presents`.
    pub fn refresh_overlay(&mut self) {
        let Some(src) = self.ext.last_guest_frame.take() else { return };
        match self.apply_overlay_to(&src) {
            Some(out) => self.present_cursor_owned(out),
            None => self.present_cursor(&src),
        }
        if self.ext.last_guest_frame.is_none() {
            self.ext.last_guest_frame = Some(src);
        }
    }
}

fn copy_frame(dst: &mut FrameBuffer, src: &FrameBuffer) {
    dst.width = src.width;
    dst.height = src.height;
    dst.pixels.clear();
    dst.pixels.extend_from_slice(&src.pixels);
}
