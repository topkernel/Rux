//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 framework edge module (layer-1 doctrine, unsafe-slim round 5
//! lane L3, continuation): the saved-context frame primitive for the trap
//! / signal / fork entry paths. Together with `mm/unsafe.rs` (the
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

// ===========================================================================
// INV-ARCHDIAG-* — the arch-cold lane's audited diagnostic read edges
// (arch service crate). Every row is a READ-ONLY, best-effort
// bottom for a forensic walker that runs on an already-broken machine
// or a dying task; no row mutates kernel state, no row is for
// healthy-path use, and every consumer prints then parks/exits.
// ===========================================================================

/// INV-ARCHDIAG-1 (adoption): `tp` is a task pointer the per-CPU slot
/// or a cpu_state named one event ago (the same adoption proof as the
/// switch path). Returns the foreign-entry report's header fields:
/// (task address, pid, kernel-stack bottom, kernel-stack top).
pub fn diag_task_fields(tp: *mut crate::process::types::Task) -> (u64, u32, usize, usize) {
    // SAFETY: broken-machine report read of the task the per-CPU slot
    // named; short ops, read-only, the CPU parks after the report.
    match unsafe { crate::process::types::TaskBorrow::from_raw(tp) } {
        Some(t) => (
            t.as_ptr() as u64,
            t.pid(),
            t.kernel_stack_bottom(),
            t.get_kernel_stack().map_or(0, |p| p as usize),
        ),
        None => (0, 0, 0, 0),
    }
}

/// INV-ARCHDIAG-2 (adoption): `tp` is a CPU's current-task pointer read
/// from its pub cpu_state slot. Returns (pid, comm) for the remote-BT
/// requester's one-line attribution.
pub fn diag_task_pid_comm(tp: *const crate::process::types::Task) -> (u32, [u8; 16]) {
    if tp.is_null() {
        return (0, [0; 16]);
    }
    // SAFETY: read-only pid/comm read of the CPU-slot-named task; the
    // requester prints and drops the values immediately.
    unsafe { ((*tp).pid(), *(*tp).comm()) }
}

/// INV-ARCHDIAG-3 (best-effort read): `addr` is a kernel-half address
/// on a stack being reported (foreign frame words). A torn or
/// faulting read is acceptable — the report is the point.
pub fn diag_volatile_read_u64(addr: usize) -> u64 {
    // SAFETY: best-effort forensic read of a (possibly torn) kernel
    // stack word; the caller prints it and parks/exits.
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

/// INV-ARCHDIAG-4 (checked user read): walks the page table rooted at
/// `root_ppn` for `va` and reads the u64 through the linear map;
/// returns None on an unmapped hole (the walk is the check — a bad or
/// absent address can never take the kernel down).
pub fn diag_fore_read_u64(root_ppn: u64, va: u64) -> Option<u64> {
    // The walk is total for any input (the stale-frame guards inline):
    // a garbage root or recycled intermediate returns None.
    let phys = crate::arch::mm::mm_ops::PageTableWalker::walk(root_ppn, va & !0xfff)?;
    let kv = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new((phys.0 << 12) | (va & 0xfff)),
    );
    // SAFETY: kv points at the mapped user page through the linear map.
    Some(unsafe { core::ptr::read_volatile(kv.as_u64() as *const u64) })
}

/// INV-ARCHDIAG-4 (same walk, PTE form): returns (ppn, pte bits) for
/// the covering leaf, or None on a hole. Read-only.
pub fn diag_user_pte(root_ppn: u64, va: u64) -> Option<(u64, u64)> {
    crate::arch::mm::mm_ops::PageTableWalker::walk(root_ppn, va & !0xfff)
}

/// INV-ARCHDIAG-5 (descriptor snapshot): read-only frame-descriptor
/// snapshot for the forensic PTE dump — (refcount, mapcount, flags_raw,
/// rmap_index); the -999/0 sentinels mirror the no-descriptor case the
/// report prints.
pub fn diag_frame_desc(ppn: u64) -> (i64, i64, u64, usize) {
    let frame = (ppn as usize) << 12;
    let pd = crate::mm::hotpath::pfn_to_page_mut(crate::mm::hotpath::phys_to_pfn(frame));
    if pd.is_null() {
        return (-999, -999, 0, 0);
    }
    // SAFETY: read-only descriptor reads of the mapped frame.
    unsafe {
        (
            (*pd).refcount() as i64,
            (*pd).mapcount() as i64,
            (*pd).flags().raw() as u64,
            (*pd).index(),
        )
    }
}

/// INV-ARCHDIAG-6 (park): the crashed-forensic-context permanent park —
/// hlt with IF=0 never wakes. The re-entrancy latch path in the fatal
/// SIGSEGV report uses this; it never returns.
pub fn diag_halt_forever() -> ! {
    // SAFETY: the intentional permanent park of a crashed forensic
    // context — interrupts were disabled by the caller.
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}
