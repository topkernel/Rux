//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V page fault handling
//!
//! Processing flow:
//! 1. Distinguish kernel/user mode
//! 2. Check interrupt context
//! 3. Find VMA
//! 4. Verify permissions
//! 5. Handle COW
//! 6. Handle anonymous pages
//! 7. Send signal or OOM
//!
//! # Exception Table Mechanism
//!
//! Exception tables are used to safely handle exceptions that may occur
//! when the kernel accesses user space.
//! Typical use cases:
//! - `copy_to_user()`: Copy data from kernel to user space
//! - `copy_from_user()`: Copy data from user space to kernel
//! - `get_user()`: Read single value from user space
//! - `put_user()`: Write single value to user space
//!
//! When these operations access invalid user addresses, a page fault is triggered.
//! The exception table records each access instruction that may fail and its fixup handler.
//! If a page fault occurs on these instructions, the kernel jumps to the fixup handler
//! instead of crashing.

use crate::arch::pt_regs::PtRegs;
use crate::arch::mm::{VirtAddr, FaultFlags, AddressSpace, handle_cow_fault, handle_mm_fault};

// Re-export MmFaultResult from page_fault (canonical definition)
pub use super::page_fault::MmFaultResult;
use crate::println;
use crate::process::task::TaskState;
use crate::mm::vma::VmaFlags;

/// Exception table entry
///
/// Used for exception fixup when kernel accesses user space.
/// When the kernel has an exception at the specified address, jump to fixup address to continue.
///
/// # Memory layout
/// Each entry occupies 16 bytes (2 × 8 byte addresses)
#[repr(C)]
pub struct ExceptionTableEntry {
    /// Instruction address where exception may occur (PC value)
    pub insn: u64,
    /// Jump address after fixup (position to continue after handling exception)
    pub fixup: u64,
}

/// Exception table boundary symbols (defined by linker script)
extern "C" {
    /// Exception table start address
    static __ex_table_start: ExceptionTableEntry;
    /// Exception table end address
    static __ex_table_end: ExceptionTableEntry;
}

/// Find fixup address in exception table
///
/// Uses linear search to find matching instruction address in exception table.
/// If found, returns fixup address; otherwise returns None.
///
/// # Arguments
/// - `addr`: Instruction address where exception occurred (usually EPC value)
///
/// # Returns
/// - `Some(fixup_addr)`: Found fixup address
/// - `None`: No matching entry found
///
/// # Performance
/// Linear search O(n), but exception table is usually small (tens to hundreds of entries),
/// performance impact is acceptable. Can use binary search for optimization (requires sorted table).
pub fn fixup_exception(addr: u64) -> Option<u64> {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;

        // Calculate number of entries in table
        let count = (end as usize - start as usize) / core::mem::size_of::<ExceptionTableEntry>();

        // Linear search
        for i in 0..count {
            let entry = &*start.add(i);
            if entry.insn == addr {
                return Some(entry.fixup);
            }
        }
    }

    None
}

/// Check if exception table is empty
#[allow(dead_code)]
pub fn exception_table_empty() -> bool {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;
        start == end
    }
}

/// Get exception table entry count
#[allow(dead_code)]
pub fn exception_table_count() -> usize {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;
        (end as usize - start as usize) / core::mem::size_of::<ExceptionTableEntry>()
    }
}

/// Send signal to current process
///
/// # Arguments
/// - `sig`: Signal number
/// - `code`: Signal code (SI_XXX)
/// - `addr`: Address that triggered exception
/// - `epc`: Instruction address where exception occurred
/// - `access_type`: Access type
/// - `regs`: PtRegs pointer, used to get user mode tp
fn send_signal(sig: i32, code: i32, _addr: u64, _epc: u64, _access_type: u32, _regs: &crate::arch::pt_regs::PtRegs) {
    // Send signal using real signal mechanism. The queued siginfo carries
    // the si_code the caller classified (SEGV_MAPERR/SEGV_ACCERR): a fatal
    // hardware-fault death then prints a real origin in SIGDEATH/KILLTRACE
    // instead of the indistinguishable 0/0 of the info-less bitmap path.
    if let Some(current) = crate::sched::current() {
        let pid = current.pid();
        crate::signal::send_signal_with_info(
            pid,
            crate::signal::SigInfo::new(sig, code, 0, 0),
        )
        .ok();

        // Wake up process to handle signal (if it's sleeping)
        crate::signal::signal_wake_up(current as *mut _);
    }
}

