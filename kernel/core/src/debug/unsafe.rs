//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Forensics edges — the hunt-era diagnostics' shared raw edges
//! (unsafe-slim round 5, lane L5).
//!
//! OWNERSHIP (the coordinator's framework-consolidation doctrine): every
//! raw stale-handle read the debug hunt family performs routes through
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

use crate::process::types::Task;
use core::mem::size_of;
use core::panic::Location;
use core::sync::atomic::Ordering;
use core::sync::atomic::AtomicU32;
#[cfg(feature = "x86_64")]
use crate::debug::types::ReportBuf;
#[cfg(feature = "x86_64")]
use crate::arch::mm::pagetable::PageTableEntry;
use crate::debug::hotpath::leakorigin_slot_of;
use crate::debug::types::RecordHeader;
use crate::debug::hotpath::PPAGE_TABLE_ENTRIES;
use crate::debug::hotpath::plausible_fp;
use crate::debug::hotpath::{MAGIC, N, PATHSTASH_SLOTS, PSTORE_RAM_SIZE, SINK, SINK_AT, SINK_SIZE, VERSION, memwatch_put_dec, memwatch_put_hex, taskdump_raw_line};
#[cfg(feature = "x86_64")]
use crate::debug::hotpath::scribble::{BADIRET_ANNOUNCED, DIVERGE, GS_FIX_ANNOUNCED, HEADER_HIT_ANNOUNCED, HEAP_WINDOW, HITS, OFF_FS, OFF_GS, OFF_ONCPU, OFF_RET, OFF_SP, OFF_THREAD, PARK, PARK_PID1, PARK_SWEEP, REG_COUNT, REPORT_LOCK, REPORT_LEN, REPORT_POS, REPORT_BUF, REPORTING, SCRIBBLE_SLOTS, SLOTS, VERIFY_EVERY, VERIFY_TICK, audit_drain, gen_of, hash_of, pin_park_check, put_dec, put_hex, puts, ring_dump, scribble_badiret_fixups, scribble_gs_fixups, scribble_park, scribble_seed_park, slot_of};
#[cfg(feature = "x86_64")]
use crate::debug::hotpath::x86::{CANARY_MAGIC, table_virt};
#[cfg(feature = "x86_64")]
use crate::arch::mm::memory_layout::KERNEL_MAP;

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
pub fn t_pid(task: *const Task) -> u32 {
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
pub fn t_preempt_count(task: *const Task) -> i32 {
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
pub fn location(key: usize) -> &'static core::panic::Location<'static> {
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

// ==========================================================================
// pass 3 (seed-core §2c.5): debug's collected unsafe — the inline
// blocks of the subsystem's mechanism files, one contract each.
// ==========================================================================

// ------------ pathstash slot views (INV:F18) ------------

/// The writing half of this task's stash slot.
///
/// # Contract (F18)
/// INV:F18: diagnostic write into a static slot; this task is the only writer for its tid slot, publication ordered via tid/len.
pub(crate) fn stash_slot_for(tid: u32) -> &'static mut crate::debug::types::Slot {
unsafe { &mut *core::ptr::addr_of_mut!(PATHSTASH_SLOTS[(tid as usize) % N]) }
}

/// The reading half of a stash slot (static-derived 'static borrow).
///
/// # Contract (F18)
/// INV:F18: diagnostic read of the static slot table; the 'static borrow derives from the static itself and the bytes are utf8-checked by the caller.
pub(crate) fn stash_slot_at(i: usize) -> &'static crate::debug::types::Slot {
unsafe { &*core::ptr::addr_of!(PATHSTASH_SLOTS).cast::<crate::debug::types::Slot>().add(i) }
}


// ------------ locktrace: leak-origin and phantom-booking probes (INV:F1) ------------

/// Volatile racy diagnostic read of a CPU's sched slot current.
///
/// # Contract (F1)
/// INV:F1: the sched slot is written by its owning CPU under the GRQ; this read is a racy diagnostic (value treated as data).
pub(crate) fn sched_slot_current(cpu: usize) -> usize {
unsafe {
                core::ptr::read_volatile(
                    &crate::sched::hotpath::cpu_state(cpu as usize).current
                        as *const *mut crate::process::types::Task
                ) as usize
            }
}

/// Stale-task state-is-dead probe (INV-FOR-1 twin).
///
/// # Contract (F1)
/// INV-FOR-1: read-only view of a possibly-freed task; the caller treats the answer as best-effort diagnostics.
pub(crate) fn t_state_dead(task: *const crate::process::types::Task) -> bool {
unsafe { (*task).state().is_dead() }
}

/// Stale-task state-bits probe (INV-FOR-1 twin).
///
/// # Contract (F1)
/// INV-FOR-1: read-only view of a possibly-freed task; the caller treats the value as best-effort diagnostics.
pub(crate) fn t_state_bits(task: *const crate::process::types::Task) -> u32 {
unsafe { (*task).state().bits() }
}

/// INV:F1: diagnostic scalar reads of stable Task fields; `task` is current on this CPU (pinned by execution), `slot_task` was this CPU's slot content moments before.
#[allow(unsafe_code)]
pub(crate) fn report_acquire_divergent(
    task: *mut crate::process::types::Task,
    loc: &'static Location,
    cpu: u32,
    slot_task: usize,
) {
    if !crate::trace::EVENT_ACQDIV.armed() {
        let _ = (task, loc, cpu, slot_task);
        return;
    }
    static N: AtomicU32 = AtomicU32::new(60);
    let b = N.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if N.compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    use crate::console::uart_putc_raw as putchar;
    // SAFETY: diagnostic scalar reads of stable Task fields; `task` is
    // current on this CPU (pinned by execution), `slot_task` was this
    // CPU's slot content moments before (same discipline as the s14
    // IRQ-PAIR report).
    unsafe {
        let pid = (*task).pid();
        let pc = (*task).preempt_count();
        let slot_pid = if slot_task >= 0x80000000 {
            (*(slot_task as *mut crate::process::types::Task)).pid()
        } else {
            0
        };
        for &c in b"\nACQUIRE-DIVERGENT pid=" {
            putchar(c);
        }
        let mut v = pid as u64;
        let mut buf = [0u8; 12];
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        for &c in b" pc=0x" {
            putchar(c);
        }
        let mut x = pc as u32 as u64;
        for sh in (0..8).rev() {
            let d = ((x >> (sh * 4)) & 0xF) as u8;
            putchar(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        }
        for &c in b" cpu=" {
            putchar(c);
        }
        let mut v = cpu as u64;
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        for &c in b" site=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        let mut v = loc.line() as u64;
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        for &c in b" slotpid=" {
            putchar(c);
        }
        let mut v = slot_pid as u64;
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        putchar(b'\n');
    }
}
/// Outermost base release (base field 1->0): clears the leak-origin
/// record — that acquire paired. Called from `Task::sub_preempt_count_at`
/// / `sub_preempt_count2_at` when val == PREEMPT_OFFSET and the base
/// field transitions 1->0.
///
/// Pair-split face: if the recorded outermost acquire names a DIFFERENT
/// task than the releaser, the acquire's 1->0 never ran on its own task
/// — either a slot collision (two tasks hashing to one slot; the report
/// carries both pointers so collisions are identifiable) or a genuine
/// cross-task booking split. Budgeted; the flag is evidence, not proof.
/// INV:F1: diagnostic reads of the leak-origin slot and the releaser task's header fields; budgeted direct console (R34).
#[inline]
pub fn note_release(task: *mut crate::process::types::Task) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    let slot = leakorigin_slot_of(task);
    let ot = slot.otask.load(Ordering::Acquire);
    if ot == 0 {
        return;
    }
    if ot == task as usize {
        slot.otask.store(0, Ordering::Release);
        return;
    }
    // Split: budgeted direct console (may fire from IRQ-laden contexts).
    static SPLIT_N: AtomicU32 = AtomicU32::new(40);
    let b = SPLIT_N.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if SPLIT_N
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::uart_putc_raw as putchar;
    fn ph(v: usize) {
        use crate::console::uart_putc_raw as putchar;
        for sh in (0..16).rev() {
            let n = ((v >> (sh * 4)) & 0xF) as u8;
            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        }
    }
    for &c in b"\nACQUIRE-PAIR-SPLIT releaser=0x" {
        putchar(c);
    }
    ph(task as usize);
    for &c in b" recorded=0x" {
        putchar(c);
    }
    ph(ot);
    // The recorded acquire's site + cpu identify where the orphan 0->1
    // came from.
    let site = slot.osite.load(Ordering::Acquire) as *const Location;
    if !site.is_null() {
        // SAFETY: stored from a &'static Location (see note_acquire).
        let loc = crate::debug::r#unsafe::location_ref(site);
        for &c in b" osite=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        let mut v = loc.line() as u64;
        let mut buf = [0u8; 20];
        let mut n = 0;
        while v > 0 {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
    }
    for &c in b" ocpu=" {
        putchar(c);
    }
    let mut v = slot.ocpu.load(Ordering::Acquire) as u64;
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        putchar(b'0');
    }
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        putchar(buf[n]);
    }
    // stress-port r8 discriminators: the RECORDED task's live state at
    // print time. The r7 forker boot saturated this print's 40-line
    // budget with osite=sched.rs:2641 pairs (including symmetric
    // swapped releaser/recorded pairs across cpu0/cpu1) — the shape of
    // two live tasks sharing one 2048-entry hash slot under fork churn,
    // their __schedule GRQ acquire/release windows (~µs) interleaving
    // across CPUs, each release observing the other's still-armed
    // o-record. That cross-talk shows the recorded task OWNED (slot- or
    // SWITCHING_OUT-named) with its base count in flight or already 0;
    // a GENUINE cross-task booking (the lying-slot family) shows the
    // recorded task owned by nothing with base stuck at 1 (its release
    // truly never ran). rpid/rpc/rown let every line self-adjudicate.
    // SAFETY: diagnostic reads of stable Task fields on a pointer that
    // was executing on `ocpu` moments ago; same discipline as
    // note_acquire's sched-slot read.
    if ot >= 0x80000000 {
        unsafe {
            let rt = ot as *mut crate::process::types::Task;
            for &c in b" rpid=" {
                putchar(c);
            }
            let mut v = (*rt).pid() as u64;
            let mut n = 0;
            if v == 0 {
                putchar(b'0');
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                putchar(buf[n]);
            }
            for &c in b" rpc=" {
                putchar(c);
            }
            let mut v = ((*rt).preempt_count()
                & crate::interrupt::preempt::PREEMPT_MASK) as u64;
            let mut n = 0;
            if v == 0 {
                putchar(b'0');
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                putchar(buf[n]);
            }
            // rown bit 1: some cpu's sched slot names it; bit 2: some
            // cpu is switching it out right now.
            let mut rown: u64 = 0;
            for c in 0..crate::config::MAX_CPUS {
                if crate::sched::hotpath::cpu_state(c).current == rt {
                    rown |= 1;
                }
            }
            if crate::sched::hotpath::task_is_mid_switch_out(rt) {
                rown |= 2;
            }
            for &c in b" rown=" {
                putchar(c);
            }
            let mut v = rown;
            let mut n = 0;
            if v == 0 {
                putchar(b'0');
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                putchar(buf[n]);
            }
        }
    }
    putchar(b'\n');
}

