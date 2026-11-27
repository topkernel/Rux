//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Forensics edges — the hunt-era diagnostics' shared raw edges
//! (unsafe-slim round 5, lane L5).
//!
//! OWNERSHIP (the coordinator's framework-consolidation doctrine): every
//! raw stale-handle read the dfx hunt family performs routes through
//! THIS file. The modules that hunt (leakorigin, scribble,
//! wildwatch, memwatch's fp-chain, the preempt/softirq
//! bracket diagnostics) stay in their own files; their raw pointer
//! discipline lives here, named and audited once.
//!
//! WHY THESE EDGES ARE RAW — the irreducibility argument (full proofs
//! in docs/architecture/safety-and-performance.md §5.1):
//!
//! The hunts exist BECAUSE a typed invariant was violated somewhere.
//! Their inputs are captured mid-corruption: a `*mut Task` frozen into
//! a diagnostic slot one tick ago may name a task that has since exited
//! and been freed (the census's stale-pid class); a `fp` chain value was
//! read out of memory the kernel does not own in any typed sense. NO
//! typed handle can prove a liveness that does not exist — demanding one
//! would make the diagnostic unable to report the very condition it
//! exists to catch. The reference kernel's equivalents (show_task's
//! stack scans, KASAN's quarantine walks, the hang-task checker's task
//! dereferences) are the same shape; there, the whole tree is TCB.
//!
//! Containment: every edge below is a READ of at most 16 bytes at an
//! address handed to us by a diagnostic producer; callers are gated by
//! their module's cmdline flag / tracepoint (release builds default
//! OFF); no edge is reachable from a hot path that is measured by the
//! perf gate with the gates disarmed.

use crate::process::task::Task;

/// Adopt a possibly-stale task handle as a shared view.
///
/// INV-FOR-1 (stale adoption): `task` came from a diagnostic slot
/// (census tick, IRQ-pair record, softirq bracket) that captured a
/// live `*mut Task`; the task MAY since have exited. The view is used
/// read-only for fields that are not torn down until the task struct's
/// final put (pid, comm, state, preempt_count) or best-effort (the
/// reader tolerates garbage). The reference's hang-task checker makes
/// the same trade (tasklist_lock does not pin a refcount for
/// sched_show_task's dereferences).
pub(crate) fn adopt_task(task: *const Task) -> Option<&'static Task> {
    if task.is_null() {
        return None;
    }
    // SAFETY: INV-FOR-1 — read-only view of a possibly-freed task; the
    // callers treat every field read as best-effort diagnostics.
    Some(unsafe { &*task })
}

/// Stale task pid read (INV-FOR-1; the commonest hunt line).
pub(crate) fn t_pid(task: *const Task) -> u32 {
    // SAFETY: INV-FOR-1 — pid is stable for the struct's lifetime and
    // never torn down before the final put.
    unsafe { (*task).pid() }
}

/// Stale task comm read (INV-FOR-1).
pub(crate) fn t_comm(task: *const Task) -> &'static [u8; 16] {
    // SAFETY: INV-FOR-1 — the comm buffer lives for the struct's
    // lifetime; a racing writer can at worst tear the copy.
    unsafe { (*task).comm() }
}

/// Stale task preempt-count read (INV-FOR-1).
pub(crate) fn t_preempt_count(task: *const Task) -> i32 {
    // SAFETY: INV-FOR-1.
    unsafe { (*task).preempt_count() }
}

/// One volatile qword at an arbitrary address the diagnostic captured
/// (stack scans, victim-page sampling, seed verification).
///
/// INV-FOR-2 (raw word): `addr` names memory the producer believes is
/// mapped (a kernel stack top captured earlier, a guarded page, the
/// linear map). The read is volatile, at most 8 bytes, and its result
/// is TREATED AS DATA, never dereferenced further by these edges.
pub(crate) fn raw_qword(addr: u64) -> u64 {
    // SAFETY: INV-FOR-2 — single volatile read of mapped-or-guarded
    // memory; the value is only printed or compared. addr MUST be
    // 8-aligned (a misaligned u64 volatile read is UB — the compiler's
    // alignment check panics in debug builds).
    debug_assert!(addr % 8 == 0, "raw_qword: misaligned u64 read at {:#x}", addr);
    unsafe { core::ptr::read_volatile(addr as usize as *const u64) }
}

/// One volatile u32 at an arbitrary captured address — the width-correct
/// twin of raw_qword for 4-mod-8 field offsets (u32 Task fields sit at
/// 4-aligned, not 8-aligned, offsets after the layout drift).
///
/// SAFETY: INV-FOR-2 — single aligned volatile read of mapped-or-guarded
/// memory; the value is only printed or compared.
pub(crate) fn raw_u32(addr: u64) -> u32 {
    debug_assert!(addr % 4 == 0, "raw_u32: misaligned u32 read at {:#x}", addr);
    unsafe { core::ptr::read_volatile(addr as usize as *const u32) }
}