/// Check if in interrupt context
#[inline]
fn in_interrupt() -> bool {
    crate::interrupt::preempt::in_interrupt()
}

/// Page fault handling - bad_area path
///
/// Called when address is not in a valid VMA
fn bad_area(regs: &mut PtRegs, access_type: u32, fault_addr: VirtAddr) -> MmFaultResult {
    // User mode accessing invalid address
    if regs.user_mode() {
        send_signal(11, 1, fault_addr.bits(), regs.epc, access_type, regs);  // SIGSEGV, SEGV_MAPERR = 1
        return MmFaultResult::Segfault;
    }

    // Kernel mode accessing invalid address
    // Check exception table
    if let Some(fixup) = fixup_exception(regs.epc) {
        regs.epc = fixup;
        return MmFaultResult::Fixed;
    }

    // Cannot fix, kernel panic
    MmFaultResult::KernelPanic
}

/// Page fault handling - no_context path
///
/// Called when valid process context cannot be obtained
fn no_context(_regs: &mut PtRegs, _fault_addr: VirtAddr) -> MmFaultResult {
    // Check exception table
    if let Some(fixup) = fixup_exception(_regs.epc) {
        _regs.epc = fixup;
        return MmFaultResult::Fixed;
    }

    // Cannot handle
    MmFaultResult::KernelPanic
}