// ------------ pstore: the panic record (INV:F4) ------------

/// View the panic-record header at the region base.
///
/// # Contract (F4)
/// INV:F4: the region is reserved RAM mapped by the linear map; the header reads are plain loads of memory no writer touches (the previous boot is gone).
pub(crate) fn record_header_at(virt: *mut u8) -> &'static crate::debug::types::RecordHeader {
unsafe { &*(virt as *const crate::debug::types::RecordHeader) }
}

/// Clear only the record magic — this boot owns the region.
///
/// # Contract (F4)
/// INV:F4: same reserved region; after early_scan's magic check no other writer exists until the next panic.
pub(crate) fn clear_record_magic(virt: *mut u8) {
    unsafe {
        core::ptr::write_volatile(virt as *mut u64, 0);
    }
}

/// The previous boot's record payload bytes.
///
/// # Contract (F4)
/// INV:F4: reserved, mapped, owned by this boot since early_scan cleared the magic; `length` is header-bounded.
pub(crate) fn record_payload(virt: *mut u8, length: usize) -> &'static [u8] {
unsafe {
        core::slice::from_raw_parts(virt.add(core::mem::size_of::<crate::debug::types::RecordHeader>()), length)
    }
}

/// Commit the panic record: head bytes, staged report, printk tail, then the header (magic last).
///
/// # Contract (F4)
/// INV:F4: reserved, mapped RAM owned by this dying boot; nothing else touches it (the other CPUs are stopped or on their way; a racing capture at worst interleaves bytes — the record is a forensic artifact, not a protocol).
pub(crate) fn pstore_commit(kind: u32, hw: &crate::debug::types::HeadWriter, virt: *mut u8) {
    unsafe {
        let budget = PSTORE_RAM_SIZE - size_of::<RecordHeader>();
        let limit = virt.addr() + size_of::<RecordHeader>() + budget;
        let mut dst = virt.add(size_of::<RecordHeader>());
        let mut write = |src: &[u8]| {
            let take = src.len().min(limit - dst.addr());
            core::ptr::copy_nonoverlapping(src.as_ptr(), dst, take);
            dst = dst.add(take);
        };

        // 1. The identity head.
        write(&hw.buf[..hw.len]);

        // 2. The staged report — the static sink, copied ring-wise
        // (atomic loads; every byte ever sunk sits at SINK[s %
        // SINK_SIZE], the newest SINK_SIZE bytes are kept, oldest-first).
        write(b"--- report (as printed) ---\n");
        let at = SINK_AT.load(Ordering::Relaxed);
        let first = at.saturating_sub(SINK_SIZE);
        for s in first..at {
            let b = SINK[s % SINK_SIZE].0.load(Ordering::Relaxed);
            if dst.addr() >= limit {
                break;
            }
            core::ptr::write_volatile(dst, b);
            dst = dst.add(1);
        }

        // 3. The console tail, formatted straight into the region's
        // remaining space (try-lock inside — a stopped CPU may hold the
        // printk ring; the panic path must never wait).
        if dst.addr() < limit {
            let tail = core::slice::from_raw_parts_mut(dst, limit - dst.addr());
            let n = crate::printk::ring_tail_into(tail);
            dst = dst.add(n);
        }
        let length = dst.addr() - virt.addr() - size_of::<RecordHeader>();

        // Header last-word-first: everything but the magic now, magic
        // LAST — a crash inside this function must not leave a record
        // the next boot would trust halfway.
        let hdr = virt as *mut RecordHeader;
        core::ptr::write_volatile(&mut (*hdr).version, VERSION);
        core::ptr::write_volatile(
            &mut (*hdr).jiffies,
            crate::timer::get_jiffies() as u32,
        );
        core::ptr::write_volatile(&mut (*hdr).kind, kind);
        core::ptr::write_volatile(&mut (*hdr)._reserved, [0; 2]);
        core::ptr::write_volatile(&mut (*hdr).length, length as u32);
        core::ptr::write_volatile(&mut (*hdr).magic, MAGIC);
    }
}

/// One raw console byte on the oops report path.
///
/// # Contract (F18)
/// INV:F18: raw console byte, no locking, no allocation (the oops path may run with anything held).
pub(crate) fn putchar_no_lock_raw(b: u8) {
unsafe { crate::console::putchar_no_lock(b) }
}


// ------------ wildwatch: the wild-write tripwire (INV:F1) ------------

/// Guard the guard-cluster's VAs in every alias: the kernel-half image mapping, the linear-map alias and (under 2GB) the low identity alias.
///
/// # Contract (F1)
/// The only linking store per split is one aligned 8-byte store; concurrent readers stay consistent under either mapping. Runs pre-SMP from debug::init on the BSP.
#[cfg(feature = "x86_64")]
pub(crate) fn wild_link_aliases(start: usize, len: usize, ok_pages: &mut usize, fail_pages: &mut usize, guarded_aliases: &mut usize) {
        unsafe {
            // Alias 1: link-time kernel-half VAs (the statics themselves).
            for va in (start..start + len).step_by(4096) {
                if protect_page(va) {
                    *ok_pages += 1;
                } else {
                    *fail_pages += 1;
                }
            }
            // Alias 2+3: every other VA the frames are reachable at.
            // Resolve the physical frames through the kernel-half walk,
            // then guard the linear-map alias and (if under 2GB) the low
            // identity alias.
            for va in (start..start + len).step_by(4096) {
                let Some(phys) = resolve_phys(va) else {
                    continue;
                };
                let lin = phys
                    .wrapping_add(crate::arch::mm::memory_layout::PAGE_OFFSET as u64);
                if lin != va as u64 && protect_page(lin as usize) {
                    *guarded_aliases += 1;
                }
                if phys < 0x8000_0000 {
                    if phys as usize != va && protect_page(phys as usize) {
                        *guarded_aliases += 1;
                    }
                }
            }
        }
}

/// The wildwatch-selftest deliberate store onto the RO guard page.
///
/// # Contract (F1)
/// Deliberate wild-store emulation — this store MUST page-fault (the attribution chain's trigger); if control returns, the tripwire is broken.
#[cfg(feature = "x86_64")]
pub(crate) fn canary_selftest_fire(victim: *mut u64) {
unsafe { victim.write_volatile(0xdead_beef_dead_beef) }
}

/// INV:F1: one CR0.WP read-modify-write; WP is a boot-time-static bit (ring-0 stores ignore page RW bits while it is clear).
#[cfg(feature = "x86_64")]
    /// Set CR0.WP on the CURRENT cpu (idempotent). With WP clear, ring-0
    /// stores ignore page RW bits — the guard pages would be writable.
    pub(crate) fn enable_write_protect() {
        // SAFETY: one CR0 read-modify-write; WP is a boot-time-static bit.
        unsafe {
            let cr0 = crate::arch::cpu::read_cr0();
            crate::arch::cpu::write_cr0(cr0 | 0x1_0000);
        }
    }
/// INV:F1: kernel .bss page written once (pattern fill) before RO arming.
#[cfg(feature = "x86_64")]
    pub(crate) fn fill_canary(start: usize) {
        // Only the FIRST page of the cluster is canary (hpet::GUARD
        // layout: canary page, then the live members).
        let page = start as *mut u64;
        for i in 0..512usize {
            let salt = i as u64;
            let v = if i & 1 == 0 {
                CANARY_MAGIC ^ (salt << 32)
            } else {
                !(CANARY_MAGIC ^ (salt << 32))
            };
            // SAFETY: kernel .bss page, written once before RO arming.
            unsafe { page.add(i).write_volatile(v) };
        }
    }
/// Read one canary qword for the DMA-writer verification pass.
///
/// # Contract (F1)
/// INV:F1: registered armed kernel page (the guard cluster's canary page); read-only.
#[cfg(feature = "x86_64")]
pub(crate) fn canary_qword_read(page: *const u64, i: usize) -> u64 {
unsafe { page.add(i).read_volatile() }
}

/// The boot-constant VMA-LMA offset (KERNEL_MAP).
///
/// # Contract (F1)
/// Read-only access of a value written once during mmu init, before any wildwatch code runs.
#[cfg(feature = "x86_64")]
pub(crate) fn kernel_pa_offset() -> usize {
unsafe { KERNEL_MAP.va_kernel_pa_offset }
}

/// Arch table-phys -> VA translation (the arch helper).
///
/// # Contract (F1)
/// Same contract as the arch helper: `phys` names a page-table page the kernel owns.
#[cfg(feature = "x86_64")]
pub(crate) fn page_table_virt_raw(phys: u64) -> *mut crate::arch::mm::pagetable::PageTable {
unsafe { crate::arch::mm::mmu_init::get_page_table_virt(phys) }
}