/// View a captured `&'static Location` key (the panic-location tables
/// the syscost family keys its histograms by).
///
/// INV-FOR-3 (location key): `key` was stored from a real
/// `core::panic::Location` reference (whose storage is the binary's
/// rodata, machine-lifetime); the table only ever stores such values.
pub(crate) fn location(key: usize) -> &'static core::panic::Location<'static> {
    // SAFETY: INV-FOR-3 — the stored value is a rodata-backed Location
    // reference; the relabel adds no lifetime the value lacks.
    unsafe { &*(key as *const core::panic::Location<'static>) }
}

/// Adopt a lock-site reference from the hunt tables (INV-FOR-3 twin for
/// the `&'static Location` values the acquire-site recorders store).
pub(crate) fn location_ref<T>(site: *const T) -> &'static T {
    // SAFETY: the stored value is a rodata-backed reference (the
    // site tables only ever store real code-location references).
    unsafe { &*site }
}

// ---------------------------------------------------------------------------
// 2026-11-27 mainline consolidation: this file (git mv'd from
// forensics_edges.rs per the edges->unsafe.rs naming decision) is now
// the dfx subsystem's ONE audited unsafe file. The arms below absorbed
// the last inline unsafe of the hunt/diagnostic modules (backtrace,
// memwatch, scribble, wildwatch, leakorigin, taskdump, pstore,
// pathstash, ledger, mmforensics, oops, hung_task, slowtask,
// mod) — all sixteen now zero-unsafe with file-level forbid. Existing
// INV-FOR-1..3 contracts are unchanged; the arms reuse those families
// or extend them with new numbers.
// ---------------------------------------------------------------------------

use crate::process::task_ref::TaskBorrow;

// ---------------------------------------------------------------------------
// INV-FOR-1 arms: the stale-task diagnostic read family (raw Task field
// reads behind one edge each; the reads are stable-field or best-effort
// per the adopt_task contract above).
// ---------------------------------------------------------------------------

/// Adopt a possibly-stale task handle as the typed short-read borrow
/// (INV-FOR-1: same adoption contract as [`adopt_task`], delivered
/// through `TaskBorrow`'s read-only guard).
pub(crate) fn task_borrow(p: *mut Task) -> Option<TaskBorrow> {
    // SAFETY: INV-FOR-1 — possibly-stale diagnostic adoption; callers
    // treat every field read as best-effort.
    unsafe { TaskBorrow::from_raw(p) }
}

/// Stale task state read (INV-FOR-1).
pub(crate) fn t_state(task: *const Task) -> crate::process::task::TaskState {
    // SAFETY: INV-FOR-1 — state is a stable field until the final put.
    unsafe { (*task).state() }
}

/// Stale task on-cpu flag read (INV-FOR-1 best-effort arm: the flag is
/// momentarily stale by design — the caller prints it).
pub(crate) fn t_on_cpu(task: *const Task) -> bool {
    // SAFETY: INV-FOR-1 — plain bool field read.
    unsafe { (*task).on_cpu() }
}

/// Stale task kernel-stack-top read (INV-FOR-1 best-effort arm).
pub(crate) fn t_get_kernel_stack(task: *const Task) -> Option<*mut u8> {
    // SAFETY: INV-FOR-1 — the stack pointer is stable while the Task
    /// page is allocated; a freed task reads garbage the caller tolerates.
    unsafe { (*task).get_kernel_stack() }
}

/// Stale task kernel-stack-bottom read (INV-FOR-1 best-effort arm).
pub(crate) fn t_kernel_stack_bottom(task: *const Task) -> usize {
    // SAFETY: INV-FOR-1 — stable while the Task page is allocated.
    unsafe { (*task).kernel_stack_bottom() }
}

/// Stale task thread-struct read (INV-FOR-1: the ThreadStruct lives
/// inside the Task allocation; shared borrow for field reads).
pub(crate) fn t_thread(task: *const Task) -> &'static crate::arch::thread::ThreadStruct {
    // SAFETY: INV-FOR-1 — same-allocation borrow, machine-lifetime
    /// relative to the possibly-stale adoption.
    unsafe { (*task).thread() }
}

/// Stale task address-space flag read (INV-FOR-1).
pub(crate) fn t_has_address_space(task: *const Task) -> bool {
    // SAFETY: INV-FOR-1 — plain flag read.
    unsafe { (*task).has_address_space() }
}

/// Stale task sched-entity on-rq flag read (INV-FOR-1 best-effort arm).
pub(crate) fn t_sched_on_rq(task: *const Task) -> bool {
    // SAFETY: INV-FOR-1 — diagnostic scheduling-flag read.
    unsafe { (*task).sched_entity().is_on_rq() }
}

/// Stale task ti_cpu read (INV-FOR-1).
pub(crate) fn t_ti_cpu(task: *const Task) -> i32 {
    // SAFETY: INV-FOR-1 — stable field read.
    unsafe { (*task).ti_cpu() }
}

/// Racy volatile read of a sched slot's published current pointer
/// (INV-FOR-1 racy-diagnostic arm: the slot is written by its owning
/// CPU under the GRQ; this read is moment-stale by design — the same
/// discipline as the hunt modules' header reads). Returns the raw pointer VALUE
/// (0 when the slot is empty), never a reference.
pub(crate) fn sched_slot_current_volatile(cpu: usize) -> usize {
    // SAFETY: INV-FOR-1 — single volatile read of the published slot;
    /// the value is only compared or printed, never dereferenced here.
    unsafe {
        core::ptr::read_volatile(
            &crate::sched::sched::cpu_state(cpu).current as *const *mut Task,
        ) as usize
    }
}

// ---------------------------------------------------------------------------
// INV-FOR-2 arms: the raw word/slice access family (volatile reads and
// writes at diagnostic addresses; results are treated as data).
// ---------------------------------------------------------------------------

/// Volatile u64 read at `addr` (INV-FOR-2 general arm: the caller has
/// already window-checked the address — kernel linear map / registered
/// guard region / live task page; at most 8 bytes, result is data).
pub(crate) fn diag_read_qword(addr: usize) -> u64 {
    // SAFETY: INV-FOR-2 — single volatile read of caller-validated
    // mapped-or-guarded memory.
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

/// Volatile u64 write at `addr` (INV-FOR-2 write arm: the caller owns
/// the destination — a canary page before RO arming, the pstore region
/// this boot owns, or the deliberate selftest guard smash).
pub(crate) fn diag_write_qword(addr: *mut u64, v: u64) {
    // SAFETY: INV-FOR-2 — single volatile write of caller-owned memory.
    unsafe { core::ptr::write_volatile(addr, v) }
}

/// View `len` bytes at `p` as a slice (INV-FOR-2 slice arm: the caller
/// proved the range — the pstore region bounds, the cmdline blob). The
/// result is read-only diagnostic data.
pub(crate) fn diag_bytes<'a>(p: *const u8, len: usize) -> &'a [u8] {
    // SAFETY: INV-FOR-2 — the range [p, p+len) is live mapped memory
    /// per the caller's bound; the borrow is read-only.
    unsafe { &*core::ptr::slice_from_raw_parts(p, len) }
}

/// General raw-reference adoption (INV-FOR-3 general arm): `p` names
/// machine-lifetime storage whose type the caller proves (a Location
/// key in rodata, a page descriptor inside the mapped vmemmap span, the
/// interrupted frame CURRENT_PT_REGS points at). Null is the caller's
/// check — this edge mirrors the raw deref it replaces.
pub(crate) fn raw_ref<T>(p: *const T) -> &'static T {
    // SAFETY: INV-FOR-3 — machine-lifetime typed storage; the relabel
    /// adds no lifetime the value lacks.
    unsafe { &*p }
}

