//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! BINDER EDGES — the audited unsafe zone the MOVED binder diagnostics
//! consume (cold-four lane): the trap-regs readers, the user page-table
//! probe walkers and the kernel-stack word reader moved here verbatim
//! from binder.rs when the cold diagnostic halves flipped to the
//! rux-ipc crate, plus the two typed task edges those halves' raw
//! derefs converted to. Everything here is diagnostic-path only — no
//! txn-path site calls into this file.
//!
//! INV rows:
//! - INV-BINDER-ENTRYFRAME (carried): the entry_frame* readers walk a
//!   task's kernel stack top for a pt_regs-shaped frame; provenance is
//!   a live task observed under the pid-hash walk or WORLD.
//! - INV-BINDER-PROBE (carried): probe_leaf/u32/u64 translate a USER
//!   virtual address through the target's root pgd — read-only.
//! - INV-BINDER-DIAG-COMM (carried): the ONE audited raw-task comm
//!   read per audit pass.
//! - INV-BINDER-DIAG-TASKREF (new): the diagnostic &Task borrow of a
//!   raw task pointer from the binder object model; the caller's
//!   context (WORLD held / pid-hash bucket) pins the task.

pub fn entry_frame(top: usize) -> (u64, u64, u64, u64, u64) {
    if top == 0 {
        return (0, 0, 0, 0, 0);
    }
    #[cfg(feature = "x86_64")]
    {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of this task's own entry frame; the
        // caller obtained `top` from kernel_stack_top_checked() (or the
        // owns_kernel_stack() gate), so the frame pages are mapped.
        unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi, (*regs).rsi, (*regs).rdx) }
    }
    #[cfg(feature = "riscv64")]
    {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract as the x86_64 arm.
        unsafe { ((*regs).epc, (*regs).a7, (*regs).a0, (*regs).a1, (*regs).a2) }
    }
    #[cfg(feature = "aarch64")]
    {
        let regs = (top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::aarch64::pt_regs::PtRegs;
        // SAFETY: same contract as the x86_64 arm.
        unsafe { ((*regs).pc, (*regs).syscallno, (*regs).regs[0], (*regs).regs[1], (*regs).regs[2]) }
    }
    #[cfg(not(any(feature = "x86_64", feature = "riscv64", feature = "aarch64")))]
    (0, 0, 0, 0, 0)
}

/// Diagnostic entry-frame read of the user stack pointer + pc: (sp, pc).
/// Zeros when `top == 0` (see the INV-BINDER-ENTRYFRAME header).
#[inline]
pub fn entry_frame_sp(top: usize) -> (u64, u64) {
    if top == 0 {
        return (0, 0);
    }
    #[cfg(feature = "x86_64")]
    {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: as entry_frame.
        unsafe { ((*regs).rsp, (*regs).rip) }
    }
    #[cfg(feature = "riscv64")]
    {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: as entry_frame.
        unsafe { ((*regs).sp, (*regs).epc) }
    }
    #[cfg(feature = "aarch64")]
    {
        let regs = (top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::aarch64::pt_regs::PtRegs;
        // SAFETY: as entry_frame.
        unsafe { ((*regs).sp, (*regs).pc) }
    }
    #[cfg(not(any(feature = "x86_64", feature = "riscv64", feature = "aarch64")))]
    (0, 0)
}

/// Diagnostic leaf walk: (ppn, pte-bits) of the mapping at `va` in the
/// target's live user page tables. INV-BINDER-PROBE: read-only, the one
/// audited walk span all probes build on.
#[inline]
pub fn probe_leaf(root: u64, va: u64) -> Option<(u64, u64)> {
    // SAFETY: read-only diagnostic walk of live user page tables.
    unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) }
}

/// Diagnostic cross-address-space probe: walk the target's live user
/// page tables and volatile-read one u32 from the mapped page. Returns
/// None when the page is unmapped. INV-BINDER-PROBE: read-only and
/// fault-free — the leaf is walked first and the read happens only
/// through the walked, present mapping.
#[inline]
pub fn probe_u32(root: u64, addr: u64) -> Option<u32> {
    probe_leaf(root, addr).map(|(ppn, _)| {
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
            as usize
            + (addr as usize & 0xfff);
        // SAFETY: the walked leaf maps this user word.
        unsafe { core::ptr::read_volatile(kv as *const u32) }
    })
}

/// u64 twin of [`probe_u32`] (INV-BINDER-PROBE).
#[inline]
pub fn probe_u64(root: u64, addr: u64) -> Option<u64> {
    probe_leaf(root, addr).map(|(ppn, _)| {
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
            as usize
            + (addr as usize & 0xfff);
        // SAFETY: the walked leaf maps this user word.
        unsafe { core::ptr::read_volatile(kv as *const u64) }
    })
}

/// x86-only entry-frame triple the futex decode needs: (rbp, rsp, r10).
/// INV-BINDER-ENTRYFRAME arm; zeros when `top == 0`.
#[cfg(feature = "x86_64")]
#[inline]
pub fn entry_frame_futex_x86(top: usize) -> (u64, u64, u64) {
    if top == 0 {
        return (0, 0, 0);
    }
    let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
        as *const crate::arch::x86_64::pt_regs::PtRegs;
    // SAFETY: diagnostic read of this task's own entry frame; the
    // caller gated the stack via owns_kernel_stack/kernel_stack_top_checked.
    unsafe { ((*regs).rbp, (*regs).rsp, (*regs).r10) }
}

/// Diagnostic kernel-stack word read: bounds-check `addr` inside the
/// task's own kernel-stack window [base, top) and volatile-read one u64.
/// INV-BINDER-STACKWIN: the only raw kernel-stack-window reads in this
/// file's diagnostics. (The pre-conversion scans bounds-checked only the
/// FIRST word and then walked up to 640 more — reads past `top` were
/// never part of the documented "inside the task's own kernel-stack
/// window" contract; the per-word check enforces it.)
#[inline]
pub fn stack_word(addr: usize, base: usize, top: usize) -> Option<u64> {
    if addr < base || addr + 8 > top || addr & 7 != 0 {
        return None;
    }
    // SAFETY: addr is inside the task's own kernel-stack window,
    // 8-byte aligned, and the full word fits below `top`.
    Some(unsafe { core::ptr::read_volatile(addr as *const u64) })
}


/// INV-BINDER-DIAG-COMM: the audited raw-task comm read the moved
/// audits share (was the per-audit `comm_of` closure; one edge now).
pub fn diag_task_comm(p: *mut crate::process::Task) -> &'static str {
    if p.is_null() {
        return "?";
    }
    // SAFETY: diagnostic comm read of a live task (INV above; the
    // pointer's provenance is a live pid-hash or walk callback task).
    let c = unsafe { (*p).comm() };
    core::str::from_utf8(&c[..])
        .unwrap_or("?")
        .trim_end_matches('\0')
}

/// INV-BINDER-DIAG-TASKREF: diagnostic &Task borrow of a raw task
/// pointer from the binder object model (proc.task / walk callbacks).
/// The caller's context pins the task for the borrow's life.
pub fn diag_task<'a>(p: *mut crate::process::Task) -> &'a crate::process::Task {
    // SAFETY: read-only diagnostic borrow; the owner is alive under the
    // caller's WORLD section / pid-hash bucket (INV above).
    unsafe { &*p }
}