/// INV:F1: read-only walk of the shared kernel root (PML4->PDPT->PD->PTE); table pages live in the early pool or the image.
#[cfg(feature = "x86_64")]
    /// Resolve the physical address a kernel-half VA maps to (leaf walk).
    /// Full 4-level walk: PML4 -> PDPT -> PD -> PTE.
    pub(crate) fn resolve_phys(va: usize) -> Option<u64> {
        // SAFETY: read-only walk of the shared kernel root.
        unsafe {
            let root = table_virt(crate::arch::mm::mmu_init::root_static_phys());
            let pml4i = (va >> 39) & 0x1ff;
            let pdpti = (va >> 30) & 0x1ff;
            let pdi = (va >> 21) & 0x1ff;
            let pti = (va >> 12) & 0x1ff;
            let pml4e = (*root).get(pml4i);
            if !pml4e.is_table() {
                return None;
            }
            let pdpt = &mut *table_virt(pml4e.phys_addr());
            let pd_e = pdpt.get(pdpti);
            if !pd_e.is_table() {
                return None;
            }
            let pd = &mut *table_virt(pd_e.phys_addr());
            let pmd = pd.get(pdi);
            if !pmd.is_valid() {
                return None;
            }
            if pmd.is_leaf() {
                return Some(pmd.phys_addr() | ((va as u64) & 0x1F_FFFF));
            }
            let pt = &mut *table_virt(pmd.phys_addr());
            let pte = pt.get(pti);
            if !pte.is_valid() {
                return None;
            }
            Some(pte.phys_addr() | ((va as u64) & 0xFFF))
        }
    }
/// INV:F1: clear RW on the 4KiB page covering `va`, splitting a 2MiB leaf if needed. SAFETY (caller): runs pre-SMP from debug::init on the BSP; the linking store is one aligned 8-byte store, so old and new translations are both consistent for concurrent readers.
#[cfg(feature = "x86_64")]
    /// Clear RW on the 4KiB page covering `va`, splitting a 2MiB leaf if
    /// needed. Returns true when the leaf reads back read-only.
    ///
    /// SAFETY (caller): runs pre-SMP from debug::init on the BSP; the
    /// linking store is one aligned 8-byte store, so the old and new
    /// translations are both consistent for concurrent readers.
    pub(crate) unsafe fn protect_page(va: usize) -> bool {
        use crate::arch::mm::pagetable::PageTable;

        let root = table_virt(crate::arch::mm::mmu_init::root_static_phys());
        let pml4i = (va >> 39) & 0x1ff;
        let pdpti = (va >> 30) & 0x1ff;
        let pdi = (va >> 21) & 0x1ff;
        let pti = (va >> 12) & 0x1ff;

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
        let pdpt: &mut PageTable = &mut *table_virt(pml4e.phys_addr());
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
        let pd: &mut PageTable = &mut *table_virt(pd_e.phys_addr());
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
            let pt: &mut PageTable = &mut *table_virt(pmd.phys_addr());
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
            &mut *table_virt(newpt_phys);
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
        let pt: &mut PageTable = &mut *table_virt(pd.get(pdi).phys_addr());
        !pt.get(pti).is_writable()
    }

// ------------ scribble: the byte-scribble hunter (INV:F1) ------------

/// INV:F1: read-only probes of scheduler header fields; the range check keeps us inside mapped memory for both known task classes (kernel image / heap).
#[cfg(feature = "x86_64")]
    /// Task-pointer authenticity probe for the scheduler consumption points
    /// (R65). The x86-scribble2/3 captures show `__switch_to` running with
    /// prev/next = kernel-stack addresses and Task ownership words holding
    /// kernel-static pointers — this predicate separates real Tasks from
    /// such garbage before it is dereferenced by the switch path.
    ///
    /// Cheap by design: range check, then a pid bound and a kernel-stack
    /// sanity pass. Static (.bss) tasks (idle storages, the boot task) run on
    /// static stacks, so only the pid bound applies to them.
    pub fn task_plausible(task: *mut Task) -> bool {
        if task.is_null() {
            return true;
        }
        let a = task as usize;
        let in_kimg = (0xffff_ffff_8000_0000..0xffff_ffff_8100_0000).contains(&a);
        let in_heap = HEAP_WINDOW.contains(&a);
        if !in_kimg && !in_heap {
            return false;
        }
        // SAFETY: read-only probes of scheduler header fields. The range
        // check above keeps us inside mapped memory for both known classes.
        unsafe {
            let pid = (*task).pid();
            if pid == crate::sched::hotpath::TASK_POISON || pid > (1 << 22) {
                return false;
            }
            if !in_heap {
                return true; // static task (idle/boot): stacks are static too
            }
            let top = (*task).get_kernel_stack().map_or(0, |p| p as usize);
            if top == 0 || top & 0xF != 0 {
                return false;
            }
            if !HEAP_WINDOW.contains(&top)
                && !(0xffff_ffff_8000_0000..0xffff_ffff_8100_0000).contains(&top)
            {
                return false;
            }
            let bottom = (*task).kernel_stack_bottom();
            let sp = (*task).thread().sp as usize;
            if bottom >= top || sp < bottom || sp > top {
                return false;
            }
            true
        }
    }
/// INV:F1: diagnostic header reads of the double-run seed task (GRQ held by the caller).
#[cfg(feature = "x86_64")]
    /// R65 double-run seed report: `mark_picked_on_cpu` (GRQ held) is about to
    /// mark a task that is STILL another CPU's published current or still
    /// carries another CPU's running_on_cpu claim — the pick just passed the
    /// on_cpu skip, so the ownership word was reset outside the publish
    /// protocol. This is the earliest scheduler-visible instant of the
    /// shared-stack engine; dump the full ownership picture and park.
    pub fn double_seed(task: *mut Task, me: u64, owner_slot: i32, claim: i32) {
        let mut spins = 0u32;
        while REPORT_LOCK
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
            && spins < 200_000
        {
            spins += 1;
            core::hint::spin_loop();
        }
        REPORT_POS.store(0, Ordering::Relaxed);
        REPORTING.store(true, Ordering::Relaxed);
        puts(b"\nDOUBLE-SEED task=");
        put_hex(task as u64);
        // SAFETY: diagnostic header reads of the seed task.
        unsafe {
            puts(b" pid=");
            put_dec((*task).pid() as u64);
            puts(b" oncpu=");
            put_dec((*task).on_cpu() as u64);
            puts(b" claim=");
            put_dec(claim as i64 as u64);
        }
        puts(b" me=");
        put_dec(me);
        puts(b" owner_slot=");
        put_dec(owner_slot as i64 as u64);
        puts(b"\n");
        // Ownership word + neighborhood (byte-flip / static-pointer shapes).
        dump_qwords(b"OWN", (task as usize + OFF_ONCPU) & !0xF, 8);
        for c in 0..crate::config::MAX_CPUS {
            puts(b" slot");
            put_dec(c as u64);
            puts(b"=");
            put_hex(crate::sched::hotpath::cpu_state(c).current as u64);
            puts(b"/");
            put_hex(crate::arch::smp::PER_CPU[c].current_task.load(Ordering::Relaxed));
            puts(b"\n");
        }
        ring_dump();
        REPORTING.store(false, Ordering::Relaxed);
        REPORT_LEN.store(REPORT_POS.load(Ordering::Relaxed), Ordering::Release);
        REPORT_LOCK.store(false, Ordering::Release);
        if PARK.load(Ordering::Relaxed) {
            puts(b"SCRIBBLE-SEEDPARK task=");
            put_hex(task as u64);
            puts(b" oncpu_addr=");
            put_hex((task as usize + OFF_ONCPU) as u64);
            puts(b"\n");
            scribble_seed_park(task as u64);
        }
    }
/// INV:F1: the static report buffer is bounded by the len check and only written under REPORTING; the console byte path is lock-free.
#[cfg(feature = "x86_64")]
    pub(crate) fn putc(c: u8) {
        // While a report is being formatted, mirror every byte into the
        // static report buffer (gdb-readable copy immune to serial
        // interleaving).
        if REPORTING.load(Ordering::Relaxed) {
            let p = REPORT_POS.fetch_add(1, Ordering::Relaxed);
            // SAFETY: diagnostic buffer, bounded by len check.
            let buf = unsafe { &mut *REPORT_BUF.0.get() };
            if p < buf.len() {
                buf[p] = c;
            }
        }
        // SAFETY: raw console byte, no locking, no allocation.
        unsafe { crate::console::putchar_no_lock(c) };
    }
/// INV:F1: diagnostic volatile reads of kernel memory for the hex dump; printed, never dereferenced further.
#[cfg(feature = "x86_64")]
    /// Hex-dump `n` qwords at `base`, 4 per line, tagged.
    pub(crate) fn dump_qwords(tag: &[u8], base: usize, n: usize) {
        puts(tag);
        puts(b" @");
        put_hex(base as u64);
        puts(b"\n");
        for i in 0..n {
            if i % 4 == 0 {
                puts(b"  ");
                put_hex((base + i * 8) as u64);
                puts(b":");
            }
            puts(b" ");
            // SAFETY: diagnostic read of kernel memory.
            let v = unsafe { core::ptr::read_volatile((base + i * 8) as *const u64) };
            put_hex(v);
            if i % 4 == 3 {
                puts(b"\n");
            }
        }
        if n % 4 != 0 {
            puts(b"\n");
        }
    }
/// INV:F1: diagnostic volatile read of a live Task field (byte offsets captured at registration).
#[cfg(feature = "x86_64")]
    #[inline]
    pub(crate) fn rd(task: *mut Task, off: usize) -> u64 {
        // SAFETY: diagnostic read of a live Task field.
        unsafe { core::ptr::read_volatile((task as usize + off) as *const u64) }
    }