// ---------------------------------------------------------------------------
// INV-FOR-4: the register/CSR diagnostic read family (pure system
// register reads — no memory access beyond the caller's own stack
// buffer; valid in any context the reader already runs in).
// ---------------------------------------------------------------------------

/// The current frame-pointer register (`s0`/`rbp`/`x29`), raw read
/// (INV-FOR-4: pure register read, nomem/nostack).
#[inline]
pub(crate) fn fp_read_raw() -> u64 {
    let fp: u64;
    #[cfg(feature = "riscv64")]
    // SAFETY: INV-FOR-4 — pure register read.
    unsafe {
        core::arch::asm!("mv {}, s0", out(reg) fp, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: INV-FOR-4 — pure register read.
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack));
    }
    #[cfg(feature = "aarch64")]
    // SAFETY: INV-FOR-4 — pure register read.
    unsafe {
        core::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64", feature = "aarch64")))]
    {
        fp = 0;
    }
    fp
}

/// The calling function's return address, raw read (INV-FOR-4; on
/// x86_64 `[rbp+8]` — frame pointers are forced on).
#[inline]
pub(crate) fn caller_ra_read_raw() -> u64 {
    let ra: u64;
    #[cfg(feature = "riscv64")]
    // SAFETY: INV-FOR-4 — pure register read.
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: INV-FOR-4 — [rbp+8] is this function's return address.
    unsafe {
        core::arch::asm!("mov {}, [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "aarch64")]
    // SAFETY: INV-FOR-4 — [x29+8] is the LR slot.
    unsafe {
        core::arch::asm!("ldr {}, [x29, 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64", feature = "aarch64")))]
    {
        ra = 0;
    }
    ra
}

/// Unwind one frame (INV-FOR-2 arm on the frame words: `fp` was
/// validated by the caller — kernel range, 8-aligned; the two word
/// reads are the standard unwind operation).
#[inline]
pub(crate) fn unwind_frame_raw(fp: u64) -> Option<(u64, u64)> {
    // SAFETY: INV-FOR-2 — caller-validated frame-pointer reads.
    unsafe {
        let ret_addr = *((fp + 8) as *const u64);
        let next_fp = *(fp as *const u64);
        if ret_addr == 0 {
            return None;
        }
        Some((ret_addr, next_fp))
    }
}

/// Spill the integer registers into a dump array (INV-FOR-4: balanced
/// stack scratch / register stores only, no memory reads).
#[cfg(feature = "aarch64")]
pub(crate) fn save_regs_raw(regs: &mut [u64; 31]) {
    // SAFETY: INV-FOR-4 — register stores into the caller's buffer.
    unsafe {
        core::arch::asm!(
            "str x0, [{buf}, #0]",
            "str x1, [{buf}, #8]",
            "str x2, [{buf}, #16]",
            "str x3, [{buf}, #24]",
            "str x4, [{buf}, #32]",
            "str x5, [{buf}, #40]",
            "str x6, [{buf}, #48]",
            "str x7, [{buf}, #56]",
            "str x8, [{buf}, #64]",
            "str x9, [{buf}, #72]",
            "str x10, [{buf}, #80]",
            "str x11, [{buf}, #88]",
            "str x12, [{buf}, #96]",
            "str x13, [{buf}, #104]",
            "str x14, [{buf}, #112]",
            "str x15, [{buf}, #120]",
            "str x16, [{buf}, #128]",
            "str x17, [{buf}, #136]",
            "str x18, [{buf}, #144]",
            "str x19, [{buf}, #152]",
            "str x20, [{buf}, #160]",
            "str x21, [{buf}, #168]",
            "str x22, [{buf}, #176]",
            "str x23, [{buf}, #184]",
            "str x24, [{buf}, #192]",
            "str x25, [{buf}, #200]",
            "str x26, [{buf}, #208]",
            "str x27, [{buf}, #216]",
            "str x28, [{buf}, #224]",
            "str x29, [{buf}, #232]",
            "str x30, [{buf}, #240]",
            buf = in(reg) regs.as_mut_ptr(),
            options(nostack)
        );
    }
}

/// riscv64 twin (INV-FOR-4).
#[cfg(feature = "riscv64")]
pub(crate) fn save_regs_raw(regs: &mut [u64; 31]) {
    // SAFETY: INV-FOR-4 — register stores into the caller's buffer.
    unsafe {
        core::arch::asm!(
            "sd ra,  0*8({buf})",
            "sd sp,  1*8({buf})",
            "sd gp,  2*8({buf})",
            "sd tp,  3*8({buf})",
            "sd t0,  4*8({buf})",
            "sd t1,  5*8({buf})",
            "sd t2,  6*8({buf})",
            "sd s0,  7*8({buf})",
            "sd s1,  8*8({buf})",
            "sd a0,  9*8({buf})",
            "sd a1, 10*8({buf})",
            "sd a2, 11*8({buf})",
            "sd a3, 12*8({buf})",
            "sd a4, 13*8({buf})",
            "sd a5, 14*8({buf})",
            "sd a6, 15*8({buf})",
            "sd a7, 16*8({buf})",
            "sd s2, 17*8({buf})",
            "sd s3, 18*8({buf})",
            "sd s4, 19*8({buf})",
            "sd s5, 20*8({buf})",
            "sd s6, 21*8({buf})",
            "sd s7, 22*8({buf})",
            "sd s8, 23*8({buf})",
            "sd s9, 24*8({buf})",
            "sd s10, 25*8({buf})",
            "sd s11, 26*8({buf})",
            "sd t3, 27*8({buf})",
            "sd t4, 28*8({buf})",
            "sd t5, 29*8({buf})",
            "sd t6, 30*8({buf})",
            buf = in(reg) regs.as_mut_ptr(),
            options(nostack)
        );
    }
}

/// x86_64 twin (INV-FOR-4): spill the callee-saved GPRs (rbx is
/// reserved in LLVM, so push/spill/pop is the reliable way to read it).
#[cfg(feature = "x86_64")]
pub(crate) fn save_regs_raw(regs: &mut [u64; 31]) {
    // SAFETY: INV-FOR-4 — the pushes are balanced by the rsp
    // adjustment; only this stack scratch area is touched.
    unsafe {
        core::arch::asm!(
            "push rbx", "push rbp", "push r12", "push r13", "push r14", "push r15",
            "mov {rbx_v}, [rsp + 5*8]",
            "mov {rbp_v}, [rsp + 4*8]",
            "mov {r12_v}, [rsp + 3*8]",
            "mov {r13_v}, [rsp + 2*8]",
            "mov {r14_v}, [rsp + 1*8]",
            "mov {r15_v}, [rsp + 0*8]",
            "add rsp, 6*8",
            rbx_v = out(reg) regs[0],
            rbp_v = out(reg) regs[1],
            r12_v = out(reg) regs[2],
            r13_v = out(reg) regs[3],
            r14_v = out(reg) regs[4],
            r15_v = out(reg) regs[5],
            options(preserves_flags)
        );
    }
}