/// do_page_fault - Page fault handling main function
///
/// # Arguments
/// - `regs`: Trap frame/register state
/// - `access_type`: Access type (FaultFlags)
///
/// # Returns
/// Handling result
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    let fault_addr = VirtAddr::new(regs.badaddr);

    crate::pr_debug!("do_page_fault: addr={:#x}, epc={:#x}, type={:#x}, mode={}",
        fault_addr.bits(), regs.epc, access_type,
        if regs.kernel_mode() { "kernel" } else { "user" });

    // Get current process's address space
    let current = match crate::sched::current() {
        Some(t) => t,
        None => {
            // No current process, might be early boot stage
            return no_context(regs, fault_addr);
        }
    };

    let addr_space = match current.address_space() {
        Some(aspace) => aspace,
        None => {
            // Kernel thread has no address space
            return no_context(regs, fault_addr);
        }
    };

    // Check if in interrupt context
    if in_interrupt() {
        // R74 (preempt-count corruption self-heal): a NEGATIVE
        // preempt_count is never legal — it means an unpaired decrement
        // (double-executed spinlock-guard drop: the double-run engine's
        // fingerprint; r74g/r74h captures: two harts both executing as
        // the same pid, both faulting in user mode with preempt == -1
        // and dying through this no_context path as "in interrupt").
        // Treat it as corruption, not an IRQ context: reset the count and
        // let the fault be served as the ordinary user fault it is. The
        // task keeps running (and scheduling) instead of taking the
        // whole machine down; the R74-DOUBLE-SEED / noctx prints keep
        // naming the underlying double-run for the producer hunt.
        let pc = crate::interrupt::preempt::preempt_count();
        if pc < 0 {
            use core::fmt::Write;
            let mut w = crate::dfx::backtrace::ConsoleWriter::new();
            let _ = write!(w,
                "trap: PREEMPT-UNDERFLOW pid={} pc={:#x} — healed to 0\n",
                crate::sched::get_current_pid(), pc);
            // R83 (R74 producer hunt): the underflow victim's header may
            // already carry the foreign store — dump it while it is fresh.
            if let Some(cur) = crate::sched::current() {
                crate::dfx::r83hunt::dump_victim(b"PREEMPT-UF", cur as *mut _);
            }
            crate::interrupt::preempt::preempt_count_reset();
        } else {
            // Cannot sleep in interrupt context
            return no_context(regs, fault_addr);
        }
    }

        // Kernel mode access
        if regs.kernel_mode() {
            // R82 (uaccess demand-fill BEFORE fixup — the x86_64 "X3 fix"
            // mirrored here): R78 hoisted the exception-table fixup above
            // everything else in this block, which silently shadowed the
            // kernel_data_fill branch below — every kernel uaccess fault on
            // a VMA-covered but not-yet-present/COW page (first kernel touch
            // of an untouched .rodata string, fresh stdio/NSS buffer, COW
            // stack) returned EFAULT instead of faulting the page in. That
            // re-introduced the exact family the fill branch was added to
            // fix: dbus-daemon "Failed to open /dev/null: Bad address",
            // getpwuid_r EFAULT, XIO fatal IO error 14, ls statfs/fstatat
            // EFAULT -> uninitialized stat buffer -> SIGSEGV (fire-gate-
            // merged-r1 t=125/142s). Linux resolves the fault via
            // handle_mm_fault for kernel accesses too and only fixups to a
            // short copy when the address is genuinely unresolvable.
            //
            // EXEC faults are EXCLUDED (see the comment on the fill below).
            let kernel_data_fill = fault_addr.bits()
                < crate::arch::mm::user_addr::USER_END as u64
                && (access_type & FaultFlags::EXEC) == 0;
            if kernel_data_fill {
                let covered = addr_space
                    .vma_read()
                    .find(crate::mm::page::VirtAddr::new(fault_addr.as_usize()))
                    .is_some();
                if covered {
                    let result = handle_mm_fault(
                        &addr_space,
                        fault_addr,
                        access_type | FaultFlags::USER,
                    );
                    match result {
                        MmFaultResult::Handled | MmFaultResult::Fixed => {
                            // Re-execute the faulting kernel load/store.
                            return MmFaultResult::Handled;
                        }
                        MmFaultResult::CowPending => {
                            // Kernel WRITE uaccess onto a COW page: resolve
                            // it like the user-mode path below (retry counts
                            // as progress — the re-executed store either
                            // succeeds or re-faults into the right path).
                            match unsafe {
                                handle_cow_fault(addr_space, addr_space.root_ppn(), fault_addr)
                            } {
                                crate::arch::mm::CowFaultResult::Resolved
                                | crate::arch::mm::CowFaultResult::Retry => {
                                    return MmFaultResult::Handled;
                                }
                                crate::arch::mm::CowFaultResult::OutOfMemory => {}
                            }
                        }
                        // Unresolvable (no perms, OOM, ...): fall through to
                        // the exception-table fixup so the copy reports a
                        // short copy.
                        _ => {}
                    }
                }
            }

            // R78 (uaccess-first fixup discipline vs the stack-overflow
            // panic): a faulting epc that has a fixup entry is BY
            // CONSTRUCTION inside a copy_to_user/copy_from_user window —
            // the right recovery is the fixup (short copy → EFAULT),
            // letting the syscall unwind normally (dropping any locks its
            // frames hold). The old order ran the stack-overflow check
            // first and PANICKED on the three GNOME full-session terminal
            // captures (r75-full1 t≈360, inst-full1 t≈960, inst-full3
            // t≈110: copy_user load/store faults at wild user addresses on
            // a cross-stack sp), each of which carried EXT4_BIG_LOCK into
            // the grave and wedged every later ext4 user in D-state
            // (VW-EXT4LOCK owner=<victim> queued=3). With fixup-first
            // those deaths become an EFAULT return + clean task kill;
            // genuine kernel corruption (no fixup entry) still reaches the
            // loud stack-overflow panic below unchanged. Wild addresses
            // are not VMA-covered, so the demand-fill above already
            // declined them.
            if let Some(fixup) = fixup_exception(regs.epc) {
                regs.epc = fixup;
                return MmFaultResult::Fixed;
            }

            // Check for kernel stack overflow
            // If fault address is near current task's kernel stack, it's likely a stack overflow
            let fault_addr_usize = fault_addr.bits() as usize;

            if current.is_in_kernel_stack(fault_addr_usize) || current.is_stack_overflow(regs.sp as usize) {
            // R74 (stale-dead-context containment): a hart executing a
            // context whose sp lives on ANOTHER task's stack, or faulting
            // at that task's stack range, is the double-run engine's
            // second half (r74i: pid 438's first hart exited cleanly
            // after the PREEMPT-UNDERFLOW heal; the second hart's stale
            // 438-context then faulted here cross-stack). If the named
            // task is already dead/dying, this hart is running a corpse:
            // panicking kills the whole machine for a one-hart loss.
            // Park this hart loudly instead — the other CPUs (and the
            // producer-hunt tripwires) keep running.
            {
                let dying = current.state().is_dead()
                    || (current.ti_flags()
                        & crate::process::task::task_flags::TaskFlags::PF_EXITING
                            .bits())
                        != 0;
                let cross_stack = !current.is_in_kernel_stack(regs.sp as usize);
                if dying && cross_stack {
                    use core::fmt::Write;
                    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
                    let _ = write!(w,
                        "trap: STALE-CONTEXT-PARK pid={} epc={:#x} sp={:#x} (dead task, foreign stack)\n",
                        current.pid(), regs.epc, regs.sp);
                    // SAFETY: wfi halts this hart until an interrupt; the
                    // context is dead and must never resume.
                    loop {
                        unsafe { core::arch::asm!("wfi") };
                    }
                }
            }
            panic!("Kernel stack overflow in task {} (fault_addr={:#x}, sp={:#x})",
                current.pid(), fault_addr_usize, regs.sp);
        }

        // Check exception table (copy_to_user/copy_from_user etc.)
        // Reachable for EXEC faults at user addresses (excluded from the
        // demand-fill above) and unresolvable data faults with no VMA.
        if let Some(fixup) = fixup_exception(regs.epc) {
            regs.epc = fixup;
            return MmFaultResult::Fixed;
        }

        // Kernel accessed invalid address (possibly a bug)
        return MmFaultResult::KernelPanic;
    }

    // User mode page fault handling

    // Non-canonical Sv39 address: bits 63..39 of a user VA must be zero.
    // The hardware faults BEFORE any page-table walk, but the kernel's own
    // walkers MASK the index to 39 bits — a page mapped at the truncated
    // position would make every fault resolve as "already mapped, perms
    // OK", retrying the same non-canonical fetch forever: a silent
    // machine-wide trap loop (bit us with an out-of-range SIGTRAMP_BASE).
    if (fault_addr.bits() >> 39) != 0 {
        return bad_area(regs, access_type, fault_addr);
    }

    // 1. Call handle_mm_fault to handle
    let result = handle_mm_fault(&addr_space, fault_addr, access_type | FaultFlags::USER);

    match result {
        crate::arch::mm::MmFaultResult::Handled => {
            // Page mapped, can re-execute instruction
            return MmFaultResult::Handled;
        }
        crate::arch::mm::MmFaultResult::CowPending => {
            // COW page, try copy-on-write
            match unsafe { handle_cow_fault(addr_space, addr_space.root_ppn(), fault_addr) } {
                crate::arch::mm::CowFaultResult::Resolved => {
                    return MmFaultResult::Handled;
                }
                crate::arch::mm::CowFaultResult::Retry => {
                    // The PTE changed between handle_mm_fault's lock-free
                    // is_cow_page() check and the locked re-walk — a
                    // sibling thread sharing this mm broke the COW first
                    // (fake-OOM layer 2: the old None→OutOfMemory mapping
                    // here SIGKILLed the race loser with the allocator
                    // nearly full). Re-execute the instruction; if the
                    // store still faults, the next entry sees the new PTE
                    // state and takes the proper path.
                    return MmFaultResult::Handled;
                }
                crate::arch::mm::CowFaultResult::OutOfMemory => {
                    // Allocating the private copy genuinely failed
                    return MmFaultResult::OutOfMemory;
                }
            }
        }
        crate::arch::mm::MmFaultResult::AlreadyMapped => {
            // Mapped but permission issue
            // Possibly writing to read-only page etc.
            send_signal(11, 2, fault_addr.bits(), regs.epc, access_type, regs);  // SIGSEGV, SEGV_ACCERR = 2
            return MmFaultResult::PermissionDenied;
        }
        crate::arch::mm::MmFaultResult::Segfault => {
            // Address not in any VMA
            // FORENSIC: identify mixed-state tasks — exe_path tells whether
            // exec completed for THIS task; satp vs pgd tells whether the
            // active root matches task->mm.
            if access_type & FaultFlags::EXEC != 0 {
                let satp_val: u64;
                unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp_val); }
                let exe = crate::sched::current()
                    .map(|t| unsafe { (*t).get_exe_path() })
                    .unwrap_or(&[]);
                let exe_str = core::str::from_utf8(exe).unwrap_or("?");
                let pgd = addr_space.pgd() as u64;
crate::pr_err!(
                    "NOVMA-EXEC: addr={:#x} pid={} exe={} pgd={:#x} satp_ppn={:#x}{} frame={:#x} ksp={:#x}",
                    fault_addr.bits(),
                    crate::sched::get_current_pid(),
                    exe_str,
                    pgd,
                    satp_val & 0xFFF_FFFF_FFFF,
                    if (satp_val & 0xFFF_FFFF_FFFF) == pgd { "" } else { " MISMATCH" },
                    regs as *mut _ as usize,
                    crate::sched::current().map(|t| unsafe { (*t).ti_kernel_sp() } as usize).unwrap_or(0)
                );
                // Dump the exec-built stack top (argv/envp/auxv area):
                // the victim dies in ld.so's pure-memory phase, so a bad
                // value on this stack is the remaining candidate source.
                {
                    use crate::arch::uaccess::copy_from_user;
                    let base = 0x3fffffe_a80u64; // exec-built argv/envp/auxv zone (fixed layout for this rootfs)
                    let mut buf = [0u8; 128];
                    let unc = unsafe { copy_from_user(buf.as_mut_ptr(), base as *const u8, 128) };
                    // refcount of the stack page: must be exclusively 1 —
                    // a higher count means a fork/teardown lost an update and
                    // someone else still maps (and zeroes) this frame.
                    {
                        use crate::arch::mm::mm_ops::PageTableWalker;
                        if let Some((ppn, bits)) = unsafe { PageTableWalker::walk(addr_space.pgd() as u64, 0x3fffffe000u64) } {
                            use crate::mm::page_desc::pfn_to_page_mut;
                            let page = pfn_to_page_mut(ppn as usize);
                            if !page.is_null() {
                                crate::pr_err!("  STACKPG ppn={:#x} pte={:#x} refcount={} mapcount={}",
                                    ppn, bits,
                                    unsafe { (*page).refcount() },
                                    unsafe { (*page).mapcount() });
                            }
                            // PTE-install replay: which roots EVER mapped this ppn.
                            {
                                use core::sync::atomic::Ordering::Relaxed;
                                use crate::arch::mm::mmu_init::{PTEI_RING, PTEI_CUR};
                                let my_root = addr_space.pgd() as u64;
                                let mut n = 0;
                                for i in 0..PTEI_RING.len() {
                                    let e = &PTEI_RING[i];
                                    if e.ppn.load(Relaxed) == ppn as u64 {
                                        let r = e.root.load(Relaxed);
                                        let v = e.va.load(Relaxed);
                                        if n < 6 {
                                            crate::pr_err!(
                                                "  PTEI: ppn={:#x} root={:#x}{} va={:#x}",
                                                ppn, r,
                                                if r == my_root { " (SELF)" } else { " ALIEN!" },
                                                v
                                            );
                                        }
                                        n += 1;
                                    }
                                }
                                crate::pr_err!("  PTEI total installs of ppn={:#x}: {}", ppn, n);
                            }
                            // Zone-ledger replay: alloc/free history of this ppn.
                            {
                                use core::sync::atomic::Ordering::Relaxed;
                                use crate::mm::zone::{ZTRACE_ALLOC, ZALLOC_CUR, ZTRACE_FREE, ZFREE_CUR};
                                let covers = |entry: u64| -> bool {
                                    let b = entry >> 4;
                                    let o = (entry & 0xF) as u64;
                                    (ppn as u64) >= b && (ppn as u64) < b + (1u64 << o)
                                };
                                let mut na = 0; let mut nf = 0;
                                let mut fa: [u64; 6] = [0; 6];
                                let mut ff: [u64; 6] = [0; 6];
                                for i in 0..ZTRACE_ALLOC.len() {
                                    let e = ZTRACE_ALLOC[i].load(Relaxed);
                                    if e != 0 && covers(e) {
                                        if na < 6 { fa[na] = i as u64; }
                                        na += 1;
                                    }
                                }
                                for i in 0..ZTRACE_FREE.len() {
                                    let e = ZTRACE_FREE[i].load(Relaxed);
                                    if e != 0 && covers(e) {
                                        if nf < 6 { ff[nf] = i as u64; }
                                        nf += 1;
                                    }
                                }
                                crate::pr_err!(
                                    "  ZLEDGER ppn={:#x}: allocs={}@{:?} frees={}@{:?} (cursors a={}/f={})",
                                    ppn, na, fa, nf, ff,
                                    ZALLOC_CUR.load(Relaxed), ZFREE_CUR.load(Relaxed)
                                );
                            }
                            // PTE-rewrite discriminator: the victim's OWN exec
                            // recorded its stack phys base — compare with the
                            // currently-walked stack ppn.
                            {
                                use core::sync::atomic::Ordering::Relaxed;
                                use crate::process::exec::STACKZERO_RING;
                                let me = crate::sched::get_current_pid();
                                for i in 0..STACKZERO_RING.len() {
                                    let zp = STACKZERO_RING[i].pid.load(Relaxed);
                                    if zp != me { continue; }
                                    let b = STACKZERO_RING[i].base.load(Relaxed);
                                    let l = STACKZERO_RING[i].len.load(Relaxed);
                                    let pa = ppn as u64 * 4096;
                                    let inside = pa >= b && pa < b + l;
                                    crate::pr_err!(
                                        "  OWNSTACK: pid={} exec base={:#x}+{:#x}; walked ppn pa={:#x} inside={}",
                                        me, b, l, pa, inside
                                    );
                                }
                            }
                            // Correlate with the exec stack-zeroing ledger.
                            {
                                use core::sync::atomic::Ordering::Relaxed;
                                use crate::process::exec::STACKZERO_RING;
                                let me = crate::sched::get_current_pid();
                                let pa = ppn as u64 * 4096;
                                for i in 0..STACKZERO_RING.len() {
                                    let b = STACKZERO_RING[i].base.load(Relaxed);
                                    if b == 0 { continue; }
                                    let l = STACKZERO_RING[i].len.load(Relaxed);
                                    let zp = STACKZERO_RING[i].pid.load(Relaxed);
                                    if pa >= b && pa < b + l && zp != me {
                                        crate::pr_err!(
                                            "  ZEROCOLLIDE: victim pid={} stack ppn={:#x} ZEROED by exec of pid={} (range {:#x}+{:#x})",
                                            me, ppn, zp, b, l
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if unc == 0 {
                        // 整页首 256B：判别"整页清零"(fill/预零) vs "定点清零"
                        let page_va = {
                            use crate::arch::mm::mm_ops::PageTableWalker;
                            match unsafe { PageTableWalker::walk(addr_space.pgd() as u64, 0x3fffffe000u64) } {
                                Some(p) => crate::arch::mm::phys_to_virt(
                                    crate::arch::mm::PhysAddr::new(p.0 as u64 * 4096)
                                ).bits() as usize,
                                None => 0,
                            }
                        };
                        if page_va != 0 {
                        for w in 0..16 {
                            let v = unsafe { core::ptr::read_volatile((page_va + w*8) as *const u64) };
                            crate::pr_err!("  PAGE[+{:#04x}] = {:#018x}", w*8, v);
                        }
                        }
                        for w in 0..16 {
                            crate::pr_err!("  STACK[{:#x}] = {:#018x}",
                                base + (w*8) as u64,
                                u64::from_le_bytes(buf[w*8..w*8+8].try_into().unwrap()));
                        }
                    }
                }
                // Read the victim's PLTGOT through its page tables.
                {
                    use crate::arch::mm::mm_ops::PageTableWalker;
                    for probe in [0x1842usize, 0x184eusize, 0x17208usize, 0x17210usize, 0x17218usize, 0x17330usize] {
                        match unsafe { PageTableWalker::walk(addr_space.pgd() as u64, probe as u64) } {
                            Some((ppn, bits)) => {
                                let va = crate::arch::mm::phys_to_virt(
                                    crate::arch::mm::PhysAddr::new(ppn as u64 * 4096)
                                ).bits() as usize + (probe & 0xFFF);
                                let v = unsafe { core::ptr::read_volatile(va as *const u64) };
                                crate::pr_err!("  GPROBE {:#x} -> ppn={:#x} pte={:#x} val={:#x}", probe, ppn, bits, v);
                            }
                            None => crate::pr_err!("  GPROBE {:#x} UNMAPPED", probe),
                        }
                    }
                }
                // GOT-page PTE history: if relocation results live on a
                // different ppn than the crash-time PTE, a post-relocation
                // page replacement reset the file-initial contents.
                {
                    use crate::arch::mm::mm_ops::PageTableWalker;
                    if let Some((gppn, _)) = unsafe { PageTableWalker::walk(addr_space.pgd() as u64, 0x17208) } {
                        use core::sync::atomic::Ordering::Relaxed;
                        use crate::arch::mm::mmu_init::{PTEI_RING, PTEI_CUR};
                        let mut n = 0;
                        for i in 0..PTEI_RING.len() {
                            let e = &PTEI_RING[i];
                            if e.ppn.load(Relaxed) == gppn as u64 {
                                if n < 6 {
                                    crate::pr_err!("  GOTPTE ppn={:#x} root={:#x} va={:#x}",
                                        gppn, e.root.load(Relaxed), e.va.load(Relaxed));
                                }
                                n += 1;
                            }
                        }
                        crate::pr_err!("  GOTPTE installs of ppn={:#x}: {} (cursor={})",
                            gppn, n, PTEI_CUR.load(Relaxed));
                    }
                }
                // Replay this pid's final syscalls from the global ring.
                {
                    use core::sync::atomic::Ordering::Relaxed;
                    use crate::syscall::dispatch::{SYSCALL_RING, SYSCALL_CURSOR};
                    let pid = crate::sched::get_current_pid();
                    let cur = SYSCALL_CURSOR.load(Relaxed);
                    let mut shown = 0;
                    for k in 0..SYSCALL_RING.len() {
                        if shown >= 28 { break; }
                        let i = (cur + SYSCALL_RING.len() - 1 - k) % SYSCALL_RING.len();
                        let e = &SYSCALL_RING[i];
                        if e.pid.load(Relaxed) == pid {
                            shown += 1;
                            crate::pr_err!(
                                "  SYSCALL[-{}] pid={} nr={} a0={:#x} a1={:#x} ret={:#x}",
                                shown, pid, e.nr.load(Relaxed),
                                e.a0.load(Relaxed), e.a1.load(Relaxed), e.ret.load(Relaxed)
                            );
                        }
                    }
                }
                // Replay this pid's full mmap history from the ring.
                {
                    use core::sync::atomic::Ordering::Relaxed;
                    use crate::syscall::memory::{MMAP_RING, MMAP_CURSOR};
                    let pid = crate::sched::get_current_pid();
                    let cur = MMAP_CURSOR.load(Relaxed);
                    crate::pr_err!("  MMAP ring cursor={}", cur);
                    // replay the last 8 entries regardless of pid: a mis-attributed
                    // mmap (recorded under a neighbor's pid) is visible this way
                    for k in 0..8usize {
                        if k >= cur.min(MMAP_RING.len()) { break; }
                        let i = (cur - 1 - k) % MMAP_RING.len();
                        let e = &MMAP_RING[i];
                        crate::pr_err!(
                            "  MMAP[-{}] pid={} addr={:#x} len={:#x} flags={:#x} fd={} off={:#x} ret={:#x}",
                            k, e.pid.load(Relaxed), e.addr.load(Relaxed), e.len.load(Relaxed),
                            e.flags.load(Relaxed), e.fd.load(Relaxed),
                            e.off.load(Relaxed), e.ret.load(Relaxed)
                        );
                    }
                }
            }
            return bad_area(regs, access_type, fault_addr);
        }
        crate::arch::mm::MmFaultResult::PermissionDenied => {
            // Insufficient permissions
            send_signal(11, 2, fault_addr.bits(), regs.epc, access_type, regs);  // SIGSEGV, SEGV_ACCERR = 2
            return MmFaultResult::PermissionDenied;
        }
        crate::arch::mm::MmFaultResult::OutOfMemory => {
            // Out of memory, send SIGKILL
            send_signal(9, 0, fault_addr.bits(), regs.epc, access_type, regs);  // SIGKILL
            return MmFaultResult::OutOfMemory;
        }
        crate::arch::mm::MmFaultResult::BusError => {
            // File-backed fault past EOF: SIGBUS with si_code BUS_ADRERR (2)
            send_signal(7, 2, fault_addr.bits(), regs.epc, access_type, regs);
            return MmFaultResult::BusError;
        }
        // Fixed and KernelPanic are handled by bad_area/no_context, not by handle_mm_fault
        crate::arch::mm::MmFaultResult::Fixed
        | crate::arch::mm::MmFaultResult::KernelPanic => {
            return bad_area(regs, access_type, fault_addr);
        }
    }
}