/// INV:F1: read-only probes of the just-built task (pid, kernel stack bottom) under the scribble switch.
#[cfg(feature = "x86_64")]
    /// Register a freshly built Task (called at the end of `new_task_at`).
    pub fn register(task: *mut Task) {
        if !crate::trace::EVENT_SCRIBBLE.armed() || task.is_null() {
            return;
        }
        // R65 pin park (boot param `scribblepin=0x<hex>`): park exactly when
        // the Task allocated at the given address registers, so a gdb session
        // can arm QEMU write-watchpoints on the ownership word (task+0x38)
        // BEFORE the engine strikes. The hunt victim's page is deterministic
        // across boots (same allocation sequence), which is what makes the
        // pin usable.
        pin_park_check(task);
        // Reclaimed-page guard: a Task page freed without its unregister
        // landing (exit raced the table-full path, or the free came from an
        // error path that bypassed free_task_slot) leaves a STALE armed slot
        // for this address; the slot_of probe would then hand verify() a
        // half-old shadow for the NEW occupant (the TASK_NEW + zeroed-thread
        // HIT family on reused pages, u-series). Reset any such slot first.
        if let Some(idx) = slot_of(task) {
            SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
            SLOTS[idx].task.store(0, Ordering::Release);
            SLOTS[idx].armed.store(0, Ordering::Release);
        }
        let h = hash_of(task as usize);
        for i in 0..SCRIBBLE_SLOTS {
            let idx = (h + i) & (SCRIBBLE_SLOTS - 1);
            if SLOTS[idx]
                .task
                .compare_exchange(0, task as u64, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
                SLOTS[idx].armed.store(0, Ordering::Release);
                SLOTS[idx].sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
                SLOTS[idx].sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
                SLOTS[idx].sh_sp.store(rd(task, OFF_SP), Ordering::Relaxed);
                SLOTS[idx].sh_ret.store(rd(task, OFF_RET), Ordering::Relaxed);
                SLOTS[idx].sh_rip.store(0, Ordering::Relaxed);
                SLOTS[idx].sh_rsp.store(0, Ordering::Relaxed);
                SLOTS[idx].sh_cs.store(0, Ordering::Relaxed);
                // Sweep park: once the live set is large (boot tasks ~25,
                // LTP storm children after), hand gdb the addresses so it
                // can arm quiet-region mines before the scribbler strikes.
                if PARK_SWEEP.load(Ordering::Relaxed) {
                    let n = REG_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                    if n == 30 {
                        puts(b"SCRIBBLE-SWEEPPARK");
                        puts(b" task=");
                        put_hex(task as u64);
                        puts(b" gs=");
                        put_hex((task as usize + OFF_GS) as u64);
                        puts(b" fs=");
                        put_hex((task as usize + OFF_FS) as u64);
                        puts(b" sp=");
                        put_hex((task as usize + OFF_SP) as u64);
                        puts(b" ret=");
                        put_hex((task as usize + OFF_RET) as u64);
                        puts(b" thread=");
                        put_hex((task as usize + OFF_THREAD) as u64);
                        puts(b"\n");
                        scribble_park(task as u64);
                    }
                }
                // pid1 park: the init task outlives the whole storm; its
                // Task struct and kernel stack sit at boot-fixed addresses.
                // gdb breaks on scribble_park, parses this line from the
                // serial log, and arms QEMU watchpoints on the listed
                // addresses (the x86 link strips DWARF, so the kernel hands
                // gdb the exact field addresses).
                if PARK_PID1.load(Ordering::Relaxed) {
                    // SAFETY: read-only pid probe of the just-built task.
                    let pid = unsafe { (*task).pid() };
                    if pid == 1 {
                        puts(b"SCRIBBLE-PID1");
                        puts(b" task=");
                        put_hex(task as u64);
                        if let Some(kstack_bottom) = unsafe {
                            (*task).get_kernel_stack().map(|top| {
                                top as usize
                                    - crate::config::KERNEL_STACK_SIZE
                            })
                        } {
                            puts(b" kstack=");
                            put_hex(kstack_bottom as u64);
                        }
                        puts(b" gs=");
                        put_hex((task as usize + OFF_GS) as u64);
                        puts(b" fs=");
                        put_hex((task as usize + OFF_FS) as u64);
                        puts(b" sp=");
                        put_hex((task as usize + OFF_SP) as u64);
                        puts(b" ret=");
                        put_hex((task as usize + OFF_RET) as u64);
                        puts(b" state=");
                        put_hex((task as usize + crate::process::hotpath::TASK_STATE) as u64);
                        puts(b"\n");
                        scribble_park(task as u64);
                    }
                }
                return;
            }
        }
        // Table full: silently untracked (512 live tasks is already a flood).
    }
/// INV:F1: the task's context is fully saved at the quiesce point; reads of its parked pt_regs are diagnostic.
#[cfg(feature = "x86_64")]
    /// Switch-out quiesce point: the task's context is fully saved. Any
    /// tracked field that differs from its shadow changed outside the
    /// legitimate write points — scribbled while running.
    pub fn quiesce(task: *mut Task) {
        if !crate::trace::EVENT_SCRIBBLE.armed() || task.is_null() {
            return;
        }
        let Some(idx) = slot_of(task) else { return };
        let s = &SLOTS[idx];
        if s.armed.load(Ordering::Acquire) != 0 {
            let live_gs = rd(task, OFF_GS);
            let live_fs = rd(task, OFF_FS);
            if live_gs != s.sh_gs.load(Ordering::Relaxed)
                || live_fs != s.sh_fs.load(Ordering::Relaxed)
            {
                report(task, idx, b"run-window");
            }
        }
        // Absorb this run's saved state into the shadow (sp/ret/fs/gs were
        // just rewritten by the switch path itself). Seqlock discipline:
        // odd generation while the shadows are being rewritten.
        let g = s.gen.fetch_add(1, Ordering::AcqRel);
        let _ = g;
        s.sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
        s.sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
        s.sh_sp.store(rd(task, OFF_SP), Ordering::Relaxed);
        s.sh_ret.store(rd(task, OFF_RET), Ordering::Relaxed);
        // Snapshot the parked pt_regs (last complete trap frame at the
        // kernel stack top).
        if let Some(stack_top) = unsafe { (*task).get_kernel_stack() } {
            let pr = stack_top as usize - core::mem::size_of::<crate::arch::pt_regs::PtRegs>();
            const OFF_RIP: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, rip);
            const OFF_CS: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, cs);
            const OFF_RSP: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, rsp);
            // SAFETY: diagnostic reads of the task's kernel stack top.
            unsafe {
                s.sh_rip
                    .store(core::ptr::read_volatile((pr + OFF_RIP) as *const u64), Ordering::Relaxed);
                s.sh_rsp
                    .store(core::ptr::read_volatile((pr + OFF_RSP) as *const u64), Ordering::Relaxed);
                s.sh_cs
                    .store(core::ptr::read_volatile((pr + OFF_CS) as *const u64), Ordering::Relaxed);
            }
        }
        s.gen.fetch_add(1, Ordering::AcqRel); // even again
        s.armed.store(1, Ordering::Release);
    }