/// Read the trap CSRs (sstatus/scause/stval/sepc) as a tuple
/// (INV-FOR-4: pure CSR reads, valid in any context).
#[cfg(feature = "riscv64")]
pub(crate) fn trap_regs_read_raw() -> (u64, u64, u64, u64) {
    let (sstatus, scause, stval, sepc): (u64, u64, u64, u64);
    // SAFETY: INV-FOR-4 — pure system-register reads.
    unsafe {
        core::arch::asm!(
            "csrr {0}, sstatus",
            "csrr {1}, scause",
            "csrr {2}, stval",
            "csrr {3}, sepc",
            out(reg) sstatus, out(reg) scause, out(reg) stval, out(reg) sepc,
            options(nomem, nostack)
        );
    }
    (sstatus, scause, stval, sepc)
}

/// aarch64 twin (esr_el1/elr_el1/far_el1/spsr_el1) (INV-FOR-4).
#[cfg(feature = "aarch64")]
pub(crate) fn trap_regs_read_raw() -> (u64, u64, u64, u64) {
    let (esr, elr, far, spsr): (u64, u64, u64, u64);
    // SAFETY: INV-FOR-4 — pure system-register reads.
    unsafe {
        core::arch::asm!(
            "mrs {0}, esr_el1",
            "mrs {1}, elr_el1",
            "mrs {2}, far_el1",
            "mrs {3}, spsr_el1",
            out(reg) esr, out(reg) elr, out(reg) far, out(reg) spsr,
            options(nomem, nostack)
        );
    }
    (esr, elr, far, spsr)
}

/// The per-CPU magic word at `gs:[PC_MAGIC]` (INV-FOR-4 gs-data arm: a
/// plain data read of the active GS base — exactly what the trap stubs
/// do; the misdirection canary).
#[cfg(feature = "x86_64")]
pub(crate) fn percpu_magic_read() -> u64 {
    let v: u64;
    // SAFETY: INV-FOR-4 — %gs:PC_MAGIC data read of this CPU's PerCpu.
    unsafe {
        core::arch::asm!(
            "mov {0}, qword ptr gs:[{1}]",
            out(reg) v,
            const crate::arch::x86_64::smp::PC_MAGIC,
            options(nomem, nostack, preserves_flags)
        );
    }
    v
}

/// The cpu id the ACTIVE GS base points at (`gs:[0]`, the low u32)
/// (INV-FOR-4 gs-data arm; a wrong GS reads garbage — that is the
/// point of the scribble GS tripwire).
#[cfg(feature = "x86_64")]
pub(crate) fn gs_cpu_dword_read() -> u32 {
    let id: u32;
    // SAFETY: INV-FOR-4 — one %gs-relative u32 read; MSR reads are
    // data-safe.
    unsafe {
        core::arch::asm!(
            "mov {0:e}, dword ptr gs:[0]",
            out(reg) id,
            options(nomem, nostack, preserves_flags)
        );
    }
    id
}

/// The GS-pairing triple for the scribble tripwire: (gs:[0] cpu id,
/// IA32_KERNEL_GS_BASE, IA32_GS_BASE) (INV-FOR-4 gs-data arm; the MSR
/// reads are data-safe primitives).
#[cfg(feature = "x86_64")]
pub(crate) fn gs_pairing_read() -> (u32, u64, u64) {
    let id = gs_cpu_dword_read();
    // SAFETY: INV-FOR-4 — MSR data reads.
    unsafe {
        (
            id,
            crate::arch::x86_64::cpu::rdmsr(0xC000_0101),
            crate::arch::x86_64::cpu::rdmsr(0xC000_0102),
        )
    }
}

// ---------------------------------------------------------------------------
// INV-FOR-5: the deliberate-fault family (the provoke-oops and
// wildwatch-selftest tripwires — the fault is the DESIGN).
// ---------------------------------------------------------------------------

/// Deliberate NULL-adjacent page read at 0x10 (INV-FOR-5: the oops
/// provocation — a NULL-adjacent page with no mapping and no fixup
/// entry on this instruction; the fault is the expected outcome).
pub(crate) fn provoke_null_page_read() {
    let p = 0x10usize as *const u64;
    // SAFETY: INV-FOR-5 — deliberate fault (the provoke-oops store
    /// asked for exactly this).
    let _ = unsafe { core::ptr::read_volatile(p) };
}

/// Deliberate machine-mode CSR access from S-mode (INV-FOR-5: traps
/// IllegalInstruction — the illegal-instruction provocation, riscv64).
#[cfg(feature = "riscv64")]
pub(crate) fn provoke_illegal_insn_riscv() {
    // SAFETY: INV-FOR-5 — deliberate fault (the provoke-oops store
    /// asked for exactly this).
    unsafe {
        core::arch::asm!("csrr x0, 0x300", options(nomem, nostack));
    }
}

/// Deliberate ud2 (INV-FOR-5: ud2 is reserved for traps — the BUG
/// instruction; the illegal-instruction provocation, x86_64).
#[cfg(feature = "x86_64")]
pub(crate) fn provoke_illegal_insn_x86() {
    // SAFETY: INV-FOR-5 — deliberate fault (the provoke-oops store
    /// asked for exactly this).
    unsafe {
        core::arch::asm!("ud2", options(nomem, nostack));
    }
}

// ---------------------------------------------------------------------------
// INV-FOR-6: the raw-console and report-buffer family (the panic/oops
// print path runs with locks down; putchar_no_lock's own contract is
// the caller's context).
// ---------------------------------------------------------------------------

/// One raw console byte (INV-FOR-6: no locking, no allocation — the
/// caller is the panic/oops/diagnostic print path, locks already down).
#[inline]
pub(crate) fn console_put_raw(b: u8) {
    // SAFETY: INV-FOR-6 — the no-lock console primitive's own contract:
    /// caller runs in the locked-down diagnostic context.
    unsafe { crate::console::putchar_no_lock(b) }
}

