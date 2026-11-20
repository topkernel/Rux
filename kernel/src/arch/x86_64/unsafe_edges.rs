//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 framework edge module (layer-1 doctrine, unsafe-slim round 5
//! lane L3, continuation): the saved-context frame primitive for the trap
//! / signal / fork entry paths. Together with `mm/unsafe_edges.rs` (the
//! page-table family) this is one of the arch's two framework files.
//!
//! The reference kernel (Linux x86) handles the same frames as raw
//! `struct pt_regs *` pointers throughout C (arch/x86/entry/...); Rux's
//! difference is that forming the reference is ONE audited edge and
//! everything above it is compiler-checked.
//!
//! # Safety invariants — the saved-context frame (INV-XE-FRAME-*)
//!
//! - **INV-XE-FRAME-1 (origin)** — [`SavedFrame`] is constructed only
//!   from a pointer an entry stub handed the handler (the trap.S stubs
//!   push a complete PtRegs and pass its address), or from a task's
//!   canonical `pt_regs()` slot (fork/exec/signal paths). Both origins
//!   produce a fully-formed PtRegs by construction.
//! - **INV-XE-FRAME-2 (liveness)** — the frame lives on the interrupted
//!   task's kernel stack (or the newborn's published frame) and stays
//!   mapped for the handler's duration: the handler may return through
//!   it or schedule away, but the memory is not freed until the owning
//!   task exits (guard: frame_in_task_stack at entry, R64 rsp0
//!   validation at switch-in).
//! - **INV-XE-FRAME-3 (aliasing)** — exactly one &mut frame handle
//!   exists per entry; nesting (IRQ inside trap) creates frames at
//!   distinct stack addresses, and the per-CPU CURRENT_PT_REGS slot
//!   saves/restores the OUTER pointer — never aliasing two handles to
//!   one frame.

use super::pt_regs::PtRegs;

/// Typed handle to the saved register frame of the current trap entry
/// (INV-XE-FRAME-*). Derefs to the live `PtRegs` — every field access,
/// read or write, is plain safe Rust through this handle.
pub struct SavedFrame<'a> {
    inner: &'a mut PtRegs,
}

impl<'a> SavedFrame<'a> {
    /// The audited entry edge (INV-XE-FRAME-1/2).
    ///
    /// # Safety
    /// `regs` must be the frame pointer an entry stub passed to this
    /// handler (or the task's canonical `pt_regs()` slot): a complete,
    /// live PtRegs per the invariants above. The handle borrows it for
    /// `'a` — the caller must not form another aliasing reference in
    /// that window.
    #[inline]
    pub unsafe fn from_trap(regs: *mut PtRegs) -> SavedFrame<'a> {
        // SAFETY: INV-XE-FRAME-1/2 — the one audited reference-forming
        // edge; the trap-entry contract supplies the proof.
        SavedFrame {
            inner: unsafe { &mut *regs },
        }
    }

    /// The live frame (field access is safe through the reference).
    #[inline]
    pub fn regs(&mut self) -> &mut PtRegs {
        self.inner
    }
}

impl<'a> core::ops::Deref for SavedFrame<'a> {
    type Target = PtRegs;
    #[inline]
    fn deref(&self) -> &PtRegs {
        self.inner
    }
}

impl<'a> core::ops::DerefMut for SavedFrame<'a> {
    #[inline]
    fn deref_mut(&mut self) -> &mut PtRegs {
        self.inner
    }
}