/// INV:F1: quiescent-sleeper reads only (on-CPU and mid-switch tasks are skipped), stable read sets under the seqlock generation discipline; all field reads diagnostic.
#[cfg(feature = "x86_64")]
    /// Periodic verify from the timer tick: scan tasks and compare against
    /// the shadows using seqlock discipline (a read set is trusted only if
    /// the generation stayed even and unchanged across it).
    pub fn verify() {
        if !crate::trace::EVENT_SCRIBBLE.armed() {
            return;
        }
        let n = VERIFY_TICK.fetch_add(1, Ordering::Relaxed);
        if n % VERIFY_EVERY != 0 {
            return;
        }
        // R69: drain pending allocator audit records first — a double
        // handoff recorded seconds ago ages out of the ring otherwise.
        audit_drain();
        let cur = crate::sched::hotpath::current().map(|t| t as *mut Task);
        for idx in 0..SCRIBBLE_SLOTS {
            let t = SLOTS[idx].task.load(Ordering::Acquire);
            if t == 0 {
                continue;
            }
            let task = t as *mut Task;
            if cur == Some(task) {
                continue;
            }
            // A task running on ANOTHER CPU also moves its sp/fpu state
            // legitimately between our read and its next quiesce — the
            // sp-only WEAK reports on cross-CPU running tasks were pure
            // detector noise (u-series logs). Skip anything a CPU actually
            // runs or is picking right now; only fully quiescent sleepers
            // can be judged.
            if unsafe { (*task).on_cpu() }
                || (0..crate::config::MAX_CPUS)
                    .any(|c| crate::sched::hotpath::cpu_state(c).current == task)
            {
                continue;
            }
            // Recycled-page race: unregister (free_task_slot) bumps gen and
            // clears the task pointer, but verify may already hold the old
            // pointer while the page is re-born as a fork child — TASK_NEW
            // (0x80, half-built thread fields) or ZOMBIE/DEAD (exit path
            // rewriting them). Neither is a judgeable quiescent sleeper;
            // both produced the state=0x80/empty-comm HIT spam (u5, v1).
            {
                let st = unsafe { (*task).state().bits() };
                if st
                    & (crate::process::types::TaskState::TASK_NEW
                        | crate::process::types::TaskState::ZOMBIE
                        | crate::process::types::TaskState::DEAD)
                    != 0
                {
                    continue;
                }
            }
            let s = &SLOTS[idx];
            if s.armed.load(Ordering::Acquire) == 0 {
                continue;
            }
            // R67 (parked-tail tripwire): the exit-path frame validation in
            // trap.S refuses scribbled control tails at load time; this scan
            // catches them EARLY (while the task still sleeps — the report's
            // ring tail is then at most 40ms from the write). A kernel-origin
            // tail must name kernel text; a user tail must carry the exact
            // user selectors. Heap tasks with a user mm only: kernel threads
            // never build a user entry frame at their stack tops (their top
            // region holds boot/idle residue like rip=0/cs=8 — the s1 run's
            // false-positive storm).
            if HEAP_WINDOW.contains(&(task as usize))
                && s.armed.load(Ordering::Acquire) != 0
                && unsafe { (*task).has_address_space() }
                // Latch: report each victim once (the kill comes at resume;
                // re-reporting every scan floods the console and wedges the
                // serial interleave — the s1 lesson).
                && s.armed_swapped.fetch_or(8, Ordering::AcqRel) & 8 == 0
            {
                if let Some(stack_top) = unsafe { (*task).get_kernel_stack() } {
                    let pr = stack_top as usize - core::mem::size_of::<crate::arch::pt_regs::PtRegs>();
                    const OFF_RIP: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, rip);
                    const OFF_CS: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, cs);
                    const OFF_SS: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, ss);
                    // SAFETY: diagnostic reads of the task's parked frame.
                    let (rip, cs, ss) = unsafe {
                        (
                            core::ptr::read_volatile((pr + OFF_RIP) as *const u64),
                            core::ptr::read_volatile((pr + OFF_CS) as *const u64),
                            core::ptr::read_volatile((pr + OFF_SS) as *const u64),
                        )
                    };
                    let bad_tail = if cs & 3 == 0 {
                        // kernel-origin tail: rip must be inside the kernel image
                        !(0xffff_ffff_8000_0000..0xffff_ffff_8100_0000).contains(&rip)
                    } else {
                        cs != 0x33 || ss != 0x2b
                    };
                    if bad_tail {
                        puts(b"\nSCRIBBLE-TAIL task=");
                        put_hex(task as u64);
                        puts(b" rip=");
                        put_hex(rip);
                        puts(b" cs=");
                        put_hex(cs);
                        puts(b" ss=");
                        put_hex(ss);
                        puts(b"\n");
                        ring_dump();
                        // The exit-path guard will kill the task when it
                        // resumes; re-tripping later is fine and useful.
                    }
                }
            }
            // R68 (Task-header canary): the v12 residual crash showed the
            // victim's own comm string ("accept02") written into
            // hung_task_since (+0x20) while the ownership graph stayed
            // clean — user-data shapes inside a live Task page. Two header
            // invariants are false-positive-free on x86_64: hung_task_since
            // is 0 or a boot-relative ns timestamp (init 0, writers store
            // `now` — always far below 2^48 for any realistic uptime), and
            // ti_a0/a1/a2 are never written after construction on x86 (the
            // old misaligned switch-out store that dirtied ti_a0's low half
            // is gone — R67). A violation names the wild writer's victim
            // with the event ring at most one scan from the write. Global
            // one-shot: a corrupted header re-trips every scan and would
            // flood the serial interleave (the s1 lesson).
            #[cfg(feature = "x86_64")]
            if HEAP_WINDOW.contains(&(task as usize))
                && unsafe {
                    let hts = (*task).hung_task_since.load(Ordering::Acquire);
                    let a0 = core::ptr::read_volatile(
                        (task as usize + crate::process::hotpath::TI_A0) as *const u64,
                    );
                    hts >> 48 != 0 || a0 != 0
                }
                && !HEADER_HIT_ANNOUNCED.swap(true, core::sync::atomic::Ordering::AcqRel)
            {
                puts(b"\nSCRIBBLE-HDR task=");
                put_hex(task as u64);
                unsafe {
                    puts(b" pid=");
                    put_dec((*task).pid() as u64);
                    puts(b" hts=");
                    put_hex((*task).hung_task_since.load(Ordering::Acquire));
                    puts(b" a0=");
                    put_hex(core::ptr::read_volatile(
                        (task as usize + crate::process::hotpath::TI_A0) as *const u64,
                    ));
                    puts(b" comm=");
                    for &b in (*task).comm() {
                        put_hex(b as u64);
                    }
                }
                puts(b"\n");
                ring_dump();
            }
            // Stable read set (retry a couple of times when a concurrent
            // quiesce bumps the generation mid-read).
            let mut live_gs = 0;
            let mut live_fs = 0;
            let mut live_sp = 0;
            let mut live_ret = 0;
            let mut sh_gs = 0;
            let mut sh_fs = 0;
            let mut sh_sp = 0;
            let mut sh_ret = 0;
            let mut stable = false;
            for _ in 0..3 {
                let g0 = gen_of(idx);
                if g0 % 2 == 1 {
                    core::hint::spin_loop();
                    continue;
                }
                live_gs = rd(task, OFF_GS);
                live_fs = rd(task, OFF_FS);
                live_sp = rd(task, OFF_SP);
                live_ret = rd(task, OFF_RET);
                sh_gs = s.sh_gs.load(Ordering::Relaxed);
                sh_fs = s.sh_fs.load(Ordering::Relaxed);
                sh_sp = s.sh_sp.load(Ordering::Relaxed);
                sh_ret = s.sh_ret.load(Ordering::Relaxed);
                if gen_of(idx) == g0 {
                    stable = true;
                    break;
                }
            }
            if !stable {
                continue;
            }
            let mut mism = 0;
            if live_gs != sh_gs {
                mism += 1;
            }
            if live_fs != sh_fs {
                mism += 1;
            }
            if live_sp != sh_sp {
                mism += 1;
            }
            if live_ret != sh_ret {
                mism += 1;
            }
            if mism == 0 {
                continue;
            }
            // pt_regs control words for tasks asleep off-CPU: a garbage cs
            // (not __USER_CS 0x33 / __KERNEL_CS 0x08) is a frame scribble.
            let mut frame_bad = false;
            // SAFETY: atomic state probe of a live task.
            let st = unsafe { (*task).state().bits() };
            let on_cpu = unsafe { (*task).on_cpu() };
            if !on_cpu
                && st != crate::process::types::TaskState::RUNNING
                && (st == crate::process::types::TaskState::INTERRUPTIBLE
                    || st == crate::process::types::TaskState::UNINTERRUPTIBLE)
            {
                if let Some(stack_top) = unsafe { (*task).get_kernel_stack() } {
                    let pr = stack_top as usize - core::mem::size_of::<crate::arch::pt_regs::PtRegs>();
                    const OFF_CS: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, cs);
                    // SAFETY: diagnostic read of the task's parked frame.
                    let cs = unsafe { core::ptr::read_volatile((pr + OFF_CS) as *const u64) };
                    if cs != 0x33 && cs != 0x08 {
                        frame_bad = true;
                    }
                }
            }
            // Single-field sp drifts are the weakest signal (deep stack
            // depth changes between runs read as mismatch only when the
            // quiesce hook raced); report but do not park on them.
            if mism == 1 && live_sp != sh_sp && !frame_bad {
                puts(b"SCRIBBLE-WEAK sp-only task=");
                put_hex(task as u64);
                puts(b" sh=");
                put_hex(sh_sp);
                puts(b" live=");
                put_hex(live_sp);
                puts(b"\n");
                s.sh_sp.store(live_sp, Ordering::Relaxed);
                s.gen.fetch_add(2, Ordering::AcqRel);
                continue;
            }
            report(task, idx, b"quiescent");
            // Resync so repeated scribbles on the same field re-trip.
            let g = s.gen.fetch_add(1, Ordering::AcqRel);
            let _ = g;
            s.sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
            s.sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
            s.sh_sp.store(rd(task, OFF_SP), Ordering::Relaxed);
            s.sh_ret.store(rd(task, OFF_RET), Ordering::Relaxed);
            s.gen.fetch_add(1, Ordering::AcqRel);
        }
    }
/// (gs:[0] cpu id, KERNEL_GS_BASE, KERNEL_GS_BASE.shadow) in one probe.
///
/// # Contract (F1)
/// One %gs-relative u32 read plus two MSR reads; MSR reads are data-safe at any privilege.
#[cfg(feature = "x86_64")]
pub(crate) fn gs_triple_read() -> (u32, u64, u64) {
unsafe {
            let id: u32;
            core::arch::asm!(
                "mov {0:e}, dword ptr gs:[0]",
                out(reg) id,
                options(nomem, nostack, preserves_flags)
            );
            let b1 = crate::arch::x86_64::cpu::rdmsr(0xC000_0101);
            let b2 = crate::arch::x86_64::cpu::rdmsr(0xC000_0102);
            (id, b1, b2)
        }
}

/// INV:F1: read-only probes of live task scheduling fields; the GRQ trylock is held, so pick/dequeue/publish windows are closed.
#[cfg(feature = "x86_64")]
    /// Scheduler-consistency scan: run under the GRQ lock (trylock — skip
    /// the round when contended) so scheduler transients are excluded, and
    /// require the anomaly to persist across two consecutive scans before
    /// reporting. A task that is BOTH linked on a class queue AND on-CPU is
    /// a double-run seed — the second CPU that picks it executes on the
    /// same kernel stack (register-frame scribbles, interleaved trap
    /// frames, sp/ret drift).
    pub fn verify_consistency() {
        if !crate::trace::EVENT_SCRIBBLE.armed() {
            return;
        }
        // Try the GRQ lock; skip this round if the scheduler holds it.
        let Some(_grq_guard) = crate::sched::hotpath::try_grq_lock() else {
            return;
        };
        let cur = crate::sched::hotpath::current().map(|t| t as *mut Task);
        for idx in 0..SCRIBBLE_SLOTS {
            let t = SLOTS[idx].task.load(Ordering::Acquire);
            if t == 0 {
                continue;
            }
            let task = t as *mut Task;
            // SAFETY: read-only probes of live task scheduling fields; the
            // GRQ lock is held, so pick/dequeue/publish windows are closed.
            let (on_rq, on_cpu, state) = unsafe {
                ((*task).sched_entity().is_on_rq(), (*task).on_cpu(), (*task).state().bits())
            };
            let running = state == crate::process::types::TaskState::RUNNING;
            const CLS_A: &[u8] = b"onrq+oncpu";
            const CLS_B: &[u8] = b"onrq+sleeping";
            const CLS_C: &[u8] = b"phantom-run";
            // Under the GRQ lock the legitimate states are:
            //   (linked ∧ RUNNING ∧ ¬on_cpu ∧ ¬current)   — queued
            //   (¬linked ∧ RUNNING ∧ on_cpu)              — running/picked
            //   (¬linked ∧ sleeping ∧ ¬on_cpu)            — blocked
            //   current (this CPU's or another's pick is excluded by the lock)
            // R65: (linked ∧ on_cpu) is ALSO legitimate while the task's CPU
            // is inside its switch-out (a wake links the task — the pick-side
            // skip sequences the resume — while the on_cpu mark survives
            // until __switch_to's asm-side release point). Exclude the
            // SWITCHING_OUT-marked tasks from the anomaly classes; a mark
            // that outlives the switch is still caught two scans later.
            // Anything else that persists across two scans is an invariant
            // break worth naming. (onrq+sleeping is also the pre-schedule
            // window ONLY when the sleeper itself is current on a CPU —
            // excluded via the current-slot scan.)
            let curr_any = cur == Some(task)
                || crate::sched::hotpath::task_is_mid_switch_out(task)
                || (0..crate::config::MAX_CPUS)
                    .any(|c| crate::sched::hotpath::cpu_state(c).current == task);
            let anomaly: &[u8] = if curr_any {
                continue;
            } else if on_rq && on_cpu {
                CLS_A
            } else if on_rq && !running {
                CLS_B
            } else if running && !on_rq && !on_cpu {
                CLS_C
            } else {
                // Consistent again: re-arm the rate-limit for this slot.
                SLOTS[idx].armed_swapped.store(0, Ordering::Relaxed);
                continue;
            };
            let key = if core::ptr::eq(anomaly.as_ptr(), CLS_A.as_ptr()) {
                1u32
            } else if core::ptr::eq(anomaly.as_ptr(), CLS_B.as_ptr()) {
                2
            } else {
                4
            };
            // Persistence filter: first sighting only latches the class;
            // report on the SECOND consecutive scan.
            let seen = SLOTS[idx].armed_swapped.fetch_or(key, Ordering::AcqRel);
            if seen & key == 0 {
                // Latch: freeze the event-ring tail NOW (the report-side
                // dump comes 40ms later — causal entries scroll out).
                puts(b"SCRIBBLE-LATCH ");
                put_hex(task as u64);
                puts(b" cls=");
                puts(anomaly);
                puts(b"\n");
                ring_dump();
                continue;
            }
            puts(b"SCRIBBLE-CONS ");
            put_hex(task as u64);
            // SAFETY: diagnostic read of the task header.
            unsafe {
                puts(b" pid=");
                put_dec((*task).pid() as u64);
            }
            puts(b" cls=");
            puts(anomaly);
            puts(b" onrq=");
            put_dec(on_rq as u64);
            puts(b" oncpu=");
            put_dec(on_cpu as u64);
            puts(b" st=0x");
            put_hex(state as u64);
            puts(b" cpu=");
            put_dec(crate::arch::cpu_id() as u64);
            // Full ownership picture: which CPU's slots claim this task,
            // the task's own ti_cpu + running_on_cpu claim, policy.
            // SAFETY: diagnostic reads of scheduling fields.
            unsafe {
                puts(b" ticpu=");
                put_dec((*task).ti_cpu() as i64 as u64);
                puts(b" claim=");
                put_dec((*task).running_on_cpu.load(Ordering::Relaxed) as i64 as u64);
                for c in 0..crate::config::MAX_CPUS {
                    puts(b" s");
                    put_dec(c as u64);
                    puts(b"=");
                    put_hex(crate::sched::hotpath::cpu_state(c).current as u64);
                    puts(b"/");
                    put_hex(
                        crate::arch::smp::PER_CPU[c]
                            .current_task
                            .load(Ordering::Relaxed),
                    );
                }
            }
            puts(b"\n");
            if key == 1 || key == 2 {
                ring_dump();
                let n = HITS.fetch_add(1, Ordering::AcqRel) + 1;
                if PARK.load(Ordering::Relaxed) && n == 1 {
                    puts(b"SCRIBBLE-CONSPARK task=");
                    put_hex(task as u64);
                    puts(b" gs=");
                    put_hex((task as usize + OFF_GS) as u64);
                    puts(b" fs=");
                    put_hex((task as usize + OFF_FS) as u64);
                    puts(b" sp=");
                    put_hex((task as usize + OFF_SP) as u64);
                    puts(b" ret=");
                    put_hex((task as usize + OFF_RET) as u64);
                    puts(b" thread=");
                    put_hex((task as usize + OFF_THREAD) as u64);
                    puts(b"\n");
                    scribble_park((task as usize + OFF_THREAD) as u64);
                }
            }
        }
    }