/// The scribble report buffer, mutably (INV-FOR-6 report arm: writes
/// are bounded by the len check at the single putc call site; the
/// Sync commitment below covers the gdb-read-after-death sharing).
#[cfg(feature = "x86_64")]
pub(crate) fn scribble_report_buf_mut() -> &'static mut [u8; 4096] {
    // SAFETY: INV-FOR-6 — bounded diagnostic writes to the fixed
    /// static buffer; readers are post-mortem (gdb) or under the
    /// REPORT_LOCK.
    unsafe { &mut *crate::debug::scribble::REPORT_BUF.0.get() }
}

// SAFETY: ReportBuf is written only under the REPORT_LOCK claim (one
// reporter at a time) with a bounds-checked index, and read post-mortem
// by gdb or after the lock releases; the content is diagnostic bytes.
#[cfg(feature = "x86_64")]
unsafe impl Sync for crate::debug::scribble::ReportBuf {}

// ---------------------------------------------------------------------------
// INV-FOR-7: the memwatch frame-walk family (the walk_fp_chain bodies —
// plausibility-gated volatile frame-word reads on this CPU's own or a
// captured fp chain).
// ---------------------------------------------------------------------------

/// riscv64 frame walk (INV-FOR-7: fp passed the plausibility gate —
/// kernel-half, 8-aligned, readable; frame-word reads are volatile, the
/// walk is bounded by `out`'s length).
#[cfg(feature = "riscv64")]
pub(crate) fn walk_fp_chain_raw(mut fp: u64, out: &mut [u64]) {
    for slot in out.iter_mut() {
        *slot = 0;
        if !(fp >= 0x8000_0000_0000_0000 && fp & 7 == 0 && super::memwatch::plausible_fp(fp)) {
            return;
        }
        let ra = diag_read_qword((fp - 8) as usize);
        let next = diag_read_qword((fp - 0x10) as usize);
        *slot = ra;
        if !(next >= 0x8000_0000_0000_0000 && next & 7 == 0 && next > fp && super::memwatch::plausible_fp(next)) {
            return;
        }
        fp = next;
    }
}

/// aarch64 twin (INV-FOR-7: AAPCS64 layout — [fp] = caller's x29,
/// [fp+8] = this frame's return address).
#[cfg(feature = "aarch64")]
pub(crate) fn walk_fp_chain_raw(mut fp: u64, out: &mut [u64]) {
    for slot in out.iter_mut() {
        *slot = 0;
        if !(fp >= 0xffff_8000_0000_0000 && fp & 7 == 0 && super::memwatch::plausible_fp(fp)) {
            return;
        }
        let next = diag_read_qword(fp as usize);
        let ra = diag_read_qword((fp + 8) as usize);
        *slot = ra;
        if !(next >= 0xffff_8000_0000_0000 && next & 7 == 0 && next > fp && super::memwatch::plausible_fp(next)) {
            return;
        }
        fp = next;
    }
}

/// x86_64 twin (INV-FOR-7: SysV layout — [fp] = caller's rbp, [fp+8] =
/// this frame's return address; the ascending check mirrors riscv64).
#[cfg(feature = "x86_64")]
pub(crate) fn walk_fp_chain_raw(mut fp: u64, out: &mut [u64]) {
    for slot in out.iter_mut() {
        *slot = 0;
        if !(fp >= 0x8000_0000_0000_0000 && fp & 7 == 0 && super::memwatch::plausible_fp(fp)) {
            return;
        }
        let next = diag_read_qword(fp as usize);
        let ra = diag_read_qword((fp + 8) as usize);
        *slot = ra;
        if !(next >= 0x8000_0000_0000_0000 && next & 7 == 0 && next > fp && super::memwatch::plausible_fp(next)) {
            return;
        }
        fp = next;
    }
}

/// Store one PFN->site attribution byte in the memwatch page table
/// (INV-FOR-2 ppage arm: the table is memwatch's own heap allocation,
/// `idx < PPAGE_TABLE_ENTRIES` guaranteed by tracked_index).
#[inline]
pub(crate) fn ppage_site_store(
    table: *mut [core::sync::atomic::AtomicU8; super::memwatch::PPAGE_TABLE_ENTRIES],
    idx: usize,
    v: u8,
) {
    // SAFETY: INV-FOR-2 — live memwatch table, in-bounds index.
    unsafe { (*table)[idx].store(v, core::sync::atomic::Ordering::Relaxed) }
}

/// Load one PFN->site attribution byte (INV-FOR-2 ppage arm, shared).
#[inline]
pub(crate) fn ppage_site_load(
    table: *mut [core::sync::atomic::AtomicU8; super::memwatch::PPAGE_TABLE_ENTRIES],
    idx: usize,
) -> u8 {
    // SAFETY: INV-FOR-2 — live memwatch table, in-bounds index.
    unsafe { (*table)[idx].load(core::sync::atomic::Ordering::Relaxed) }
}

/// The kernel root's low-half population census (INV-FOR-2 pgtable-walk
/// arm: read-only walk of the live root page table and its mapped
/// non-leaf tables — the same lock-free diagnostic discipline as the
/// mmforensics walks; a racing table write costs a stale entry in a
/// print). Returns (l1 non-leaf, l0 non-leaf, user leaves, min_va,
/// max_va).
pub(crate) fn kroot_pollution_scan() -> (u64, u64, u64, u64, u64) {
    let mut l1n = 0u64;
    let mut l0n = 0u64;
    let mut min_va = u64::MAX;
    let mut max_va = 0u64;
    let mut user_leaves = 0u64;
    // SAFETY: INV-FOR-2 — read-only diagnostic walk of the kernel root
    // and its mapped sub-tables (get_page_table_virt's arch contract:
    // ppn names a mapped page-table frame).
    unsafe {
        let root_ptr = core::ptr::addr_of!(crate::arch::mm::mmu_init::ROOT_PAGE_TABLE);
        let root = &*root_ptr;
        for i in 0..2usize {
            let pte2 = root.get(i);
            if !pte2.is_valid() || pte2.is_leaf() {
                continue;
            }
            l1n += 1;
            let t1 = crate::arch::mm::mmu_init::get_page_table_virt(pte2.ppn() << 12);
            if t1.is_null() {
                continue;
            }
            for j in 0..512usize {
                let pte1 = (*t1).get(j);
                if !pte1.is_valid() {
                    continue;
                }
                let va = ((i as u64) << 30) | ((j as u64) << 21);
                if va < min_va {
                    min_va = va;
                }
                if va + (1 << 21) > max_va {
                    max_va = va + (1 << 21);
                }
                if pte1.is_leaf() {
                    if pte1.is_user() {
                        user_leaves += 1;
                    }
                    continue;
                }
                l0n += 1;
                let t0 = crate::arch::mm::mmu_init::get_page_table_virt(pte1.ppn() << 12);
                if t0.is_null() {
                    continue;
                }
                for k in 0..512usize {
                    let pte0 = (*t0).get(k);
                    if pte0.is_valid() && pte0.is_leaf() && pte0.is_user() {
                        user_leaves += 1;
                    }
                }
            }
        }
    }
    (l1n, l0n, user_leaves, min_va, max_va)
}

