//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Forensics edges — the hunt-era diagnostics' shared raw edges
//! (unsafe-slim round 5, lane L5).
//!
//! OWNERSHIP (the coordinator's framework-consolidation doctrine): every
//! raw stale-handle read the dfx hunt family performs routes through
//! THIS file. The modules that hunt (leakorigin, scribble, wildwatch,
//! memwatch's fp-chain, the preempt/softirq bracket diagnostics) stay
//! in their own files; their raw pointer discipline lives here, named
//! and audited once.
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

/// Defect-C: first-refusal-per-task marker (a 64-slot pid ring; the
/// fill_site static is overwritten by later faults, so only the FIRST
/// print per task is trustworthy). Returns true the first time `pid` is
/// seen (budget-free — bounded by task count).
pub(crate) fn mark_first_refusal(pid: u64) -> bool {
    static SEEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static RING: [core::sync::atomic::AtomicU64; 64] =
        [const { core::sync::atomic::AtomicU64::new(0) }; 64];
    let slot = (pid % 64) as usize;
    let prev = RING[slot].swap(pid, core::sync::atomic::Ordering::AcqRel);
    if prev != pid {
        SEEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return true;
    }
    false
}