/// INV:F1: diagnostic header reads; the pages are live task structs (slot-prev vs hardware-current divergence report).
#[cfg(feature = "x86_64")]
    /// R55 tripwire: __schedule observed slot-prev != hardware-current (tp).
    /// Names the divergence the moment the scheduler first sees it — before
    /// any heal, with every slot's claim printed.
    pub fn slot_divergence(cpu: usize, tp: *mut Task, slot_prev: *mut Task) {
        let n = DIVERGE.fetch_add(1, Ordering::AcqRel) + 1;
        if n > 64 {
            return; // rate limit
        }
        puts(b"SCRIBBLE-DIVERGE #");
        put_dec(n as u64);
        puts(b" cpu=");
        put_dec(cpu as u64);
        puts(b" tp=");
        put_hex(tp as u64);
        // SAFETY: diagnostic header reads; the pages are live task structs.
        unsafe {
            if !tp.is_null() {
                puts(b"(pid=");
                put_dec((*tp).pid() as u64);
                puts(b")");
            }
            puts(b" slot=");
            put_hex(slot_prev as u64);
            if !slot_prev.is_null() {
                puts(b"(pid=");
                put_dec((*slot_prev).pid() as u64);
                puts(b")");
            }
            puts(b" slots:");
            for c in 0..crate::config::MAX_CPUS {
                put_dec(c as u64);
                puts(b"=");
                put_hex(crate::sched::hotpath::cpu_state(c).current as u64);
            }
            if !tp.is_null() {
                puts(b" tponcpu=");
                put_dec((*tp).on_cpu() as u64);
                puts(b" tpclaim=");
                put_dec((*tp).running_on_cpu.load(Ordering::Relaxed) as i64 as u64);
                puts(b" tponrq=");
                put_dec((*tp).sched_entity().is_on_rq() as u64);
                puts(b" tpst=0x");
                put_hex((*tp).state().bits() as u64);
            }
        }
        puts(b"\n");
    }
/// INV:F1: ReportBuf is only ever written under REPORTING/REPORT_LOCK (the report writers' bounded-spin protocol) and read by gdb offline; Sync is the raw-shared-static proof for that discipline.
#[cfg(feature = "x86_64")]
    unsafe impl Sync for ReportBuf {}
/// INV:F1: diagnostic reads of the faulting frame, the task the hardware runs and the slot table — one CPU at a time via REPORT_LOCK.
#[cfg(feature = "x86_64")]
    /// R63 crash capture: on a kernel-mode fatal trap (#GP/#UD/#PF-to-panic),
    /// serialize a full picture BEFORE the (4-CPU-interleaving) panic printer
    /// runs: the faulting pt_regs verbatim, the per-CPU ownership picture
    /// (sched current slots, arch hardware current_task, switch-out marks),
    /// the current task identity, and the pick/publish ring tail. One CPU at
    /// a time via REPORT_LOCK, so at least one clean copy reaches the serial
    /// log even when several CPUs die together.
    pub fn crash_report(vector: &[u8], regs: &crate::arch::pt_regs::PtRegs) {
        if !crate::trace::EVENT_SCRIBBLE.armed() {
            return;
        }
        // Bounded spin (a concurrent panic path may hold the lock; never
        // wedge the crash report itself).
        let mut spins = 0u32;
        while REPORT_LOCK
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
            && spins < 200_000
        {
            spins += 1;
            core::hint::spin_loop();
        }
        REPORT_POS.store(0, Ordering::Relaxed);
        REPORTING.store(true, Ordering::Relaxed);

        puts(b"\n=== SCRIBBLE-CRASH ");
        puts(vector);
        puts(b" rip=");
        put_hex(regs.rip);
        puts(b" cs=");
        put_hex(regs.cs);
        puts(b" err=");
        put_hex(regs.orig_rax);
        puts(b" cr2-ish rsp=");
        put_hex(regs.rsp);
        puts(b"\n");
        // Full pt_regs (fault frame verbatim).
        dump_qwords(b"PTREGS", regs as *const _ as usize, 21);

        // Stack window around the faulting rsp — the recursion signature:
        // a repeated return address across this band names the function
        // that burned the stack (R64: the overflow-into-neighbor engine).
        {
            let rsp = regs.rsp as usize;
            let lo = rsp.saturating_sub(0x40) & !0xF;
            let hi = rsp + 0x380;
            dump_qwords(b"STACKWIN", lo, (hi - lo) / 8);
        }

        // Ownership picture.
        let tp = crate::arch::cpu::get_thread_id() as *mut Task;
        puts(b"OWNER cpu=");
        put_dec(crate::arch::cpu_id() as u64);
        puts(b" tp=");
        put_hex(tp as u64);
        if !tp.is_null() {
            // SAFETY: diagnostic header reads of the task the hardware runs.
            unsafe {
                puts(b" pid=");
                put_dec((*tp).pid() as u64);
                puts(b" st=0x");
                put_hex((*tp).state().bits() as u64);
                puts(b" oncpu=");
                put_dec((*tp).on_cpu() as u64);
                puts(b" claim=");
                put_dec((*tp).running_on_cpu.load(Ordering::Relaxed) as i64 as u64);
                puts(b" onrq=");
                put_dec((*tp).sched_entity().is_on_rq() as u64);
                if let Some(kstack) = (*tp).get_kernel_stack() {
                    puts(b" kstack=");
                    put_hex(kstack as u64);
                }
            }
        }
        puts(b"\nSLOTS:");
        for c in 0..crate::config::MAX_CPUS {
            puts(b" c");
            put_dec(c as u64);
            puts(b"=");
            put_hex(crate::sched::hotpath::cpu_state(c).current as u64);
            puts(b"/");
            put_hex(
                crate::arch::smp::PER_CPU[c]
                    .current_task
                    .load(Ordering::Relaxed),
            );
        }
        puts(b"\n");

        // Victim task header + thread struct (thread.sp / kernel_stack /
        // callee.ret_addr live here — a torn value names which field the
        // scribbler rewrote to route this CPU onto a foreign stack).
        if !tp.is_null() {
            dump_qwords(b"TPHDR", tp as usize, 16);
            dump_qwords(b"THRD", (tp as usize) + OFF_THREAD, 76);
        }

        // Pick/publish ring tail — the scheduling history into the crash.
        ring_dump();

        REPORTING.store(false, Ordering::Relaxed);
        REPORT_LEN.store(REPORT_POS.load(Ordering::Relaxed), Ordering::Release);
        REPORT_LOCK.store(false, Ordering::Release);
    }