// ---------------------------------------------------------------------------
// INV-FOR-8: the wildwatch guard-page family (x86_64): the kernel-half
// page-table walk and the CR0.WP arm.
// ---------------------------------------------------------------------------

/// The boot-constant VMA-LMA offset (INV-FOR-8: read-only access of the
/// value written once during mmu init, before any wildwatch code runs).
#[cfg(feature = "x86_64")]
pub(crate) fn va_kernel_pa_offset_raw() -> u64 {
    // SAFETY: INV-FOR-8 — boot-constant static read.
    unsafe { crate::arch::mm::memory_layout::KERNEL_MAP.va_kernel_pa_offset as u64 }
}

/// The arch page-table-frame translation helper (INV-FOR-8: same
/// contract as the arch helper — phys names a mapped page-table frame).
#[cfg(feature = "x86_64")]
pub(crate) fn get_page_table_virt_raw(
    phys: u64,
) -> *mut crate::arch::mm::pagetable::PageTable {
    // SAFETY: INV-FOR-8 — the arch helper's own contract.
    unsafe { crate::arch::mm::mmu_init::get_page_table_virt(phys) }
}

/// Clear RW on the 4KiB page covering `va`, splitting a 2MiB leaf if
/// needed. Returns true when the leaf reads back read-only (INV-FOR-8:
/// the caller runs pre-SMP from debug::init on the BSP; the linking store
/// is one aligned 8-byte store, so the old and new translations are
/// both consistent for concurrent readers).
#[cfg(feature = "x86_64")]
pub(crate) fn protect_page_raw(va: usize) -> bool {
    use crate::arch::mm::pagetable::{PageTable, PageTableEntry};

    let root = super::wildwatch::table_virt(crate::arch::mm::mmu_init::root_static_phys());
    let pml4i = (va >> 39) & 0x1ff;
    let pdpti = (va >> 30) & 0x1ff;
    let pdi = (va >> 21) & 0x1ff;
    let pti = (va >> 12) & 0x1ff;

    // SAFETY: INV-FOR-8 — read-only walk of the shared kernel root plus
    // the single aligned linking store, pre-SMP on the BSP per the
    // caller contract.
    unsafe {
        // Full 4-level walk. (An earlier draft read PML4[i] and used its
        // PDPT page directly as the PD — one level short; every guard
        // read as zeros. The gdb cross-check below is what caught it.)
        let pml4e = (*root).get(pml4i);
        if !pml4e.is_table() {
            crate::pr_err!(
                "wildwatch: va={:#x} pml4[{:}]={:#x} not a table link",
                va,
                pml4i,
                pml4e.bits()
            );
            return false;
        }
        let pdpt: &mut PageTable = &mut *super::wildwatch::table_virt(pml4e.phys_addr());
        let pd_e = pdpt.get(pdpti);
        if !pd_e.is_valid() {
            crate::pr_err!(
                "wildwatch: va={:#x} pdpt[{:}]={:#x} invalid",
                va,
                pdpti,
                pd_e.bits()
            );
            return false;
        }
        if pd_e.is_leaf() {
            // 1GiB leaf — split not implemented (no guard region lands
            // under one today); fail loudly rather than silently skip.
            crate::pr_err!(
                "wildwatch: va={:#x} pdpt[{:}] is a 1GiB leaf (split unsupported)",
                va,
                pdpti
            );
            return false;
        }
        let pd: &mut PageTable = &mut *super::wildwatch::table_virt(pd_e.phys_addr());
        let pmd = pd.get(pdi);
        if !pmd.is_valid() {
            crate::pr_err!(
                "wildwatch: va={:#x} pml4i={} pdpti={} pdi={} pti={} pd[{:+}]={:#x} pd[{}]={:#x} pd[{}]={:#x} pd[{}]={:#x} (cr3={:#x})",
                va,
                pml4i,
                pdpti,
                pdi,
                pti,
                pdi as isize - 1,
                pd.get(pdi.wrapping_sub(1)).bits(),
                pdi,
                pmd.bits(),
                pdi + 1,
                pd.get(pdi + 1).bits(),
                pdi + 2,
                pd.get(pdi + 2).bits(),
                crate::arch::cpu::read_cr3()
            );
            return false;
        }

        if pmd.is_table() {
            // Already 4KiB entries: flip this one's RW off.
            let pt: &mut PageTable = &mut *super::wildwatch::table_virt(pmd.phys_addr());
            let mut e = pt.get(pti);
            if !e.is_valid() {
                crate::pr_err!(
                    "wildwatch: va={:#x} cr3={:#x} pml4[{}]={:#x} pdpt[{}]={:#x} pd[{}]={:#x} pt[{}]={:#x} invalid",
                    va,
                    crate::arch::cpu::read_cr3(),
                    pml4i,
                    pml4e.bits(),
                    pdpti,
                    pd_e.bits(),
                    pdi,
                    pmd.bits(),
                    pti,
                    e.bits()
                );
                return false;
            }
            e = PageTableEntry::from_bits(e.bits() & !PageTableEntry::RW);
            pt.set(pti, e);
            crate::arch::cpu::invlpg(va as u64);
            return !pt.get(pti).is_writable();
        }

        // 2MiB leaf: split into a fresh 4KiB table replicating the leaf,
        // with the target page's RW cleared.
        let phys2m = pmd.phys_addr();
        // Keep the flag bits that apply per-page; PS (7) and GLOBAL (8)
        // are dropped for children of a split.
        let mut flags = pmd.bits() & 0xFFF & !PageTableEntry::PS & !PageTableEntry::GLOBAL;
        // A supervisor leaf never carries US; keep whatever it carried.
        flags |= PageTableEntry::P;
        let Some(newpt_phys) = crate::arch::mm::mmu_init::alloc_page_table() else {
            return false;
        };
        let newpt: &mut PageTable =
            &mut *super::wildwatch::table_virt(newpt_phys);
        for i in 0..512u64 {
            let mut bits = (phys2m + i * 4096) | flags;
            if i as usize == pti {
                bits &= !PageTableEntry::RW;
            }
            newpt.set(i as usize, PageTableEntry::from_bits(bits));
        }
        // One aligned 8-byte linking store replaces the huge leaf.
        pd.set(
            pdi,
            PageTableEntry::from_bits(newpt_phys | PageTableEntry::P | PageTableEntry::RW),
        );
        crate::arch::cpu::invlpg(va as u64);
        let pt: &mut PageTable = &mut *super::wildwatch::table_virt(pd.get(pdi).phys_addr());
        !pt.get(pti).is_writable()
    }
}