/// INV:F1: diagnostic reads of the scribble victim's shadow and live fields under REPORT_LOCK.
#[cfg(feature = "x86_64")]
    pub(crate) fn report(task: *mut Task, idx: usize, phase: &[u8]) {
        // One writer at a time (bounded spin — never deadlock a panic path).
        let mut spins = 0u32;
        while REPORT_LOCK
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
            && spins < 200_000
        {
            spins += 1;
            core::hint::spin_loop();
        }
        REPORT_POS.store(0, Ordering::Relaxed);
        REPORTING.store(true, Ordering::Relaxed);

        let n = HITS.fetch_add(1, Ordering::AcqRel) + 1;
        let s = &SLOTS[idx];
        puts(b"\nSCRIBBLE-HIT #");
        put_dec(n as u64);
        puts(b" phase=");
        puts(phase);
        puts(b" cpu=");
        put_dec(crate::arch::cpu_id() as u64);
        puts(b" task=");
        put_hex(task as u64);
        // SAFETY: diagnostic reads of stable Task header fields.
        unsafe {
            puts(b" pid=");
            put_dec((*task).pid() as u64);
            puts(b" comm=");
            let comm = (*task).comm();
            for &b in comm.iter() {
                if b == 0 {
                    break;
                }
                putc(b);
            }
            puts(b" state=0x");
            put_hex((*task).state().bits() as u64);
        }
        puts(b"\n");

        puts("  gs  sh=".as_bytes());
        put_hex(s.sh_gs.load(Ordering::Relaxed));
        puts(b" live=");
        put_hex(rd(task, OFF_GS));
        puts(b"\n  fs  sh=");
        put_hex(s.sh_fs.load(Ordering::Relaxed));
        puts(b" live=");
        put_hex(rd(task, OFF_FS));
        puts(b"\n  sp  sh=");
        put_hex(s.sh_sp.load(Ordering::Relaxed));
        puts(b" live=");
        put_hex(rd(task, OFF_SP));
        puts(b"\n  ret sh=");
        put_hex(s.sh_ret.load(Ordering::Relaxed));
        puts(b" live=");
        put_hex(rd(task, OFF_RET));
        puts(b"\n");

        // Full ThreadStruct dump — the damage SHAPE (contiguous 512-byte
        // block == fxsave-looking; register-frame runs == trap-push on a
        // wild rsp) names the writer family.
        dump_qwords(b"THREAD", (task as usize) + OFF_THREAD, 76);
        // Task header (thread_info fields + state/pid + early list links).
        dump_qwords(b"TASKHDR", task as usize, 16);
        // Parked pt_regs at the stack top.
        if let Some(stack_top) = unsafe { (*task).get_kernel_stack() } {
            let pr = stack_top as usize - core::mem::size_of::<crate::arch::pt_regs::PtRegs>();
            dump_qwords(b"PTREGS", pr, 21);
        }

        REPORTING.store(false, Ordering::Relaxed);
        REPORT_LEN.store(REPORT_POS.load(Ordering::Relaxed), Ordering::Release);
        REPORT_LOCK.store(false, Ordering::Release);

        if PARK.load(Ordering::Relaxed) {
            // Park the FIRST hit: gdb arms watchpoints on the neighborhood.
            if n == 1 {
                puts(b"SCRIBBLE-HITPARK");
                puts(b" task=");
                put_hex(task as u64);
                puts(b" gs=");
                put_hex((task as usize + OFF_GS) as u64);
                puts(b" fs=");
                put_hex((task as usize + OFF_FS) as u64);
                puts(b" sp=");
                put_hex((task as usize + OFF_SP) as u64);
                puts(b" ret=");
                put_hex((task as usize + OFF_RET) as u64);
                puts(b" thread=");
                put_hex((task as usize + OFF_THREAD) as u64);
                puts(b"\n");
                scribble_park((task as usize + OFF_GS) as u64);
            }
        }
    }

// ------------ mm diagnostic edges (INV:F3 / INV:F10 / INV:F18) ------------

/// The one audited PageTableWalker edge for the debug family.
///
/// # Contract (F10)
/// INV-DFXWALK-1: `root` is the diagnosed task's own mm root; the backend walk is the arch contract (caller holds the address space).
#[allow(dead_code)]
pub(crate) fn ptw_walk(root: u64, va: u64) -> Option<(u64, u64)> {
unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) }
}

/// Diagnostic volatile read of one `T: Copy` through the linear map.
///
/// # Contract (F10)
/// INV-DFXWALK-1: `addr` is the linear-map byte address the walk above resolved; the read covers RAM the linear map maps.
#[allow(dead_code)]
pub(crate) fn read_volatile_t<T: Copy>(addr: usize) -> T {
unsafe {
        core::ptr::read_volatile(addr as *const T)
    }
}

/// Shared view of a page descriptor the caller is operating on.
///
/// # Contract (F18)
/// INV:F18: read-only descriptor access on the diagnostic path; the caller holds the page's refcount.
pub(crate) fn page_view(page: *const crate::mm::types::Page) -> &'static crate::mm::types::Page {
unsafe { &*page }
}

/// Print one frame's page-descriptor state (buddy + page-cache view) on the mmforensics capture path.
///
/// # Contract (F18)
/// INV:F18: read-only descriptor access inside the mapped vmemmap span; rate-limited by the caller.
pub(crate) fn page_desc_report(page: *const crate::mm::types::Page) {
        unsafe {
            use crate::mm::types::PageFlag;
            let p = &*page;
            let bit = |f: PageFlag, n: &'static str| -> &'static str {
                if p.test_flag(f) {
                    n
                } else {
                    ""
                }
            };
            crate::pr_err!(
                "  desc: refcount={} mapcount={} order={} mapped={} type={} mapping={:#x} index={} flags:[{}{}{}{}{}{}{}{}]",
                p.refcount(),
                p.mapcount(),
                p.order(),
                p.is_mapped(),
                p.page_type() as u32,
                p.mapping() as usize,
                p.index(),
                bit(PageFlag::OnFreelist, "OnFreelist "),
                bit(PageFlag::OnPcp, "OnPcp "),
                bit(PageFlag::Lru, "Lru "),
                bit(PageFlag::Anonymous, "Anon "),
                bit(PageFlag::Dirty, "Dirty "),
                bit(PageFlag::UpToDate, "UpToDate "),
                bit(PageFlag::Reserved, "Reserved "),
                bit(PageFlag::Active, "Active "),
            );
        }
}

/// Stamp a phys-page site id into the PFN->site table.
///
/// # Contract (F3)
/// INV:F3: `table` is the allocated ppage site table (non-null checked by the caller), `i < PPAGE_TABLE_ENTRIES` (loop-bounded), sid <= PSITE_SLOTS.
pub(crate) fn ppage_stamp_slot(table: *mut [core::sync::atomic::AtomicU8; PPAGE_TABLE_ENTRIES], i: usize, sid: usize) {
        unsafe {
            (*table)[i].store(sid as u8, Ordering::Relaxed);
        }
}

/// Load a phys-page site id from the PFN->site table.
///
/// # Contract (F3)
/// INV:F3: `table` non-null checked by the caller; `i` is the tracked_index result (< PPAGE_TABLE_ENTRIES).
pub(crate) fn ppage_slot_load(table: *mut [core::sync::atomic::AtomicU8; PPAGE_TABLE_ENTRIES], first: usize) -> u8 {
unsafe { (*table)[first].load(Ordering::Relaxed) }
}

/// Print the kernel root's low-half (vpn2[0..1]) non-leaf population:
/// `l1` non-leaf PGD entries and `l0` non-leaf L1 entries beneath them.
/// Read-only walk; diagnostic noise under concurrency is acceptable.
/// INV:F3: read-only walk of the shared kernel root (ROOT_PAGE_TABLE) — diagnostic noise under concurrency is acceptable by design.
pub(crate) fn kroot_pollution() {
    taskdump_raw_line(b"MEMKROOT l1=");
    let mut l1n = 0u64;
    let mut l0n = 0u64;
    let mut min_va = u64::MAX;
    let mut max_va = 0u64;
    let mut user_leaves = 0u64;
    unsafe {
        let root_ptr = &raw const crate::arch::mm::mmu_init::ROOT_PAGE_TABLE;
        let root = &*root_ptr;
        for i in 0..2usize {
            let pte2 = root.get(i);
            if !pte2.is_valid() || pte2.is_leaf() {
                continue;
            }
            l1n += 1;
            let t1 = crate::arch::mm::mmu_init::get_page_table_virt(
                pte2.ppn() << 12,
            );
            if t1.is_null() {
                continue;
            }
            for j in 0..512usize {
                let pte1 = (*t1).get(j);
                if !pte1.is_valid() {
                    continue;
                }
                let va = ((i as u64) << 30) | ((j as u64) << 21);
                if va < min_va { min_va = va; }
                if va + (1 << 21) > max_va { max_va = va + (1 << 21); }
                if pte1.is_leaf() {
                    if pte1.is_user() { user_leaves += 1; }
                    continue;
                }
                l0n += 1;
                let t0 = crate::arch::mm::mmu_init::get_page_table_virt(
                    pte1.ppn() << 12,
                );
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
    memwatch_put_dec(l1n);
    taskdump_raw_line(b" l0=");
    memwatch_put_dec(l0n);
    taskdump_raw_line(b" uleaf=");
    memwatch_put_dec(user_leaves);
    if min_va != u64::MAX {
        taskdump_raw_line(b" va=0x");
        memwatch_put_hex(min_va);
        taskdump_raw_line(b"-0x");
        memwatch_put_hex(max_va);
    }
    taskdump_raw_line(b"\n");
}
/// (refcount, on-freelist, reserved) of one page descriptor for the census scan.
///
/// # Contract (F18)
/// INV:F18: read-only descriptor access inside the mapped vmemmap span; the flags atomics are the same ones every other reader uses lock-free.
pub(crate) fn leak_page_counts3(page: *const crate::mm::types::Page) -> (i32, bool, bool) {
unsafe {
            (
                (*page).refcount(),
                (*page).test_flag(crate::mm::types::PageFlag::OnFreelist),
                (*page).is_reserved(),
            )
        }
}

/// (anonymous, cow, mapped, lru) of one page descriptor for the census scan.
///
/// # Contract (F18)
/// INV:F18: read-only descriptor access inside the mapped vmemmap span.
pub(crate) fn leak_page_flags4(page: *const crate::mm::types::Page) -> (bool, bool, bool, bool) {
unsafe {
            (
                (*page).is_anonymous(),
                (*page).is_cow(),
                (*page).is_mapped(),
                (*page).test_flag(crate::mm::types::PageFlag::Lru),
            )
        }
}


// ------------ frame-pointer walk, register/CSR reads and debugfs provoques (INV:F2) ------------

/// Read the current frame pointer register (`s0` on riscv64, `rbp` on x86_64).
/// INV:F2: pure register reads (s0/rbp/x29) of the running CPU's own frame pointer; no memory access.
#[inline]
pub fn current_frame_pointer() -> u64 {
    let fp: u64;
    #[cfg(feature = "riscv64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mv {}, s0", out(reg) fp, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack));
    }
    #[cfg(feature = "aarch64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64", feature = "aarch64")))]
    {
        fp = 0;
    }
    fp
}
/// Return address of the calling function (for "current" trace entries).
/// INV:F2: reads the live ra/LR (register or [rbp+8]/[x29+8] with frame pointers forced on); no stores.
#[inline]
pub fn caller_return_address() -> u64 {
    let ra: u64;
    #[cfg(feature = "riscv64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: frame pointers are forced on (force-frame-pointers=yes), so
    // [rbp+8] is this function's return address.
    unsafe {
        core::arch::asm!("mov {}, [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "aarch64")]
    // SAFETY: x29 is the frame-chain base; [x29+8] is the LR slot.
    unsafe {
        core::arch::asm!("ldr {}, [x29, 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64", feature = "aarch64")))]
    {
        ra = 0;
    }
    ra
}
/// Unwind one frame: given a frame pointer, return (return_addr, next_fp).
///
/// riscv64 frame layout: [fp] = saved_fp, [fp+8] = return_addr.
/// x86_64 frame layout:  [rbp] = saved_rbp, [rbp+8] = return_addr.
/// INV:F2: caller validated `fp` (kernel range, aligned); the two frame-word reads are the standard unwind operation.
#[inline]
pub(crate) fn unwind_frame(fp: u64) -> Option<(u64, u64)> {
    // SAFETY: caller validated `fp` (kernel range, aligned); reads of the
    // two frame words are the standard unwind operation.
    unsafe {
        let ret_addr = *((fp + 8) as *const u64);
        let next_fp = *(fp as *const u64);
        if ret_addr == 0 {
            return None;
        }
        Some((ret_addr, next_fp))
    }
}
/// aarch64: capture x0..x30 into the dump array (x0 at slot 0).
///
/// # Contract (F2)
/// `regs` is a 31-slot stack buffer; the asm stores into it and touches no other memory.
#[cfg(feature = "aarch64")]
pub(crate) fn save_regs_aarch64(regs: &mut [u64; 31]) {
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
            buf = inout(reg) regs.as_mut_ptr() => _,
            options(nostack)
        );
    }
}

/// riscv64: save x1..x31 into the dump array.
///
/// # Contract (F2)
/// `regs` is a 31-slot stack buffer; the asm stores into it and touches no other memory.
#[cfg(feature = "riscv64")]
pub(crate) fn save_regs_riscv64(regs: &mut [u64; 31]) {
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
            "sd a1,  10*8({buf})",
            "sd a2,  11*8({buf})",
            "sd a3,  12*8({buf})",
            "sd a4,  13*8({buf})",
            "sd a5,  14*8({buf})",
            "sd a6,  15*8({buf})",
            "sd a7,  16*8({buf})",
            "sd s2,  17*8({buf})",
            "sd s3,  18*8({buf})",
            "sd s4,  19*8({buf})",
            "sd s5,  20*8({buf})",
            "sd s6,  21*8({buf})",
            "sd s7,  22*8({buf})",
            "sd s8,  23*8({buf})",
            "sd s9,  24*8({buf})",
            "sd s10, 25*8({buf})",
            "sd s11, 26*8({buf})",
            "sd t3,  27*8({buf})",
            "sd t4,  28*8({buf})",
            "sd t5,  29*8({buf})",
            "sd t6,  30*8({buf})",
            buf = inout(reg) regs.as_mut_ptr() => _,
            options(nostack, preserves_flags)
        );
    }
}

/// x86_64: spill the callee-saved GPRs (push/spill/pop).
///
/// # Contract (F2)
/// `regs` is a 31-slot stack buffer; the pushes are balanced by the rsp adjustment, so only this scratch is touched.
#[cfg(feature = "x86_64")]
pub(crate) fn save_regs_x86_64(regs: &mut [u64; 31]) {
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

/// Read (sstatus, scause, stval, sepc) in one asm block.
///
/// # Contract (F2)
/// Valid at any privilege the kernel runs in (S-mode CSR reads); no memory access.
#[cfg(feature = "riscv64")]
pub(crate) fn csr_quad_read_riscv() -> (u64, u64, u64, u64) {
    let (sstatus, scause, stval, sepc): (u64, u64, u64, u64);
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

/// Read (esr_el1, elr_el1, far_el1, spsr_el1) in one asm block.
///
/// # Contract (F2)
/// Pure system-register reads, valid in any context; no memory access.
#[cfg(feature = "aarch64")]
pub(crate) fn csr_quad_read_aarch64() -> (u64, u64, u64, u64) {
    let (esr, elr, far, spsr): (u64, u64, u64, u64);
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

/// The provoke-oops pagefault read: one volatile read at the NULL-adjacent probe address.
pub(crate) fn provoke_pagefault_read(p: *const u64) -> u64 {
unsafe { core::ptr::read_volatile(p) }
}

/// The provoke-oops illegal-instruction fault (riscv64).
#[cfg(feature = "riscv64")]
pub(crate) fn provoke_illegal_riscv() {
                    unsafe {
                        core::arch::asm!("csrr x0, 0x300", options(nomem, nostack));
                    }
}

/// The provoke-oops illegal-instruction fault (x86_64).
#[cfg(feature = "x86_64")]
pub(crate) fn provoke_illegal_x86() {
                    unsafe {
                        core::arch::asm!("ud2", options(nomem, nostack));
                    }
}

/// Read this CPU's PerCpu.magic via %gs (the percpu misdirection canary).
///
/// # Contract (F18)
/// A plain data read of the ACTIVE GS base — exactly what the trap stubs do; runs in IPI context on the frozen CPU.
#[cfg(feature = "x86_64")]
pub(crate) fn gs_percpu_magic_read() -> u64 {
    let gs_magic_q: u64;
    unsafe {
        core::arch::asm!(
            "mov {0}, qword ptr gs:[{1}]",
            out(reg) gs_magic_q,
            const crate::arch::x86_64::smp::PC_MAGIC,
            options(nomem, nostack, preserves_flags)
        );
    }
    gs_magic_q
}

/// INV:F3: the validated walk — fp/range checks before each volatile read; a stale or corrupt chain terminates the walk.
#[inline(never)]
#[cfg(feature = "riscv64")]
pub unsafe fn walk_fp_chain(mut fp: u64, out: &mut [u64]) {
    for slot in out.iter_mut() {
        *slot = 0;
        if !(fp >= 0x8000_0000_0000_0000 && fp & 7 == 0 && plausible_fp(fp)) {
            return;
        }
        let ra = core::ptr::read_volatile((fp - 8) as *const u64);
        let next = core::ptr::read_volatile((fp - 0x10) as *const u64);
        *slot = ra;
        if !(next >= 0x8000_0000_0000_0000 && next & 7 == 0 && next > fp && plausible_fp(next)) {
            return;
        }
        fp = next;
    }
}
/// x86_64 twin. The riscv64 walk above reads the RISC-V frame layout
/// (s0 at the TOP of the frame: saved ra at [fp-8], saved s0 at
/// [fp-0x10]); SysV x86_64 stores the saved rbp AT [rbp] with the return
/// address at [rbp+8]. Running the riscv64 layout on x86_64 dereferenced
/// the frame's LOCAL VARIABLES — locals that happened to hold kernel
/// pointers passed the plausibility gate, so nearly every allocation
/// interned a UNIQUE garbage chain, exhausting PSITE_SLOTS within the
/// first minutes of a boot and dropping every real call site from the
/// MEMPSITES attribution (observed live on the pt4 audit boot: f2 slots
/// full of ASCII data like 0x49657069505d737d, top net=198 against
/// 442k page frees).
/// aarch64 twin: AAPCS64 frame layout stores the saved x29 (caller fp)
/// at [fp] and lr at [fp+8] — the same walk shape as the x86_64 SysV
/// twin below (NOT the riscv64 top-of-frame layout).
/// INV:F3: the AAPCS64 twin — same plausibility discipline, [fp]=saved x29, [fp+8]=lr.
#[inline(never)]
#[cfg(feature = "aarch64")]
pub unsafe fn walk_fp_chain(mut fp: u64, out: &mut [u64]) {
    for slot in out.iter_mut() {
        *slot = 0;
        if !(fp >= 0xffff_8000_0000_0000 && fp & 7 == 0 && plausible_fp(fp)) {
            return;
        }
        // AAPCS64: [fp] = caller's saved x29, [fp+8] = this frame's
        // return address. Stacks grow down, so the caller's fp is
        // strictly greater than the current one.
        let next = core::ptr::read_volatile(fp as *const u64);
        let ra = core::ptr::read_volatile((fp + 8) as *const u64);
        *slot = ra;
        if !(next >= 0xffff_8000_0000_0000 && next & 7 == 0 && next > fp && plausible_fp(next)) {
            return;
        }
        fp = next;
    }
}
/// INV:F3: the SysV twin — same plausibility discipline, [rbp]=saved rbp, [rbp+8]=return address.
#[inline(never)]
#[cfg(feature = "x86_64")]
pub unsafe fn walk_fp_chain(mut fp: u64, out: &mut [u64]) {
    for slot in out.iter_mut() {
        *slot = 0;
        if !(fp >= 0x8000_0000_0000_0000 && fp & 7 == 0 && plausible_fp(fp)) {
            return;
        }
        // SysV: [fp] = caller's saved rbp, [fp+8] = this frame's return
        // address. Stacks grow down, so the caller's rbp is strictly
        // greater than the current one (same ascending check as riscv64).
        let next = core::ptr::read_volatile(fp as *const u64);
        let ra = core::ptr::read_volatile((fp + 8) as *const u64);
        *slot = ra;
        if !(next >= 0x8000_0000_0000_0000 && next & 7 == 0 && next > fp && plausible_fp(next)) {
            return;
        }
        fp = next;
    }
}

// ------------ stale-task adoption edges (INV:F18) ------------

/// Adopt a possibly-stale task handle as a TaskBorrow view (TaskBorrow's short-read contract).
///
/// # Contract (F18)
/// INV:F18 (one adopt edge): the handle came from a diagnostic producer that captured a live task; the view is read-only for fields stable until the struct's final put.
pub(crate) fn task_view(p: *const crate::process::types::Task) -> Option<crate::process::types::TaskBorrow> {
unsafe { crate::process::types::TaskBorrow::from_raw(p as *mut crate::process::types::Task) }
}