/// Set CR0.WP on the CURRENT cpu (INV-FOR-8: one CR0 read-modify-write;
/// WP is a boot-time-static bit; with WP clear ring-0 stores ignore
/// page RW bits — the guard pages would be writable).
#[cfg(feature = "x86_64")]
pub(crate) fn cr0_set_write_protect() {
    // SAFETY: INV-FOR-8 — single CR0 RMW.
    unsafe {
        let cr0 = crate::arch::cpu::read_cr0();
        crate::arch::cpu::write_cr0(cr0 | 0x1_0000);
    }
}

// ---------------------------------------------------------------------------
// INV-FOR-9: the page-descriptor diagnostic read family (ledger +
// mmforensics: read-only descriptor access inside the mapped vmemmap
// span; the flags atomics are the same ones every other reader uses
// lock-free).
// ---------------------------------------------------------------------------

/// One snapshot of a page descriptor's diagnostic fields (INV-FOR-9):
/// returns (refcount, on_freelist, reserved, anon, cow, mapped, lru).
/// The caller null-checked pfn_to_page upstream.
pub(crate) fn page_diag_read(
    page: *const super::super::mm::page_desc::Page,
) -> (i32, bool, bool, bool, bool, bool, bool) {
    use super::super::mm::page_desc::PageFlag;
    // SAFETY: INV-FOR-9 — in-span descriptor; read-only lock-free flag
    // loads (a racing writer costs a torn snapshot in a print).
    unsafe {
        let p = &*page;
        (
            p.refcount(),
            p.test_flag(PageFlag::OnFreelist),
            p.is_reserved(),
            p.is_anonymous(),
            p.is_cow(),
            p.is_mapped(),
            p.test_flag(PageFlag::Lru),
        )
    }
}

/// One page-descriptor flag snapshot for the mmforensics capture report
/// (INV-FOR-9 capture arm): returns (refcount, mapcount, order, mapped,
/// type-as-u32, mapping, index) — the tuple the desc: line prints. The
/// named-flag bits are read via [`page_diag_flags`].
pub(crate) fn page_diag_words(
    page: *const super::super::mm::page_desc::Page,
) -> (i32, i32, u32, bool, u32, usize, usize) {
    // SAFETY: INV-FOR-9 — in-span descriptor, read-only snapshot.
    unsafe {
        let p = &*page;
        (
            p.refcount(),
            p.mapcount(),
            p.order() as u32,
            p.is_mapped(),
            p.page_type() as u32,
            p.mapping() as usize,
            p.index(),
        )
    }
}

/// The named flag bits of one descriptor (INV-FOR-9 capture arm): 8
/// bools in mmforensics' print order (OnFreelist, OnPcp, Lru, Anonymous,
/// Dirty, UpToDate, Reserved, Active).
pub(crate) fn page_diag_flags(
    page: *const super::super::mm::page_desc::Page,
) -> [bool; 8] {
    use super::super::mm::page_desc::PageFlag;
    // SAFETY: INV-FOR-9 — in-span descriptor, read-only snapshot.
    unsafe {
        let p = &*page;
        [
            p.test_flag(PageFlag::OnFreelist),
            p.test_flag(PageFlag::OnPcp),
            p.test_flag(PageFlag::Lru),
            p.test_flag(PageFlag::Anonymous),
            p.test_flag(PageFlag::Dirty),
            p.test_flag(PageFlag::UpToDate),
            p.test_flag(PageFlag::Reserved),
            p.test_flag(PageFlag::Active),
        ]
    }
}

// ---------------------------------------------------------------------------
// INV-FOR-10: the dfx page-table diagnostic read (the ONE audited
// PageTableWalker edge for the dfx family — moved from debug/mod.rs's
// walk_read; INV-DFXWALK-1: `root` is the diagnosed task's own mm root
// and the read is read-only through the linear map).
// ---------------------------------------------------------------------------

/// Read one `T: Copy` through a task's page tables (INV-FOR-10).
pub(crate) fn dfx_walk_read_raw<T: Copy>(root: u64, va: u64) -> Option<T> {
    use crate::arch::mm::memory_layout::{phys_to_virt, PhysAddr};
    use crate::arch::mm::mm_ops::PageTableWalker;
    // SAFETY: INV-FOR-10 — the backend walk is the arch contract
    // (caller holds the address space), and the linear map covers all
    // of RAM for a read-only diagnostic access.
    let (ppn, _) = unsafe { PageTableWalker::walk(root, va) }?;
    let base = phys_to_virt(PhysAddr((ppn << 12) as u64));
    // SAFETY: INV-FOR-10 — in-frame offset read of the diagnosed
    // address space.
    Some(unsafe {
        core::ptr::read_volatile((base.as_usize() + (va & 0xfff) as usize) as *const T)
    })
}

// ---------------------------------------------------------------------------
// INV-FOR-11: the pstore ramoops capture family (the dying boot's
// bounded copy into the reserved, mapped, owned region).
// ---------------------------------------------------------------------------

/// Write the pstore record into the ramoops region: bounded copies of
/// the identity head and the sink bytes, the printk ring tail formatted
/// straight into the remaining space, then the header with magic LAST.
/// Returns the record length (INV-FOR-11: the region is reserved RAM
/// owned by this dying boot — the other CPUs are already stopped or on
/// their way; a racing capture at worst interleaves bytes — the record
/// is a forensic artifact, not a protocol).
pub(crate) fn pstore_capture_write(
    virt: *mut u8,
    kind: u32,
    head: &[u8],
    sink_bytes: &[u8],
) -> usize {
    use core::ptr::copy_nonoverlapping;
    use super::pstore::{RecordHeader, PSTORE_RAM_SIZE, MAGIC, VERSION};
    let hdr_size = core::mem::size_of::<RecordHeader>();
    // SAFETY: INV-FOR-11 — dying-boot-owned reserved region; bounded
    // writes below never pass `limit`.
    unsafe {
        let budget = PSTORE_RAM_SIZE - hdr_size;
        let limit = virt.addr() + hdr_size + budget;
        let mut dst = virt.add(hdr_size);
        let mut write = |src: &[u8]| {
            let take = src.len().min(limit - dst.addr());
            copy_nonoverlapping(src.as_ptr(), dst, take);
            dst = dst.add(take);
        };

        // 1. The identity head.
        write(head);

        // 2. The staged report — the static sink, pre-reconstructed
        // ring-wise by the caller (atomic loads).
        write(b"--- report (as printed) ---\n");
        write(sink_bytes);

        // 3. The console tail, formatted straight into the region's
        // remaining space (try-lock inside — a stopped CPU may hold the
        // printk ring; the panic path must never wait).
        if dst.addr() < limit {
            let tail = core::slice::from_raw_parts_mut(dst, limit - dst.addr());
            let n = crate::printk::ring_tail_into(tail);
            dst = dst.add(n);
        }
        let length = dst.addr() - virt.addr() - hdr_size;

        // Header last-word-first: everything but the magic now, magic
        // LAST — a crash inside this function must not leave a record
        // the next boot would trust halfway.
        let hdr = virt as *mut RecordHeader;
        core::ptr::write_volatile(&mut (*hdr).version, VERSION);
        core::ptr::write_volatile(
            &mut (*hdr).jiffies,
            crate::drivers::timer::get_jiffies() as u32,
        );
        core::ptr::write_volatile(&mut (*hdr).kind, kind);
        core::ptr::write_volatile(&mut (*hdr)._reserved, [0; 2]);
        core::ptr::write_volatile(&mut (*hdr).length, length as u32);
        core::ptr::write_volatile(&mut (*hdr).magic, MAGIC);
        length
    }
}

// ---------------------------------------------------------------------------
// INV-FOR-1/2 extension arms (second wave: scribble/wildwatch/pathstash
// specifics).
// ---------------------------------------------------------------------------

/// Stale task hung_task_since read (INV-FOR-1: the header canary word —
/// written by the hung-task checker; a boot-relative ns timestamp or 0).
pub(crate) fn t_hung_task_since(task: *const Task) -> u64 {
    // SAFETY: INV-FOR-1 — atomic load through the possibly-stale
    /// adoption; plain data read.
    unsafe { (*task).hung_task_since.load(core::sync::atomic::Ordering::Acquire) }
}

/// Stale task running_on_cpu claim read (INV-FOR-1: the ownership word).
pub(crate) fn t_running_on_cpu(task: *const Task) -> i32 {
    // SAFETY: INV-FOR-1 — atomic load through the possibly-stale
    /// adoption; plain data read.
    unsafe { (*task).running_on_cpu.load(core::sync::atomic::Ordering::Relaxed) }
}

/// The pathstash slot for `tid`, mutably (INV-FOR-1 stash arm: this
/// task is the only writer for its tid slot; publication ordered via
/// tid/len stores).
pub(crate) fn pathstash_slot_mut(tid: u32) -> &'static mut super::pathstash::Slot {
    // SAFETY: INV-FOR-1 — single-writer-per-slot diagnostic stash.
    unsafe {
        &mut (*core::ptr::addr_of_mut!(super::pathstash::SLOTS))
            [(tid as usize) % super::pathstash::N]
    }
}

/// The pathstash slot at index `i` (i < N by the caller's modulo),
/// shared (INV-FOR-1 stash read arm: bytes utf8-checked by the caller).
pub(crate) fn pathstash_slot_ref(i: usize) -> &'static super::pathstash::Slot {
    // SAFETY: INV-FOR-1 — diagnostic read of the static slot table.
    unsafe { &(*core::ptr::addr_of!(super::pathstash::SLOTS))[i] }
}

/// The kernel text range from the linker symbols (INV-FOR-2 image-bound
/// arm: machine-lifetime linker symbols; addresses only, never
/// dereferenced).
pub(crate) fn kernel_text_range_raw() -> (u64, u64) {
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    // SAFETY: INV-FOR-2 — address-taking of linker symbols only.
    unsafe { (&_stext as *const u8 as u64, &_etext as *const u8 as u64) }
}

/// Resolve the physical address a kernel-half VA maps to (leaf walk,
/// x86_64; INV-FOR-8 arm: read-only walk of the shared kernel root via
/// table_virt's translation).
#[cfg(feature = "x86_64")]
pub(crate) fn resolve_phys_raw(va: usize) -> Option<u64> {
    // SAFETY: INV-FOR-8 — read-only walk of the shared kernel root.
    unsafe {
        let root = super::wildwatch::table_virt(crate::arch::mm::mmu_init::root_static_phys());
        let pml4i = (va >> 39) & 0x1ff;
        let pdpti = (va >> 30) & 0x1ff;
        let pdi = (va >> 21) & 0x1ff;
        let pti = (va >> 12) & 0x1ff;
        let pml4e = (*root).get(pml4i);
        if !pml4e.is_table() {
            return None;
        }
        let pdpt = &mut *super::wildwatch::table_virt(pml4e.phys_addr());
        let pd_e = pdpt.get(pdpti);
        if !pd_e.is_table() {
            return None;
        }
        let pd = &mut *super::wildwatch::table_virt(pd_e.phys_addr());
        let pmd = pd.get(pdi);
        if !pmd.is_valid() {
            return None;
        }
        if pmd.is_leaf() {
            return Some(pmd.phys_addr() | ((va as u64) & 0x1F_FFFF));
        }
        let pt = &mut *super::wildwatch::table_virt(pmd.phys_addr());
        let pte = pt.get(pti);
        if !pte.is_valid() {
            return None;
        }
        Some(pte.phys_addr() | ((va as u64) & 0xFFF))
    }
}
