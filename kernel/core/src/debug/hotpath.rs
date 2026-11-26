//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The debug subsystem's code (seed-core-design §2c.3): everything of the
//! subsystem that is neither `unsafe.rs` nor `types.rs`, in one file.
//!
//! Sections follow the order of the old module declarations; each banner
//! names the topic file it was and the file's module doc follows it as the
//! section's header comment. Inside a section the items keep their original
//! order. Type definitions are in `types.rs`, the audited unsafe edges in
//! `unsafe.rs` (wildwatch's and scribble's arch halves stay inline modules of
//! their sections, picked by the same `#[cfg]` as before). The per-file
//! `#![forbid(unsafe_code)]` inner attributes of the old topic files are
//! dropped here; they return in pass 3, when the scattered unsafe is
//! collected.
//!
//! The file's NAME overstates until the shrink stage has run: it holds all
//! of the subsystem's code under the hot-path name, not a verified set of
//! hot-path functions (§2c.3, user decision 2026-11-25). debug is the
//! diagnostic subsystem, and most of what is here is the diagnostic
//! instruments' cold halves awaiting the §2c.3 sweep (replace / fold in /
//! delete).

use super::types::{
    AtomicU8Cell,
    AwGuard,
    BigRingSlot,
    CAP,
    COMM,
    Census,
    CommCount,
    CommEnt,
    ConsoleWriter,
    DEFER_FOCUS_MAX,
    DEFER_LINE_MAX,
    DeferredEmit,
    DeferredFocus,
    DfxSwitch,
    DumpStage,
    EmitBudget,
    EofSpin,
    ErrSlot,
    ExitRec,
    ForkPhaseStat,
    HeadWriter,
    Holder,
    If0Zone,
    Kind,
    KmsgSender,
    Line,
    LiveSite,
    LoSlot,
    NameRec,
    PageSite,
    PhaseStat,
    PrSlot,
    RdDataSlot,
    RdFailSlot,
    RecordHeader,
    RefRingEntry,
    RestSite,
    RipSlot,
    RxSlot,
    SITE_FRAMES,
    Site,
    Slot,
    SlowEnt,
    SlowtaskSlot,
    SysStat,
    TEv,
    TaintFlags,
    TreeHit,
    UART_HIST,
    UartStat,
    Watch,
    ZSTK_DEPTH,
};

#[cfg(feature = "x86_64")]
use super::types::{FrozenAnswer};

extern crate alloc;

use core::sync::atomic::{
    AtomicBool,
    AtomicI32,
    AtomicI64,
    AtomicU32,
    AtomicU64,
    AtomicU8,
    AtomicUsize,
    Ordering,
};
use core::fmt;
use core::fmt::Write as _;
use crate::config::MAX_CPUS;
use alloc::format;
use alloc::string::{String, ToString};
use crate::safety::frame::KernelStackWindow;
use crate::mm::hotpath;
use crate::mm::types::MetaFinding;
use crate::mm::hotpath::EMPTY_FINDING;
use core::sync::atomic::Ordering::Relaxed;
use core::mem::size_of;
use core::panic::Location;
// ════════════════ debug root ══════════════════════════
// (was mod.rs: the subsystem's init and debugfs knob registration,
// the panic-path macros and the shared walk_read helper; its module
// declarations are gone — every path now names hotpath or types)
// (mod.rs's description of the r#unsafe module, whose file keeps its own
// //! doc — kept here because mod.rs is three lines now:)
// The hunt-era forensics family's shared raw edges (round-5
// consolidation; the irreducibility proofs in
// docs/architecture/safety-and-performance.md name this file as the
// owner of the stale-handle read family).
// DEBUG Subsystem — Debug, Fault, and Diagnostics
//
// Unified diagnostic subsystem for the Rux kernel. Consolidates scattered
// debugging features into a single module and implements critical missing
// capabilities for production kernel debugging.
//
// # Modules
//
// - `taint`      — Kernel taint bitmask
// - `backtrace`  — Reusable `dump_stack()`, `dump_regs()`, `dump_csrs()`
// - `bug`        — `WARN()`, `BUG()` macros
// - `softlockup` — Softlockup detector (per-CPU timestamp check)
// - `hung_task`  — Hung task detector (khungtaskd kernel thread)
//
// # Macros
//
// ```rust
// warn_on!(condition)       // Non-fatal warning if condition is true
// warn_on_once!(condition)  // Same but fires only once per callsite
// bug_on!(condition)        // Fatal BUG if condition is true
// bug!()                    // Unconditional fatal BUG
// ```

// (round-5: debug/sbi_debug.rs DELETED — the SBI low-level print helpers
// had zero consumers anywhere in the tree; same dead-hunt-module class
// as round-4D's hexdump.rs.)
// (2026-11-23, §7 retirement round: debug/r83hunt.rs DELETED — the R74/R83
// task-header hunt closed with c1539f6ed (the trap.S producer) and the
// fc5f23d47 + 2e9755c67 corruption family; the module's 8 call-site
// stubs and its rux:r83scan gate event went with it.)
// (2026-11-23, §7 retirement round: debug/treehunt.rs DELETED — the
// E8-REPAR unlinked-snapshot hunt closed with the 7d4481320/6f5e397ad
// reparent fixes; its timer-tick stub, its rux:treehunt gate event and
// the parent_children_head_ptr accessor went with it.)
// The observability hook table (DebugHooks) is crate::api::debug.

/// Initialize the DEBUG subsystem.
///
/// Called during kernel boot after `sched::init()`.
/// Starts the softlockup detector and hung task detector.
pub fn init() {
    self::init_from_cmdline();
    // mmprof (cmdline mmprof=1): spawn the task-context dump thread here
    // (cmdline is parsed; the thread prints with the task emit budget).
    self::ensure_dump_thread();
    // oops=panic seeds panic_on_oops (the sysctl can flip it later).
    self::oops_init_from_cmdline();
    // Wild-write tripwire arms between the timekeeper's init and
    // start_secondaries (no TLB shootdown needed; see wildwatch.rs).
    self::arm();
    self::softlockup_init();
    // hung_task detector is deferred — it requires kthread infrastructure
    // that may not be fully ready during early boot. Enable after testing.
    // self::hung_task_init();
    // Publish the instrument switches in debugfs so they can be armed
    // and disarmed while the system runs.
    register_debugfs_knobs();
    // Publish the tracepoint control surface (tracefs) — the file set
    // moved to the debug crate (debug::trace_files); the row is
    // registered by the composition bin before this runs.
    crate::api::debug::trace_files_register();

    // aarch64 bring-up probe (debug-aa64-uaccess-selftest): exercise the
    // uaccess exception-table fault containment with a poisoned user
    // pointer before init exec — every access must come back EFAULT
    // with no oops. Compile-time gated (the aarch64 cmdline cannot
    // carry a debug= switch yet: the EL2 trampoline clobbers the x0 DTB
    // pointer, so cmdline parsing falls back to the default string).
    #[cfg(all(feature = "aarch64", feature = "debug-aa64-uaccess-selftest"))]
    crate::arch::aarch64::uaccess::selftest();
}

/// Register the DEBUG instrument switches in debugfs.
///
/// Linux keeps runtime debug knobs out of the stable userspace ABI and
/// exposes them in debugfs instead. Rux follows the same rule and the
/// same conventions:
///
/// - one file per switch, named after its `debug=` spelling;
/// - the file reads `Y` or `N` (Linux's `debugfs_create_bool`
///   convention);
/// - writing `Y`/`y`/`1` arms the instrument, `N`/`n`/`0` disarms it,
///   anything else is `EINVAL` (Linux's `debugfs_bool_set`).
///
/// ```text
/// /sys/kernel/debug/taskdump   -> Y
/// ```
///
/// Returns the number of knobs registered.
pub fn register_debugfs_knobs() -> usize {
    use crate::fs::hotpath::debugfs;
    if !debugfs::create_dir("/debug") {
        return 0;
    }
    let mut count = 0;
    for sw in self::ALL {
        let path = alloc::format!("/debug/{}", sw.name());
        let gen = move || {
            // Linux debugfs bool files: "Y\n" / "N\n".
            let mut out = alloc::vec![b'N', b'\n'];
            if self::enabled(sw) {
                out[0] = b'Y';
            }
            out
        };
        let store = move |buf: &[u8]| -> i32 {
            let text = core::str::from_utf8(buf).unwrap_or("").trim();
            if text.is_empty() {
                // The guest's `echo v > file` writes the value and the
                // trailing newline as SEPARATE write() calls: the empty
                // second write is a no-op, not an error (same convention
                // as /proc/[pid]/oom_score_adj).
                return 0;
            }
            let on = match text {
                "Y" | "y" | "1" | "true" | "on" => true,
                "N" | "n" | "0" | "false" | "off" => false,
                _ => return crate::errno::Errno::InvalidArgument.as_neg_i32(),
            };
            self::set(sw, on);
            crate::pr_info!(
                "debug: runtime switch '{}' {}",
                sw.name(),
                if on { "enabled" } else { "disabled" }
            );
            0
        };
        if debugfs::create_file(&path, 0o644, gen, Some(store)) {
            count += 1;
        }
    }

    // The oops provoquer — LKDTM's provoke-crash discipline: a debugfs
    // write that faults in kernel mode on purpose, so the Oops path
    // (report, pstore capture, task-death policy) is exercisable from
    // a guest shell instead of waiting for a real kernel bug. The
    // faulting read sits on no lock path: the write handler itself
    // performs it, so the recoverable oops kills only the writer.
    // Kinds: "pagefault" (the default) — a NULL-adjacent kernel read
    // with no exception-table entry; "illegal" — an M-mode CSR in
    // S-mode (riscv64) / ud2 (x86_64).
    {
        let gen = || {
            b"write: pagefault | illegal\n".to_vec()
        };
        let store = |buf: &[u8]| -> i32 {
            let text = core::str::from_utf8(buf).unwrap_or("").trim();
            match text {
                "" => 0, // the trailing-newline write, same convention
                "pagefault" | "1" => {
                    crate::pr_info!("debug: provoking kernel page fault oops");
                    // SAFETY: deliberate fault — read address 0x10 (a
                    // NULL-adjacent page with no mapping and no fixup
                    // entry on this instruction).
                    let p = 0x10usize as *const u64;
                    let _ = unsafe { core::ptr::read_volatile(p) };
                    // Unreachable when the oops policy runs; if the
                    // fault was fixed up somehow, report it.
                    crate::pr_err!("debug: provoke-oops fault did not fire");
                    crate::errno::Errno::IOError.as_neg_i32()
                }
                "illegal" => {
                    crate::pr_info!("debug: provoking kernel illegal-instruction oops");
                    #[cfg(feature = "riscv64")]
                    // SAFETY: deliberate fault — machine-mode CSR access
                    // from S-mode traps IllegalInstruction.
                    unsafe {
                        core::arch::asm!("csrr x0, 0x300", options(nomem, nostack));
                    }
                    #[cfg(feature = "x86_64")]
                    // SAFETY: deliberate fault — ud2 is reserved for
                    // traps (the BUG instruction).
                    unsafe {
                        core::arch::asm!("ud2", options(nomem, nostack));
                    }
                    crate::pr_err!("debug: provoke-oops illegal did not fire");
                    crate::errno::Errno::IOError.as_neg_i32()
                }
                _ => crate::errno::Errno::InvalidArgument.as_neg_i32(),
            }
        };
        if debugfs::create_file("/debug/provoke-oops", 0o644, gen, Some(store)) {
            count += 1;
        }
    }
    count
}

// ============================================================================
// Re-exported macros
// ============================================================================

/// `warn_on!` — Non-fatal warning if condition is true.
///
/// Returns `true` if the condition was true (warning was printed),
/// `false` otherwise. Always evaluates the condition.
///
/// # Example
/// ```rust
/// if warn_on!(ptr.is_null()) {
///     // warning was printed
/// }
/// ```
#[macro_export]
macro_rules! warn_on {
    ($cond:expr) => {{
        if $cond {
            $crate::debug::hotpath::warn(file!(), line!(), stringify!($cond));
            true
        } else {
            false
        }
    }};
}

/// `warn_on_once!` — Non-fatal warning, fires only once per callsite.
///
/// Uses a static `AtomicBool` per callsite to ensure the warning
/// is printed at most once per boot.
#[macro_export]
macro_rules! warn_on_once {
    ($cond:expr) => {{
        static ONCE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if $cond && $crate::debug::hotpath::warn_on_once_check(&ONCE) {
            $crate::debug::hotpath::warn(file!(), line!(), stringify!($cond));
            true
        } else {
            false
        }
    }};
}

/// `bug_on!` — Fatal BUG if condition is true.
///
/// If the condition is true, prints a BUG message with stack trace
/// and calls `panic!()`. The kernel will not continue.
#[macro_export]
macro_rules! bug_on {
    ($cond:expr) => {
        if $cond {
            $crate::debug::hotpath::bug_bug(file!(), line!());
        }
    };
}

/// `bug!` — Unconditional fatal BUG.
///
/// Always triggers a BUG and calls `panic!()`.
#[macro_export]
macro_rules! bug {
    () => {
        $crate::debug::hotpath::bug_bug(file!(), line!());
    };
}

/// DEBUG shared: diagnostic read of ONE `T: Copy` through a task's page
/// tables (round-4 consolidation — the ONE audited PageTableWalker edge
/// for the debug family; INV-DFXWALK-1: `root` is the diagnosed task's own
/// mm root and the read is read-only through the linear map).
pub(crate) fn walk_read<T: Copy>(root: u64, va: u64) -> Option<T> {
    use crate::arch::mm::memory_layout::{phys_to_virt, PhysAddr};
    use crate::arch::mm::mm_ops::PageTableWalker;
    // SAFETY: INV-DFXWALK-1 — the backend walk is the arch contract
    // (caller holds the address space), and the linear map covers all of
    // RAM for a read-only diagnostic access.
    let (ppn, _) = unsafe { PageTableWalker::walk(root, va) }?;
    let base = phys_to_virt(PhysAddr((ppn << 12) as u64));
    Some(unsafe {
        core::ptr::read_volatile((base.as_usize() + (va & 0xfff) as usize) as *const T)
    })
}


// ════════════════ mmforensics ═════════════════════════
// SPDX-License-Identifier: MIT OR Apache-2.0
// mmforensics: deep frame-tracking capture at double-tracking events.
//
// The pcachewedge lane (2026-11-15) contained the RAW-FREE-OF-LIVE-TABLE
// cascade with two refuse-and-leak guards (cache-side live-PT release
// refusal, buddy-vs-pcp double-link refusal), but 1-2 residual echo
// events per boot remained whose decode required naming the STALE TREE
// POINTER's origin: which structure still references a frame that
// re-entered circulation, and when that reference was created.
//
// This module is the capture side of `rux:mmforensics (retired debug=mmforensics)` (existing gated
// family — no new switch). At a double-tracking event it dumps the
// frame's FULL tracking state:
//
//   1. page-descriptor state (refcount/mapcount/order/flags/type/
//      mapping/index — the buddy + page-cache view),
//   2. PT-ledger bit + boot set,
//   3. the frame's incarnation history from the forensic rings
//      (PT_STAMP_RING — now with the stamping tree's ROOT (§6 residual:
//      "PT_STAMP_RING has no root field"), PT_FREE_RING, ZTRACE_ALLOC,
//      ZTRACE_FREE, PTEI_RING data-page installs),
//   4. per-CPU pageset rows naming the pfn,
//   5. and the decisive one: a bounded scan of EVERY live task's page
//      table tree for links pointing at the pfn — the reverse map the
//      kernel otherwise lacks. A frame referenced by two trees (or twice
//      by one) is double-tracked; the tree that is NOT the stamping root
//      holds the stale pointer.
//
// Callers (all task-context, no zone/pcp/page-cache locks held at the
// call site): the RAW-FREE watchpoint in `mm::zone::Zone::free_pages`,
// the live-PT release refusal in `fs::page_cache::release_page_frame`,
// the PTLEDGER DOUBLE-FREE refusal and the FUT-STALE-TREE census in the
// arch teardown walks. The capture itself takes NO locks except the
// pid-hash bucket locks inside `for_each_task_try` (non-blocking try
// variant: a bucket held at snapshot time is skipped and reported).
// Reading foreign trees is racy-by-design forensics: every link is
// bounds-checked (`phys_valid` + RAM top) before descent and every
// walk carries a hard entry budget.

/// Per-boot cap on captures: the events occur <=2/boot; four captures
/// carry the evidence, more would only flood the serial console.
static CAPTURES: AtomicUsize = AtomicUsize::new(0);

// ==================== ref-transition ring (early-put hunt, 2026-11-15) ====================
//
// The mmforensics capture answers "what does the frame look like NOW";
// the ORIGINAL-producer hunt (why a PCACHE mapping's descriptor
// reference is consumed before unmap) additionally needs WHO performed
// every refcount transition on the frame in the past. Every mm-side
// put on a mapped leaf runs through `Page::put_page_site` (driftwatch
// attribution); installs and fork copies run through `Page::get_page`.
// This ring records BOTH for page-cache-typed frames only (the other
// get/put users — shm/ashmem/binder/io_uring/perf — hold their own
// frames), gated by the mmforensics switch so standard boots pay one
// relaxed load per transition. The capture's ring replay then prints
// the get/put sequence with sites: a put whose `old` shows the mapping
// reference already missing names the early consumer directly.


// 8192 entries (256 KiB of statics): a fammm boot's PageCache mapping
// transitions can push far past 1024 entries between the early put and
// the capture that replays them — the deeper ring keeps the window.
const REF_RING_LEN: usize = 8192;
pub static REF_RING: [RefRingEntry; REF_RING_LEN] =
    [const {
        RefRingEntry {
            ppn: AtomicU64::new(0),
            kind_site: AtomicU64::new(0),
            old: AtomicI32::new(0),
            mc: AtomicI32::new(0),
            pid: AtomicU32::new(0),
        }
    }; REF_RING_LEN];
pub static REF_CUR: AtomicUsize = AtomicUsize::new(0);

/// Record a get/put transition on a page-cache-typed descriptor.
/// `kind`: 0 = get_page, 1 = put (site-attributed). No-op unless
/// `rux:mmforensics (retired debug=mmforensics)` is armed; takes no locks (safe under PTE locks).
#[inline]
pub fn note_ref(page: *const crate::mm::types::Page, kind: u8, site: u8, old: i32) {
    if !crate::trace::EVENT_MMFORENSICS.armed() {
        return;
    }
    // SAFETY: the caller passes a valid page descriptor it is operating on.
    let p = unsafe { &*page };
    use crate::mm::types::PageType;
    if p.page_type() != PageType::PageCache {
        return;
    }
    let ppn = crate::mm::hotpath::page_to_pfn(page) as u64;
    let idx = REF_CUR.fetch_add(1, Ordering::Relaxed) % REF_RING_LEN;
    let e = &REF_RING[idx];
    e.ppn.store(ppn, Ordering::Relaxed);
    e.kind_site.store(((kind as u64) << 8) | site as u64, Ordering::Relaxed);
    e.old.store(old, Ordering::Relaxed);
    e.mc.store(p.mapcount(), Ordering::Relaxed);
    e.pid.store(crate::process::hotpath::current_pid(), Ordering::Relaxed);
}


/// Current task's page-table root (0 when the context has no user mm).
fn current_root_ppn() -> u64 {
    crate::sched::hotpath::current()
        .and_then(|t| t.address_space_arc())
        .map(|mm| mm.root_ppn())
        .unwrap_or(0)
}

/// Replay every forensic ring naming `ppn`, newest first. The rings are
/// per-arch statics with identical names/shapes on x86_64 and riscv64
/// (PT_STAMP_RING / PT_FREE_RING / PTEI_RING under
/// `arch::mm::mmu_init`, ZTRACE_ALLOC / ZTRACE_FREE under `mm::zone`),
/// so this compiles once for both. Each replay is capped (the first
/// matches carry the history; more would flood the console).
#[cfg(any(feature = "x86_64", feature = "riscv64"))]
fn ring_history(ppn: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    const MAX_PER_RING: usize = 6;

    // Table stamps (alloc side; root recorded when mmforensics armed).
    let cur = crate::arch::mm::mmu_init::PT_STAMP_CURSOR.load(Relaxed);
    let ring = &crate::arch::mm::mmu_init::PT_STAMP_RING;
    let mut n = 0;
    for k in 0..ring.len() {
        let idx = (cur + ring.len() - 1 - k) % ring.len();
        if ring[idx].ppn.load(Relaxed) == ppn {
            crate::pr_err!(
                "  hist stamp[age {}]: ppn={:#x} by pid={} root={:#x}",
                k,
                ppn,
                ring[idx].pid.load(Relaxed),
                ring[idx].root.load(Relaxed)
            );
            n += 1;
            if n >= MAX_PER_RING {
                break;
            }
        }
    }
    if n == 0 {
        crate::pr_err!("  hist stamp: no ring entry names this ppn");
    }

    // Table frees (take side: pid, teardown site, freeing root).
    let cur = crate::arch::mm::mmu_init::PT_FREE_CURSOR.load(Relaxed);
    let ring = &crate::arch::mm::mmu_init::PT_FREE_RING;
    let mut n = 0;
    for k in 0..ring.len() {
        let idx = (cur + ring.len() - 1 - k) % ring.len();
        if ring[idx].ppn.load(Relaxed) == ppn {
            crate::pr_err!(
                "  hist free[age {}]: ppn={:#x} by pid={} site={} root={:#x}",
                k,
                ppn,
                ring[idx].pid.load(Relaxed),
                ring[idx].site.load(Relaxed) as u8 as char,
                ring[idx].root.load(Relaxed)
            );
            n += 1;
            if n >= MAX_PER_RING {
                break;
            }
        }
    }
    if n == 0 {
        crate::pr_err!("  hist free: no ring entry names this ppn");
    }

    // Zone allocator ledger (raw pfn<<4|order entries; no pid).
    for (ring, cur, name) in [
        (
            &crate::mm::hotpath::ZTRACE_ALLOC,
            &crate::mm::hotpath::ZALLOC_CUR,
            "zalloc",
        ),
        (
            &crate::mm::hotpath::ZTRACE_FREE,
            &crate::mm::hotpath::ZFREE_CUR,
            "zfree",
        ),
    ] {
        let c = cur.load(Relaxed);
        let mut n = 0;
        for k in 0..ring.len() {
            let idx = (c + ring.len() - 1 - k) % ring.len();
            let v = ring[idx].load(Relaxed);
            if v != 0 && (v >> 4) == (ppn as u64) {
                crate::pr_err!(
                    "  hist {}[age {}]: pfn={:#x} order={}",
                    name,
                    k,
                    v >> 4,
                    v & 0xF
                );
                n += 1;
                if n >= 2 {
                    break;
                }
            }
        }
    }

    // User PTE installs naming this ppn as DATA (root, va) — records
    // whether the frame also lived as a mapped data page.
    let cur = crate::arch::mm::mmu_init::PTEI_CUR.load(Relaxed);
    let ring = &crate::arch::mm::mmu_init::PTEI_RING;
    let mut n = 0;
    for k in 0..ring.len() {
        let idx = (cur + ring.len() - 1 - k) % ring.len();
        if ring[idx].ppn.load(Relaxed) == ppn {
            crate::pr_err!(
                "  hist pte-install[age {}]: ppn={:#x} root={:#x} va={:#x}",
                k,
                ppn,
                ring[idx].root.load(Relaxed),
                ring[idx].va.load(Relaxed)
            );
            n += 1;
            if n >= MAX_PER_RING {
                break;
            }
        }
    }

    // Refcount transitions (early-put hunt): the get/put sequence with
    // driftwatch sites. Newest first; the full relevant window matters,
    // so this replay prints more matches than the others.
    let site_name = |s: u64| -> &'static str {
        match s {
            0 => "TEARDOWN_RV",
            1 => "TEARDOWN_X86_L1",
            2 => "TEARDOWN_X86_L2",
            3 => "TEARDOWN_X86_L3",
            4 => "COW_RV",
            5 => "COW_X86",
            6 => "UNMAP_X86",
            7 => "UNMAP_RV",
            8 => "VMSCAN_SWAP",
            9 => "SHM_DROP",
            10 => "ASHMEM_DROP",
            11 => "BINDER_DROP",
            12 => "BINDER_ZAP",
            13 => "IOURING_DROP",
            14 => "PAGECACHE",
            _ => "OTHER",
        }
    };
    let cur = REF_CUR.load(Relaxed);
    let mut n = 0;
    for k in 0..REF_RING_LEN {
        let idx = (cur + REF_RING_LEN - 1 - k) % REF_RING_LEN;
        if REF_RING[idx].ppn.load(Relaxed) == ppn {
            let ks = REF_RING[idx].kind_site.load(Relaxed);
            let (kind, site) = ((ks >> 8) as u8, (ks & 0xFF) as u64);
            crate::pr_err!(
                "  hist ref[age {}]: ppn={:#x} {} site={} old={} mc={} pid={}",
                k,
                ppn,
                if kind == 0 { "GET" } else { "PUT" },
                // GETs carry no driftwatch site (they are installs/fork
                // copies); only PUTs are site-attributed.
                if kind == 0 { "install" } else { site_name(site) },
                REF_RING[idx].old.load(Relaxed),
                REF_RING[idx].mc.load(Relaxed),
                REF_RING[idx].pid.load(Relaxed)
            );
            n += 1;
            if n >= 16 {
                break;
            }
        }
    }
    if n == 0 {
        crate::pr_err!("  hist ref: no recorded transition names this ppn");
    }
}

/// Deep capture of one frame's tracking state. Rate-limited; a no-op
/// unless `rux:mmforensics (retired debug=mmforensics)` armed the switch.
pub fn capture_frame(pfn: usize, why: &'static str) {
    if !crate::trace::EVENT_MMFORENSICS.armed() {
        return;
    }
    if CAPTURES.fetch_add(1, Ordering::Relaxed) >= 4 {
        return;
    }
    let pid = crate::process::hotpath::current_pid();
    crate::pr_err!(
        "=== MMFORENSICS capture #{}: pfn={:#x} why={} pid={} root={:#x} ===",
        CAPTURES.load(Ordering::Relaxed),
        pfn,
        why,
        pid,
        current_root_ppn()
    );

    // ---- 1. page-descriptor state (buddy + page-cache view) ----
    let page = crate::mm::hotpath::pfn_to_page(pfn);
    if page.is_null() {
        crate::pr_err!("  desc: NO DESCRIPTOR (pfn outside managed range)");
    } else {
        // SAFETY: read-only descriptor access on the diagnostic path.
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

    // ---- 2. PT ledger ----
    let ppn = pfn as u64;
    crate::pr_err!(
        "  ledger: live={} boot={}",
        crate::arch::mm::mmu_init::PtLedger::is_live_table(ppn),
        crate::arch::mm::mmu_init::PT_LEDGER.is_boot(ppn),
    );

    // ---- 3. incarnation history (forensic rings, newest first) ----
    #[cfg(any(feature = "x86_64", feature = "riscv64"))]
    ring_history(ppn);
    #[cfg(feature = "aarch64")]
    crate::pr_err!("  hist: (no forensic rings on aarch64)");

    // ---- 4. per-CPU pageset rows ----
    let mut rows = [0usize; 8];
    let n = crate::mm::hotpath::forensics_scan_rows(pfn, &mut rows);
    if n > 0 {
        crate::pr_err!(
            "  pcp: pfn sits in CPU row(s) {:?} (still pcp-cached!)",
            &rows[..n]
        );
    } else {
        crate::pr_err!("  pcp: not present in any CPU row");
    }

    // ---- 5. bounded reverse scan of every live tree ----
    #[cfg(any(feature = "x86_64", feature = "riscv64"))]
    {
        let mut hits = [
            TreeHit {
                pid: 0,
                root_ppn: 0,
                level: 0,
                idx0: 0,
                idx1: 0,
            };
            16
        ];
        let (nh, tasks, skipped, budget) =
            crate::arch::mm::mmu_init::forensics_scan_trees(ppn, &mut hits);
        for h in hits.iter().take(nh) {
            let kind = match h.level {
                0 => "ROOT frame",
                1..=3 => "upper-table LINK",
                _ => "LEAF (data mapping)",
            };
            crate::pr_err!(
                "  tree-ref: pid={} root={:#x} level={} idx0={} idx1={} ({})",
                h.pid, h.root_ppn, h.level, h.idx0, h.idx1, kind
            );
        }
        crate::pr_err!(
            "  tree-scan: {} reference(s) over {} tasks ({} buckets skipped, {} entries read)",
            nh, tasks, skipped, budget
        );
    }
    #[cfg(feature = "aarch64")]
    crate::pr_err!("  tree-scan: (not implemented on aarch64)");
    crate::pr_err!("=== MMFORENSICS capture end ===");
}


// ════════════════ taint ═══════════════════════════════
// Kernel Taint Bitmask
//
// Taint flags for tracking kernel state anomalies.
// Used by WARN, BUG, panic, softlockup, and other DEBUG modules.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Global kernel taint bitmask
static TAINT_MASK: AtomicU32 = AtomicU32::new(0);

/// Add a taint flag to the kernel taint bitmask.
///
/// Once set, taint flags are never cleared.
pub fn add_taint(flag: TaintFlags) {
    TAINT_MASK.fetch_or(flag.bits(), Ordering::Release);
}

/// Get the current taint bitmask value.
pub fn get_taints() -> u32 {
    TAINT_MASK.load(Ordering::Acquire)
}

/// Check if a specific taint flag is set.
pub fn tainted(flag: TaintFlags) -> bool {
    (get_taints() & flag.bits()) != 0
}

/// Convert taint bitmask to character string.
///
/// Linux renders the taint as a 16-column line ("Tainted: G        W
///          "): the first column is the proprietary-module flag — 'P'
/// when tainted, 'G' ("good") otherwise — and every other column holds
/// its flag letter when the flag is set, a space when it is not. The
/// letters are Linux's:
///
/// | bit  | letter | flag                 | bit   | letter | flag               |
/// |------|--------|----------------------|-------|--------|--------------------|
/// | 1    | P      | proprietary module   | 256   | A      | ACPI override      |
/// | 2    | F      | forced module        | 512   | W      | warning            |
/// | 4    | S      | unsafe SMP           | 1024  | C      | crappy module      |
/// | 8    | R      | forced rmmod        | 2048  | I      | firmware workaround|
/// | 16   | M      | machine check        | 4096  | O      | out-of-tree module |
/// | 32   | B      | bad page             | 8192  | E      | unsigned module    |
/// | 64   | U      | user                 | 16384 | L      | soft lockup        |
/// | 128  | D      | kernel died          | 32768 | K      | livepatch          |
///
/// The string is written to the provided buffer (must be >= 17 bytes for
/// 16 chars + null).
///
/// # Returns
/// Number of characters written (excluding null terminator).
pub fn taint_string(buf: &mut [u8]) -> usize {
    const TAINT_CHARS: &[u8; 16] = b"PFSRMBUDWACIOELK";

    let mask = get_taints();
    let len = 16.min(buf.len());

    for i in 0..len {
        let bit = 1u32 << i;
        buf[i] = if mask & bit != 0 {
            TAINT_CHARS[i]
        } else if i == 0 {
            b'G' // "good" — Linux's first column
        } else {
            b' '
        };
    }

    len
}

/// Get taint string as a fixed-size array (no allocation needed).
pub fn taint_string_arr() -> [u8; 16] {
    let mut buf = [0u8; 16];
    taint_string(&mut buf);
    buf
}


// ════════════════ allocwatch ══════════════════════════
// Allocator class-lock holder watch (rux:allocwatch (retired debug=allocwatch)).
//
// The OH dark-boot family (2026-11-02 decodes, lb234/235/241/243/244):
// every allocating syscall parks (unix_write_slice_owned -> RawVec
// finish_grow, epoll_create1) because a small_alloc/slab/buddy class lock
// is held by a thread frozen mid-section. The holder sits at IF=0 inside
// lock_irqsave, so its OWN CPU's tick never fires — every tick-side
// observer (PREEMPT-HEAL, claim census) is blind to it; only a cross-CPU
// observer can name it. This instrument is that observer:
//
//   - mm sites stamp (pid, tgid, site, class, jiffies, epoch) at
//     class-lock acquire and clear it at release (slow paths only; the
//     magazine fast path never takes the lock);
//   - scheduler_tick calls tick_check(): a stamp older than 1 s prints
//     the holder's CURRENT task state (parked vs runnable-not-picked vs
//     on-cpu-with-preempt-bits) — the discrimination the sched-vs-mm
//     split needs:
//       (a) holder state RUNNING with clean preempt bits and not current
//           on its cpu  => runnable-but-never-picked — scheduler strand
//           family (the preempt-hunt Face C candidate);
//       (b) holder state SLEEPING/STOPPED while holding => mm lock misuse
//           (sleeping under a spinlock — a kernel bug to fix in mm);
//       (c) holder state RUNNING with phantom preempt bits (0x10000/
//           0x10001 class) on-cpu => the phantom-preempt monopolist —
//           the producer hunt's direct evidence.
//
// Slots are keyed by (site, class) with a monotonic epoch so a stamp is
// cleared only by its own release — a frozen holder on one class lock
// cannot be hidden by traffic on another class mapping to the same slot
// (first version had one slot per site; different classes share a site
// and last-writer-wins erased exactly the frozen holders).
//
// Gated OFF by default (Relaxed atomic loads/stores per slow-path
// acquire when on; a single load when off). Budgeted prints: 200
// reports per boot, re-report per holder at most every 5 s.

pub const SITE_SMALL_ALLOC: u8 = 0;
pub const SITE_SLAB: u8 = 1;
pub const SITE_BUDDY: u8 = 2;
pub const SITE_MAX: usize = 3;

pub const SITE_NAMES: [&str; SITE_MAX] = ["small_alloc", "slab", "buddy"];

// FIXED per-key slot table (fx9fix): slot = SITE_BASE[site] + class.
// The first design scanned a shared 16-slot table for a free/exact slot
// UNDER THE CLASS LOCK — but two DIFFERENT class locks' enters ran that
// scan CONCURRENTLY (the slot table has no lock of its own), racing
// slot choice and mixing pid/epoch/jif fields between stamps. A mixed
// stamp survived its holder's exit (exit matched nothing) and read as
// "held 300ms+" while the meter showed the real sections at 1500
// CYCLES — and the roller then killed healthy boots on the print.
// Dedicated slots make enter/exit lock-free-correct by construction:
// same key is serialized by the class lock itself; different keys never
// share a slot.
const SLOTS: usize = 24; // [0..8) small_alloc, [8..18) slab, [18..19) buddy
const SITE_BASE: [usize; SITE_MAX] = [0, 8, 18];

fn slot_of(site: u8, class: u8) -> usize {
    let base = SITE_BASE[(site as usize).min(SITE_MAX - 1)];
    (base + class as usize).min(SLOTS - 1)
}


impl Holder {
    const fn new() -> Self {
        Holder {
            key: AtomicU32::new(0),
            epoch: AtomicU64::new(0),
            pid: AtomicU32::new(0),
            tgid: AtomicU32::new(0),
            jif: AtomicU64::new(0),
            jif_report: AtomicU64::new(0),
        }
    }
}

static HOLDERS: [Holder; SLOTS] = [const { Holder::new() }; SLOTS];
static EPOCH: AtomicU64 = AtomicU64::new(1);
static REPORTS: AtomicU32 = AtomicU32::new(0);
static LAST_REPORT_JIF: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);

/// Meter arm check for the mm section-duration meters (Relaxed load).
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

fn allocwatch_enabled() -> bool {
    if ARMED.load(Ordering::Relaxed) {
        return true;
    }
    if crate::trace::EVENT_ALLOCWATCH.armed() {
        ARMED.store(true, Ordering::Relaxed);
        return true;
    }
    false
}

/// Stamp the current task as holder of (site, class)'s lock. Runs under
/// the acquired lock with IF=0: straight Relaxed stores, no allocation,
/// no scan beyond the slot table.
fn enter_keyed(site: u8, class: u8) -> u64 {
    let key = site as u32 | ((class as u32) << 8);
    let (pid, tgid) = match crate::sched::hotpath::current() {
        Some(t) => ((*t).pid(), (*t).tgid()),
        None => (0, 0),
    };
    let ep = EPOCH.fetch_add(1, Ordering::Relaxed);
    let h = &HOLDERS[slot_of(site, class)];
    // Publish order: everything before pid; pid last (x86 TSO retires
    // stores in order even under Relaxed) — a reader seeing pid != 0
    // sees a coherent fresh stamp.
    h.jif
        .store(crate::timer::get_jiffies() as u64, Ordering::Relaxed);
    h.key.store(key, Ordering::Relaxed);
    h.epoch.store(ep, Ordering::Relaxed);
    h.tgid.store(tgid, Ordering::Relaxed);
    h.pid.store(pid, Ordering::Relaxed);
    ep
}

/// Clear the stamp (lock released) — only if the slot still carries our
/// epoch (an evicted/overwritten stamp belongs to someone else now).
fn exit_keyed(site: u8, class: u8, ep: u64) {
    let h = &HOLDERS[slot_of(site, class)];
    // Clear only our epoch (same-key serialization makes this exact;
    // the epoch check is belt-and-braces for a torn interleaving on
    // the same slot).
    if h.epoch.load(Ordering::Relaxed) == ep {
        h.pid.store(0, Ordering::Relaxed);
    }
}

/// RAII stamp: `let _aw = self::guard(site, class);` immediately
/// after acquiring the class lock. Drops (and clears) BEFORE the lock
/// guard releases (reverse declaration order) — the stamp slightly
/// overstates the hold, never understates it.
pub fn guard(site: u8, class: u8) -> AwGuard {
    let ep = if allocwatch_enabled() { enter_keyed(site, class) } else { 0 };
    AwGuard { site, class, ep }
}


impl Drop for AwGuard {
    fn drop(&mut self) {
        if self.ep != 0 {
            exit_keyed(self.site, self.class, self.ep);
        }
    }
}

/// Live stamp present for (site, class)?
fn stamp_live(site: u8, class: u8) -> bool {
    let key = site as u32 | ((class as u32) << 8);
    for h in HOLDERS.iter() {
        if h.pid.load(Ordering::Relaxed) != 0 && h.key.load(Ordering::Relaxed) == key {
            return true;
        }
    }
    false
}

// STUCK detection: a class lock's WORD observed locked at every tick for
// >1 s with NO live holder stamp is not a held lock — it is a stuck/
// corrupt word (the wild-write producer's freeze face: waiters spin
// forever on a lock nobody holds, and the preemptible TTAS wait makes
// them cycle through finish_grow forever — the exact OH victim
// signature with NO frozen holder to name).
static SA_STUCK: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static SLAB_STUCK: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
static BUDDY_STUCK: AtomicU64 = AtomicU64::new(0);

fn stuck_report(
    now: u64,
    site: &str,
    class: u8,
    first: u64,
) {
    let last = LAST_REPORT_JIF.load(Ordering::Relaxed);
    if now < last + 500 {
        return;
    }
    LAST_REPORT_JIF.store(now, Ordering::Relaxed);
    let n = REPORTS.fetch_add(1, Ordering::Relaxed);
    if n > 200 {
        return;
    }
    crate::trace::trace_allocwatch(0, (n + 1) as u32, class as u64, now.saturating_sub(first) * 10);
    crate::pr_warn!(
        "ALLOC-STUCK #{} site={} class={} stuck={}ms (lock word held with NO live holder stamp — corrupt/stuck word, wild-write producer face)",
        n,
        site,
        class,
        now.saturating_sub(first) * 10
    );
}

fn tick_stuck_scan(now: u64) {
    let sa = crate::mm::hotpath::audit_class_locks();
    for i in 0..8u32 {
        let s = &SA_STUCK[i as usize];
        if sa & (1 << i) != 0 && !stamp_live(SITE_SMALL_ALLOC, i as u8) {
            let first = s.load(Ordering::Relaxed);
            if first == 0 {
                s.store(now, Ordering::Relaxed);
            } else if now.saturating_sub(first) >= 20 {
                stuck_report(now, "small_alloc", i as u8, first);
                // keep the run (no re-arm) — the word is still stuck;
                // the global throttle paces the re-reports.
            }
        } else {
            s.store(0, Ordering::Relaxed);
        }
    }
    let sl = crate::mm::hotpath::audit_cache_locks();
    for i in 0..10u32 {
        let s = &SLAB_STUCK[i as usize];
        if sl & (1 << i) != 0 && !stamp_live(SITE_SLAB, i as u8) {
            let first = s.load(Ordering::Relaxed);
            if first == 0 {
                s.store(now, Ordering::Relaxed);
            } else if now.saturating_sub(first) >= 20 {
                stuck_report(now, "slab", i as u8, first);
            }
        } else {
            s.store(0, Ordering::Relaxed);
        }
    }
    if crate::mm::hotpath::audit_lock_held() && !stamp_live(SITE_BUDDY, 0) {
        let first = BUDDY_STUCK.load(Ordering::Relaxed);
        if first == 0 {
            BUDDY_STUCK.store(now, Ordering::Relaxed);
        } else if now.saturating_sub(first) >= 20 {
            stuck_report(now, "buddy", 0, first);
        }
    } else {
        BUDDY_STUCK.store(0, Ordering::Relaxed);
    }
}

/// Cross-CPU check from scheduler_tick: report stamps older than 1 s.
/// The holder's own CPU cannot run this (IF=0 froze its tick); the other
/// CPU's tick scans ALL slots regardless of which CPU stamped them.
pub fn tick_check() {
    if !allocwatch_enabled() {
        return;
    }
    let now = crate::timer::get_jiffies() as u64;
    tick_stuck_scan(now);
    // Section-meter heartbeat (every 10 s): names the per-boot cost
    // class even when no hold crosses the report threshold.
    {
        static LAST_SECT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let last = LAST_SECT.load(Ordering::Relaxed);
        if now >= last + 1000
            && LAST_SECT
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            let m = crate::mm::hotpath::audit_section_peek();
            crate::pr_warn!(
                "ALLOC-SECT small_alloc max_cyc={:x} over10ms={}",
                m.iter().map(|x| x.0).max().unwrap_or(0),
                m.iter().map(|x| x.1).sum::<u64>()
            );
        }
    }
    for i in 0..SLOTS {
        let h = &HOLDERS[i];
        let pid = h.pid.load(Ordering::Relaxed);
        if pid == 0 {
            continue;
        }
        let jif = h.jif.load(Ordering::Relaxed);
        if now <= jif || now - jif < 30 {
            continue; // held < 1 s (100 Hz)
        }
        // Re-report throttle: per-SLOT 5 s (the first version throttled
        // globally, so a freeze whose chain holds small_alloc AND buddy
        // printed only the first slot — lb260's onset print named
        // small_alloc and the machine died before the buddy slot's
        // window opened).
        let last = h.jif_report.load(Ordering::Relaxed);
        if last != 0 && now < last + 200 {
            continue;
        }
        h.jif_report.store(now, Ordering::Relaxed);
        let n = REPORTS.fetch_add(1, Ordering::Relaxed);
        if n > 200 {
            return;
        }
        let tgid = h.tgid.load(Ordering::Relaxed);
        let key = h.key.load(Ordering::Relaxed);
        let held_ms = (now - jif) * 10;
        let site = (key & 0xff) as usize;
        let site_name = if site < SITE_MAX { SITE_NAMES[site] } else { "?" };
        let class = key >> 8;
        // Find the holder's CURRENT state. Non-blocking iteration: we run
        // from the tick; a busy pid-hash bucket is skipped (partial info
        // beats a wedged diagnostic).
        let mut found = false;
        let mut state_bits = 0u32;
        let mut preempt = 0i32;
        let mut ti_cpu = 0i32;
        let mut thsp = 0u64;
        let mut comm_out = [0u8; 16];
        let mut comm_len = 0usize;
        {
            crate::process::hotpath::pid_hash_for_each_task_try(|t| {
                if !found && t.pid() == pid {
                    state_bits = t.state().bits();
                    preempt = t.preempt_count();
                    ti_cpu = t.ti_cpu();
                    thsp = t.thread().sp;
                    let c = t.comm();
                    let n = c.iter().position(|&x| x == 0).unwrap_or(16);
                    comm_out[..n].copy_from_slice(&c[..n]);
                    comm_len = n;
                    found = true;
                }
            });
        // (task scan done)
    
        }
        let comm = core::str::from_utf8(&comm_out[..comm_len]).unwrap_or("?");
        let cls = if found {
            match state_bits {
                0 => {
                    // RUNNING: on-cpu-with-phantom-bits vs
                    // runnable-not-picked. preempt != 0 while RUNNING is
                    // the monopolist face; clean preempt + not picked is
                    // the strand face. Print both discriminators raw.
                    if preempt != 0 {
                        "RUNNING+preempt-bits(monopolist?)"
                    } else {
                        "RUNNING-preempt-clean(strand?)"
                    }
                }
                1 => "SLEEPING(parked-while-holding: mm lock misuse)",
                2 => "STOPPED-while-holding(mm lock misuse)",
                _ => "other-state",
            }
        } else {
            "not-found(task exited? stamp stale)"
        };
        // Section-duration meter (lb268): one bounded section held
        // 300 ms of real KVM execution — the per-class max/over10ms
        // counters say whether that is normal for this boot.
        if site == SITE_SMALL_ALLOC as usize {
            let m = crate::mm::hotpath::audit_section_peek();
            crate::pr_warn!(
                "ALLOC-SECT small_alloc max_cyc={:x} over10ms={}",
                m.iter().map(|x| x.0).max().unwrap_or(0),
                m.iter().map(|x| x.1).sum::<u64>()
            );
        }
        // Nested-buddy-spin hint (lb263): the small_alloc section's only
        // unbounded tail is the NESTED buddy-lock acquire — a waiter
        // there has NO stamp (stamps happen after acquire), so a stuck
        // buddy word makes the small_alloc holder spin forever with no
        // buddy slot ever appearing. Name the buddy word's live state on
        // every small_alloc/slab hold report.
        let buddy_hint = if site < SITE_MAX {
            if crate::mm::hotpath::audit_lock_held() {
                " buddyword=locked"
            } else {
                " buddyword=clean"
            }
        } else {
            ""
        };
        crate::pr_warn!(
            "ALLOC-HOLD #{} slot={} site={} class={} pid={} tgid={} comm={} held={}ms state={} preempt={:#x} ti_cpu={} verdict={}{}",
            n,
            i,
            site_name,
            class,
            pid,
            tgid,
            comm,
            held_ms,
            if found { state_bits } else { 0xff },
            preempt,
            ti_cpu,
            cls,
            buddy_hint
        );
        // Holder kernel-stack PCs (lb260: the onset print named the
        // holder but not WHERE it is stuck; both cpus died within ~5 s
        // so this is the only chance to see inside). Same window scan
        // as HUNG-STK (hung_task.rs R76): text-range words upward from
        // the saved sp. For a holder LIVE on cpu at IF=0 the sp is its
        // last switch-in sp — outer frames still name the syscall.
        if found && (thsp >> 48) == 0xffff && (thsp & 7) == 0 && thsp > 0x1000 {
            #[cfg(feature = "x86_64")]
            extern "C" {
                static __stext: u8;
                static __etext: u8;
            }
            #[cfg(not(feature = "x86_64"))]
            extern "C" {
                static _stext: u8;
                static _etext: u8;
            }
            #[cfg(feature = "x86_64")]
            let lo = core::ptr::addr_of!(__stext) as u64;
            #[cfg(feature = "x86_64")]
            let hi = core::ptr::addr_of!(__etext) as u64;
            #[cfg(not(feature = "x86_64"))]
            let lo = core::ptr::addr_of!(_stext) as u64;
            #[cfg(not(feature = "x86_64"))]
            let hi = core::ptr::addr_of!(_etext) as u64;
            // Upward scan of the holder's own stack through the
            // bounds-checked window (INV-WINDOW-*); the Task pins the
            // stack pages.
            let win = crate::safety::frame::KernelStackWindow::new(
                thsp as usize,
                (thsp as usize) + 80 * 8,
            );
            let mut shown = 0;
            crate::pr_warn!("ALLOC-HOLD-STK pid={} sp={:#x}", pid, thsp);
            for k in 0..80 {
                let v = match win.and_then(|w| w.read_u64((thsp as usize) + k * 8)) {
                    Some(v) => v,
                    None => break,
                };
                if v >= lo && v < hi && (v & 1) == 0 {
                    crate::pr_warn!("ALLOC-HOLD-STK2 {:#x}", v);
                    shown += 1;
                    if shown >= 8 {
                        break;
                    }
                }
            }
            if shown == 0 {
                crate::pr_warn!("ALLOC-HOLD-STK2 none-in-text");
            }
        }
    }
}


// ════════════════ backtrace ═══════════════════════════
// Stack Trace and Register Dump
//
// Extracted from the inline panic handler in `main.rs`.
// Provides reusable `dump_stack()`, `dump_regs()`, and `dump_csrs()` APIs
// for use by WARN, BUG, softlockup, hung_task, and other DEBUG modules.
//
// All output goes directly to UART via the lockless batched writer
// (bypasses printk ring buffer), making these safe to call from
// crash/panic contexts.

// ============================================================================
// ConsoleWriter — lockless UART output for crash/panic contexts
// ============================================================================


impl ConsoleWriter {
    pub fn new() -> Self {
        ConsoleWriter {
            emit_locked: crate::printk::try_console_emit_lock(),
            buf: [0u8; 128],
            len: 0,
        }
    }

    /// Flush staged bytes through the lockless batched TX path
    /// (putchar_no_lock's batched sibling — safe in crash contexts).
    fn flush(&mut self) {
        if self.len > 0 {
            let uart = crate::console::Uart::new();
            uart.puts_batch(&self.buf[..self.len]);
            self.len = 0;
        }
    }
}

impl Drop for ConsoleWriter {
    fn drop(&mut self) {
        self.flush();
        if self.emit_locked {
            crate::printk::console_emit_unlock();
        }
    }
}

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                // Newline expansion (the OPOST discipline of the old
                // per-byte path) staged as the translated pair.
                if self.len + 2 > self.buf.len() {
                    self.flush();
                }
                self.buf[self.len] = b'\r';
                self.buf[self.len + 1] = b'\n';
                self.len += 2;
            } else {
                if self.len == self.buf.len() {
                    self.flush();
                }
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

// ============================================================================
// Frame Pointer Stack Walk
// ============================================================================

/// Read the current frame pointer register (`s0` on riscv64, `rbp` on x86_64).
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
#[inline]
fn unwind_frame(fp: u64) -> Option<(u64, u64)> {
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

/// Walk frame pointers and call the callback for each frame.
///
/// Reads the current frame pointer via inline asm, then iterates the frame
/// pointer chain: `[fp]` = saved_fp, `[fp+8]` = return_addr.
///
/// # Arguments
/// * `cb` - Callback called as `cb(program_counter, frame_pointer)` for each frame.
///
/// # Safety
/// Reads from memory via frame pointer chain. Caller must ensure the stack
/// is in a valid state (not corrupted). Validates alignment and address range.
pub fn walk_stack_trace(cb: &mut dyn FnMut(u64, u64)) {
    let mut fp: u64 = current_frame_pointer();

    let mut frame_count = 0u32;
    while fp != 0 && frame_count < 32 {
        // Validate fp: must be 8-byte aligned and in kernel address range
        if fp < 0x8000_0000 || fp > 0xFFFF_FFFF_FFFF_FFFF || fp % 8 != 0 {
            break;
        }

        if let Some((ret_addr, next_fp)) = unwind_frame(fp) {
            cb(ret_addr, fp);

            // Check if next fp is valid (should be > current fp or 0)
            if next_fp <= fp {
                break;
            }
            fp = next_fp;
        } else {
            break;
        }
        frame_count += 1;
    }
}

/// Print formatted stack trace to console (bypasses printk ring buffer).
///
/// Output format:
/// ```text
/// Call trace:
///   [<ffffffff80001234>] (current)
///   [<ffffffff80005678>]
///   [<ffffffff80009abc>]
/// ```
pub fn dump_stack() {
    let mut w = ConsoleWriter::new();

    // Print current ra first
    let ra: u64 = caller_return_address();

    // Linux's classic frame format: the address, then the function name
    // with the offset and the function size (Linux 4.x and still what
    // most people read); "?" when the address resolves to no symbol.
    let _ = w.write_str("Call Trace:\n");
    let _ = write!(
        w,
        "  [<{:016x}>] {} (current)\n",
        ra,
        self::format_symbol(ra)
    );

    walk_stack_trace(&mut |pc, _fp| {
        let _ = write!(
            w,
            "  [<{:016x}>] {}\n",
            pc,
            self::format_symbol(pc)
        );
    });

    let _ = w.write_str("\n");
}

// ============================================================================
// Register and CSR Dump
// ============================================================================

/// Register names in order matching `save_regs()` layout
#[cfg(feature = "riscv64")]
const REG_NAMES: [&str; 31] = [
    "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0",
    "s1", "a0", "a1", "a2", "a3", "a4", "a5", "a6",
    "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8",
    "s9", "s10", "s11", "t3", "t4", "t5", "t6",
];

/// x86_64: only the callee-saved GPRs are captured (slots 0..6); the
/// remaining slots stay zero.
#[cfg(feature = "x86_64")]
const REG_NAMES: [&str; 31] = [
    "rbx", "rbp", "r12", "r13", "r14", "r15", "", "",
    "", "", "", "", "", "", "", "",
    "", "", "", "", "", "", "", "",
    "", "", "", "", "", "", "",
];

/// aarch64: save_regs captures x0..x30 in slot order
#[cfg(feature = "aarch64")]
const REG_NAMES: [&str; 31] = [
    "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7",
    "x8", "x9", "x10", "x11", "x12", "x13", "x14", "x15",
    "x16", "x17", "x18", "x19", "x20", "x21", "x22", "x23",
    "x24", "x25", "x26", "x27", "x28", "x29", "x30",
];

/// aarch64: capture x0..x30 into the dump array (x0 at slot 0).
#[cfg(feature = "aarch64")]
pub fn save_regs() -> [u64; 31] {
    let mut regs: [u64; 31] = [0; 31];
    // SAFETY: register stores into a stack buffer; no memory reads.
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
    regs
}

/// Save all integer registers to a stack buffer via inline assembly.
///
/// Returns an array of 31 u64 values representing x1..x31.
#[cfg(feature = "riscv64")]
pub fn save_regs() -> [u64; 31] {
    let mut regs: [u64; 31] = [0; 31];
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
    regs
}

/// x86_64 register capture: spill the callee-saved GPRs (rbx is reserved
/// in LLVM, so push/spill/pop is the reliable way to read it).
#[cfg(feature = "x86_64")]
pub fn save_regs() -> [u64; 31] {
    let mut regs: [u64; 31] = [0; 31];
    // SAFETY: the pushes are balanced by the rsp adjustment; only this
    // stack scratch area is touched.
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
    regs
}

/// Print register dump captured via inline assembly.
///
/// Captures all integer registers (x1..x31) and prints them 4 per line.
pub fn dump_regs_inline() {
    let regs = save_regs();
    dump_regs(&regs);
}

/// Print register dump from a saved array using the global REG_NAMES.
pub fn dump_regs(regs: &[u64; 31]) {
    let mut w = ConsoleWriter::new();

    let _ = w.write_str("Registers:\n");
    for (i, chunk) in regs.chunks(4).enumerate() {
        let _ = w.write_str("  ");
        for (j, val) in chunk.iter().enumerate() {
            let name_idx = i * 4 + j;
            let _ = write!(w, "{:4}: {:016x}  ", REG_NAMES[name_idx], val);
        }
        let _ = w.write_str("\n");
    }
    let _ = w.write_str("\n");
}

/// Read and print CSR state (sstatus, scause, stval, sepc).
#[cfg(feature = "riscv64")]
pub fn dump_csrs() {
    let mut w = ConsoleWriter::new();

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

    let _ = write!(w, "Sstatus: {:016x}\n", sstatus);
    let _ = write!(w, "Scause : {:016x}\n", scause);
    let _ = write!(w, "Stval  : {:016x}\n", stval);
    let _ = write!(w, "Sepc   : {:016x}\n\n", sepc);
}

/// x86_64: print the fault/control registers instead of RISC-V CSRs.
#[cfg(feature = "x86_64")]
pub fn dump_csrs() {
    let mut w = ConsoleWriter::new();
    let _ = write!(w, "CR2    : {:016x}\n", crate::arch::cpu::read_cr2());
    let _ = write!(w, "CR3    : {:016x}\n\n", crate::arch::cpu::read_cr3());
}

/// aarch64: print the exception syndrome registers instead of RISC-V CSRs.
#[cfg(feature = "aarch64")]
pub fn dump_csrs() {
    let mut w = ConsoleWriter::new();
    let (esr, elr, far, spsr): (u64, u64, u64, u64);
    // SAFETY: pure system-register reads, valid in any context.
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
    let _ = write!(w, "ESR    : {:016x}\n", esr);
    let _ = write!(w, "ELR    : {:016x}\n", elr);
    let _ = write!(w, "FAR    : {:016x}\n", far);
    let _ = write!(w, "SPSR   : {:016x}\n\n", spsr);
}


// ════════════════ bug ═════════════════════════════════
// BUG and WARN Macros
//
// BUG/WARN infrastructure for the Rux kernel.
//
// - `WARN()`: Non-fatal warning. Logs message + stack trace + taints kernel.
// - `BUG()`:  Fatal. Logs message + stack trace, then calls `panic!()`.

/// Non-fatal warning.
///
/// Prints a "cut here" warning header, the file:line location,
/// the condition string, CPU/PID info, stack trace, and taint string.
/// Taints the kernel with `TAINT_WARN`.
///
/// Called by the `warn_on!` macro — do not call directly in normal code.
#[track_caller]
pub fn warn(file: &str, line: u32, condition: &str) {
    let mut w = ConsoleWriter::new();

    // "cut here" marker
    let _ = w.write_str("------------[ cut here ]------------\n");

    // WARNING header
    let _ = write!(w, "WARNING: {}:{}\n", file, line);
    let _ = write!(w, "  condition \"{}\" was true\n", condition);

    // CPU/PID info
    let cpu = crate::arch::cpu_id() as u32;
    let pid = crate::sched::hotpath::get_current_pid();
    let _ = write!(w, "CPU: {} PID: {}\n", cpu, pid);

    // Stack trace
    self::dump_stack();

    // Taint string
    let mut taint_buf = [0u8; 16];
    self::taint_string(&mut taint_buf);
    let taint_str = core::str::from_utf8(&taint_buf).unwrap_or("?");
    let _ = write!(w, "Tainted: {}\n", taint_str);

    // End marker
    let _ = w.write_str("---[ end trace 0000000000 ]---\n");

    // Taint the kernel
    self::add_taint(super::types::TaintFlags::WARN);
}

/// Fatal BUG.
///
/// Prints a BUG header with location, then triggers `panic!()`.
/// This is the terminal path — the kernel will not continue.
///
/// Called by the `bug_on!` macro — do not call directly in normal code.
#[track_caller]
pub fn bug_bug(file: &str, line: u32) {
    let mut w = ConsoleWriter::new();

    // "cut here" marker
    let _ = w.write_str("------------[ cut here ]------------\n");

    // BUG header
    let _ = write!(w, "kernel BUG at {}:{}!\n", file, line);

    // CPU/PID info
    let cpu = crate::arch::cpu_id() as u32;
    let pid = crate::sched::hotpath::get_current_pid();
    let _ = write!(w, "CPU: {} PID: {}\n", cpu, pid);

    // Stack trace
    self::dump_stack();

    // Taint the kernel
    self::add_taint(super::types::TaintFlags::DIE);

    // Trigger panic — this never returns
    panic!("BUG: kernel BUG at {}:{}", file, line);
}

/// WARN_ON_ONCE support: per-callsite `AtomicBool` to ensure a warning
/// fires only once per boot.
///
/// # Safety
/// The `flag` must point to a valid `AtomicBool` that outlives the program
/// (typically a `static`).
pub fn warn_on_once_check(flag: &core::sync::atomic::AtomicBool) -> bool {
    // Try to set the flag from false to true.
    // If already true, return false (already warned).
    use core::sync::atomic::Ordering;
    flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
}


// ════════════════ softlockup ══════════════════════════
// Softlockup Detector
//
// Detects CPUs that are stuck in a non-sleeping loop for too long.
// Each CPU updates a per-CPU timestamp on every `scheduler_tick()`.
// A periodic check (driven by timer interrupt) compares timestamps
// against current time and reports any CPU that hasn't scheduled
// for longer than the threshold.

/// Softlockup threshold in seconds (default: 10s)
const SOFTLOCKUP_THRESHOLD_SECS: u64 = 10;

/// Per-CPU timestamp of last scheduler tick (nanoseconds)
static TOUCH_TS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Per-CPU flag indicating softlockup detected (to avoid repeated reports)
static REPORTED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Whether the softlockup detector is enabled
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Get nanosecond timestamp from the arch wall-clock counter.
fn now_ns() -> u64 {
    let time: u64 = crate::arch::cpu::read_time();
    // timebase ticks → ns: for the 10 MHz CLINT that is exactly 100 ns per
    // tick. The old `time * (FREQ / 1_000_000)` computed ×10 instead of
    // ×100 — every elapsed window read 10x too small, delaying a 10s
    // lockup report to ~100s real time (review批次8 换算修复).
    time.saturating_mul(1_000_000_000) / crate::config::TIMER_CLOCK_FREQ_HZ as u64
}

/// Initialize the softlockup detector.
///
/// Called during `debug::init()`. Sets all timestamps to current time.
pub fn softlockup_init() {
    let now = now_ns();
    for i in 0..MAX_CPUS {
        TOUCH_TS[i].store(now, Ordering::Release);
        REPORTED[i].store(false, Ordering::Release);
    }
    ENABLED.store(true, Ordering::Release);
}

/// Touch the softlockup timestamp for the given CPU.
///
/// Called from `scheduler_tick()` to indicate this CPU is still scheduling.
pub fn touch(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    TOUCH_TS[cpu].store(now_ns(), Ordering::Release);
    REPORTED[cpu].store(false, Ordering::Release);
}

/// Last scheduler-tick timestamp (ns) recorded for the given CPU.
///
/// 0 when the CPU has never taken a tick (early boot) — callers must treat
/// that as "unknown", never as "stale". Used by the scheduler's orphan
/// harvester to distinguish a slot owned by a live, ticking CPU from one
/// left behind by a hart that stopped taking interrupts entirely.
pub fn last_touch_ns(cpu: usize) -> u64 {
    if cpu >= MAX_CPUS {
        return 0;
    }
    TOUCH_TS[cpu].load(Ordering::Acquire)
}

/// R57 (vCPU-liveness under MTTCG starvation): last observed timer-IRQ
/// count per CPU, and the CLINT ns timestamp at which it last CHANGED.
/// A TCG-starved vCPU still services its timer ISR occasionally (3-10
/// IRQs/s under host contention vs 100 nominal — observed on CPU3 of the
/// n1-organic run: timer_irq 32604→33074 across a 1600s window while its
/// scheduler tick read "stale" the whole time). The scheduler's
/// slot-capture steal used tick-staleness alone as its "dead picker"
/// test and stole a mid-exit task from that alive-but-slow hart
/// (double-run, epc on the task's own kernel stack, illegal-instruction
/// panic at t=1796s). A hart whose timer count still moves is alive
/// no matter how stale its touch_ts reads.
static IRQ_COUNT_SEEN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static IRQ_COUNT_SEEN_AT_NS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Record each CPU's current timer-IRQ count; stamp the wall-clock ns of
/// any change. Called from check(), which runs on every timer interrupt —
/// healthy CPUs keep every CPU's "changed at" fresh, so a hart whose count
/// is frozen accumulates real staleness visible to any observer.
fn note_irq_counts(now: u64) {
    for cpu in 0..MAX_CPUS {
        let cnt = crate::fs::hotpath::timer_count(cpu);
        if cnt == 0 {
            // Hart has never taken a timer IRQ (early boot / not started):
            // leave seen-at at 0 — cpu_irq_frozen_ms reports 0 ("alive")
            // for that case so nothing is ever stolen from a never-started
            // hart either.
            continue;
        }
        let prev = IRQ_COUNT_SEEN[cpu].load(Ordering::Relaxed);
        if cnt != prev {
            IRQ_COUNT_SEEN[cpu].store(cnt, Ordering::Relaxed);
            IRQ_COUNT_SEEN_AT_NS[cpu].store(now, Ordering::Release);
        }
    }
}

/// Milliseconds since the named CPU's timer-IRQ count last changed.
/// 0 means "alive or unknown" (the count is currently moving, or the hart
/// has never ticked) — the SAFE answer for any "may I steal from it"
/// test: a wrong 0 merely refuses a heal, a wrong u64::MAX destroys a
/// live task.
pub fn cpu_irq_frozen_ms(cpu: usize) -> u64 {
    if cpu >= MAX_CPUS {
        return 0;
    }
    let seen_at = IRQ_COUNT_SEEN_AT_NS[cpu].load(Ordering::Acquire);
    if seen_at == 0 {
        return 0;
    }
    let now = now_ns();
    if now <= seen_at {
        return 0;
    }
    (now - seen_at) / 1_000_000
}

/// Check all CPUs for softlockup.
///
/// Called periodically from timer interrupt handler.
/// Reports any CPU that hasn't been touched for longer than the threshold.
pub fn check() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }

    let now = now_ns();
    let threshold_ns = SOFTLOCKUP_THRESHOLD_SECS * 1_000_000_000;

    // R57: refresh the timer-count liveness table before any consumer
    // (the scheduler's slot-capture steal) reads it.
    note_irq_counts(now);

    for cpu in 0..MAX_CPUS {
        // Skip CPUs that haven't received any timer interrupt yet.
        // During boot, secondary CPUs may not have started their timers,
        // so TOUCH_TS remains at the init() value — not a real lockup.
        let timer_cnt = crate::fs::hotpath::timer_count(cpu);
        if timer_cnt == 0 {
            continue;
        }

        // Skip CPUs that are idle with no runnable tasks on the system.
        // After init exits, all CPUs enter idle loop with no work to
        // do — timer ticks stop and timestamps go stale, but this is
        // not a lockup.
        let nr_running = crate::sched::types::GlobalRunQueue::grq_nr_running();
        if nr_running == 0 {
            continue;
        }

        // Skip CPUs running the idle task (PID 0).  The idle task
        // intentionally never schedules — it only runs when there is
        // nothing else to do.  A stale touch_ts on an idle CPU is
        // expected behaviour, not a lockup.
        {
            // The pick slot's task is another CPU's CURRENT pick — a
            // possibly-stale handle read through the forensics edge.
            let current = crate::sched::hotpath::cpu_state(cpu).current;
            if current.is_null() || crate::debug::r#unsafe::t_pid(current) == 0 {
                continue;
            }
        }

        let touch_ts = TOUCH_TS[cpu].load(Ordering::Acquire);

        // Skip CPUs that haven't been initialized (touch_ts == 0)
        if touch_ts == 0 {
            continue;
        }

        let elapsed = now.saturating_sub(touch_ts);

        if elapsed > threshold_ns {
            // Only report once per occurrence
            if REPORTED[cpu].compare_exchange(
                false, true,
                Ordering::AcqRel, Ordering::Acquire
            ).is_err() {
                continue;
            }

            let elapsed_secs = elapsed / 1_000_000_000;
            // The lockup report body lives in the debug crate
            // (crate-flip rung debug): holder line, leakorigin
            // origins, stack trace, remote backtrace, taint — one row
            // call per CPU per occurrence (the REPORTED CAS above is
            // the once-gate).
            softlockup_report(cpu, elapsed_secs, timer_cnt as u64);
        }
    }
}

/// The one-shot lockup report (moved to the debug crate): same-named
/// wrapper dispatched from check() after the REPORTED gate wins.
pub fn softlockup_report(cpu: usize, elapsed_secs: u64, timer_cnt: u64) {
    crate::api::debug::softlockup_report(cpu, elapsed_secs, timer_cnt);
}


// ════════════════ hung_task ═══════════════════════════
// Hung Task Detector
//
// Detects tasks stuck in `TASK_UNINTERRUPTIBLE` state for too long.
// A kernel thread (`khungtaskd`) periodically scans all tasks and reports
// any that have been in D-state with unchanged context switch count
// for longer than the threshold.

/// Hung task timeout in seconds (default: 120s)
const HUNG_TASK_TIMEOUT_SECS: u64 = 120;

/// Check interval (timeout / 2)
const HUNG_TASK_CHECK_INTERVAL_SECS: u64 = HUNG_TASK_TIMEOUT_SECS / 2;

/// Whether khungtaskd is running
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Get nanosecond timestamp from RISC-V `rdtime`.
fn hung_task_now_ns() -> u64 {
    let time: u64 = crate::arch::cpu::read_time();
    // timebase ticks → ns (×100 for the 10 MHz CLINT; the old ×10
    // multiplier read every window 10x too small — review批次8).
    time.saturating_mul(1_000_000_000) / crate::config::TIMER_CLOCK_FREQ_HZ as u64
}

/// Initialize the hung task detector.
///
/// Starts the `khungtaskd` kernel thread.
pub fn hung_task_init() {
    if RUNNING.load(Ordering::Acquire) {
        return;
    }

    // Start khungtaskd kernel thread
    let result = crate::api::process::kthread_run(
        khungtaskd_fn,
        core::ptr::null_mut(),
        "khungtaskd",
    );

    if result.is_some() {
        RUNNING.store(true, Ordering::Release);
        } else {
        crate::pr_warn!("debug: failed to start khungtaskd");
    }
}

/// khungtaskd main loop.
///
/// Wakes every `HUNG_TASK_CHECK_INTERVAL_SECS` via a one-shot kernel timer
/// and scans all tasks (pid-hash traversal — never the pid-slot array).
extern "C" fn khungtaskd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::types::TaskState;

    // Check interval in jiffies (timeout/2, at least one tick).
    let interval_jiffies = (HUNG_TASK_CHECK_INTERVAL_SECS
        * crate::config::KERNEL_HZ as u64)
        .max(1);

    loop {
        // Check for stop request
        if crate::api::process::kthread_should_stop() {
            break;
        }

        // --- Periodic sleep with a real waker (review批次8: the old loop
        // set INTERRUPTIBLE and scheduled with NO waker armed — khungtaskd
        // slept until some unrelated wake happened to land on it, so hung
        // tasks were detected minutes late or never). Mirror the
        // nanosleep_impl state-first discipline: arm a one-shot timer for
        // our pid, mark INTERRUPTIBLE, re-check, then sleep.
        let my_pid = match crate::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => break,
        };
        let target = crate::timer::get_jiffies() + interval_jiffies;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);

        if timer_id != 0 {
            // Re-derived running-task borrows (never held across
            // schedule() below — the discipline that replaces the raw
            // pointer this loop carried).
            crate::sched::hotpath::current().map(|t| t.set_state(TaskState::new(TaskState::INTERRUPTIBLE)));
            // Re-check AFTER marking sleeping: the one-shot timer may have
            // fired between arming and set_state — either its wake captured
            // the INTERRUPTIBLE state, or the deadline is already visible.
            if crate::timer::get_jiffies() >= target {
                if let Some(t) = crate::sched::hotpath::current() {
                    t.set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while we
                    // are still executing — take ourselves back off.
                    crate::sched::hotpath::dequeue_task(t);
                }
                crate::timer::del_timer(timer_id);
            } else {
                // Enable interrupts so the tick can reach us, then sleep.
                crate::arch::cpu::restore_irq(true);
                crate::sched::hotpath::schedule();
                // Spurious wake before the deadline: drop the timer and
                // loop (re-armed next iteration).
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full — stay runnable, yield one scheduling round
            // and retry the arm.
            crate::sched::hotpath::schedule();
        }

        // Scan all tasks
        check_tasks();

        // Binder stall scan: threads parked in a blocking binder read are
        // S-state (invisible above); they mark an IPC chain that stopped
        // answering — the common OH bring-up stall shape.
        crate::api::ipc::stall_report();

        // rux:sleepsnap (retired debug=sleepsnap): name the syscall every other S-state thread is
        // parked in (appspawn-server wedge hunt, b11) — self-gated.
        crate::api::ipc::sleep_snapshot();

        // rux:socksnap (retired debug=socksnap): AF_UNIX listener queue depths + every epoll
        // interest set (appspawn-server wedge hunt, b15) — self-gated.
        crate::api::net::unix_listener_snapshot();
        crate::syscall::support::epoll_interest_snapshot();
    }

    RUNNING.store(false, Ordering::Release);
    0
}

/// Scan all tasks for hung tasks (pid-hash traversal; bookkeeping lives on
/// each Task — the old pid-indexed side table skipped every task whose pid
/// was >= MAX_TASKS, which on this system means essentially all of them,
/// review批次8).
fn check_tasks() {
    let now = hung_task_now_ns();
    let timeout_ns = HUNG_TASK_TIMEOUT_SECS * 1_000_000_000;

    crate::sched::hotpath::for_each_task(|task| {
        // The walk hands &Task under the bucket lock; reads and atomic
        // stores on the task's own hung_task_since field only.
        {
            let task_ref = task;
            let state = task_ref.state();
            let pid = task_ref.pid();

            // Only check uninterruptible tasks
            if !state.contains(crate::process::types::TaskState::UNINTERRUPTIBLE) {
                // Not in D-state: reset tracking
                task_ref.hung_task_since.store(0, Ordering::Release);
                return;
            }

            // Skip the idle task (pid 0).
            if pid == 0 {
                return;
            }

            let last_time = task_ref.hung_task_since.load(Ordering::Acquire);

            if last_time == 0 {
                // First time seeing this task in D-state
                task_ref.hung_task_since.store(now, Ordering::Release);
            } else {
                let elapsed = now.saturating_sub(last_time);
                if elapsed > timeout_ns {
                    // Hung task detected!
                    report_hung_task(task, elapsed / 1_000_000_000);

                    // Reset to avoid repeated reports
                    task_ref.hung_task_since.store(now, Ordering::Release);
                }
            }
        }
    });
}

/// Report a hung task.
fn report_hung_task(task_ref: &crate::process::types::Task, elapsed_secs: u64) {
    let mut w = ConsoleWriter::new();

    // rwlock-livelock: comm was a pid placeholder — the ub2404 wedge left
    // an anonymous D-state task (312) whose identity decided the whole
    // anatomy. comm() exists (the W2 refusal print uses it).
    let comm_b = task_ref.comm();
    let comm_end = comm_b.iter().position(|&b| b == 0).unwrap_or(16);
    let comm = core::str::from_utf8(&comm_b[..comm_end]).unwrap_or("?");
    let _ = write!(
        w,
        "INFO: task {}:{} blocked for more than {} seconds\n",
        task_ref.pid(),
        comm,
        elapsed_secs
    );

    // R76 (fast-pace wedge hunt): the old report printed only CURRENT's
    // backtrace — the khungtaskd scanner itself — which says nothing about
    // where the D-state task is parked or which wake it lost. Dump the
    // victim's scheduler-side context (ownership word, cpu, queue flag,
    // vfork linkage) and up to 8 kernel-text words from its parked stack;
    // symbolized offline these name the wait site directly.
    {
        use crate::process::types::Task;
        let t: &Task = task_ref;
        let claim = t.running_on_cpu.load(Ordering::Acquire);
        // SAFETY (one adopt edge): the vfork parent tree handle under the
        // caller's softirq context — TaskBorrow's short-read contract.
        let vfork_ppid = t
            .vfork_parent_ptr()
            .and_then(|p| unsafe { crate::process::types::TaskBorrow::from_raw(p as *mut crate::process::types::Task) })
            .map(|p| p.pid())
            .unwrap_or(0);
        let _ = write!(
            w,
            "      HUNG-CTX pid={} state={:#x} oncpu={} claim={} pk={} ti_cpu={} onrq={} vfork_ppid={} lastsys={} a0={:#x}",
            t.pid(),
            t.state().bits(),
            t.on_cpu() as u8,
            claim,
            t.pick_state(),
            t.ti_cpu(),
            t.sched_entity().is_on_rq() as u8,
            vfork_ppid,
            t.ext.last_syscall_nr.load(Ordering::Relaxed),
            t.ext.last_syscall_a0.load(Ordering::Relaxed),
        );
        // rwlock-livelock: the parked task's user entry frame — syscall
        // nr + user rip decide "kernel thread" (no user frame) vs
        // "userspace task blocked in syscall X at library Y". The old
        // print left task 312 anonymous; lastsys/a0 stayed 0.
        if let Some(pr) = t.pt_regs_ref() {
            #[cfg(feature = "x86_64")]
            let (urip, _usp, unr, _ua0, _ua1, _ua2) =
                (pr.rip, pr.rsp, pr.orig_rax, pr.rdi, pr.rsi, pr.rdx);
            #[cfg(feature = "riscv64")]
            let (urip, _usp, unr, _ua0, _ua1, _ua2) =
                (pr.epc, pr.sp, pr.a7, pr.orig_a0, pr.a1, pr.a2);
            #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
            let (urip, unr) = (0u64, 0u64);
            let _ = write!(w, " unrip={:#x} unrsys={}", urip, unr as i64);
        }
        let _ = writeln!(w);
        let thsp = t.thread().sp;
        if (thsp >> 48) == 0xffff && (thsp & 7) == 0 && thsp > 0x1000 {
            let _ = write!(w, "      HUNG-STK sp={:#x}", thsp);
            // R76: scan UPWARD from the parked sp through the
            // bounds-checked window (INV-WINDOW-*); the Task pins the
            // stack pages.
            let win = t.kernel_stack_top_checked().and_then(|top| {
                crate::safety::frame::KernelStackWindow::new(
                    top - crate::config::KERNEL_STACK_SIZE,
                    top,
                )
            });
            let mut shown = 0;
            // R76: scan UPWARD from the parked sp — the caller frames of
            // the parked chain live at HIGHER addresses (stacks grow down);
            // the words below sp are the context-switch machinery, which is
            // byte-identical for every parked task and names nothing.
            // Window = [text start, text end): the old hardcoded cap
            // 0xffffffff80200000 sat entirely BELOW the x86_64 kernel text
            // (0xffffffff80200000..0xffffffff80690000), so no victim frame
            // ever matched and every report showed only the scanner's own
            // backtrace. The two arch linker scripts spell the bounds
            // differently (x86_64: __stext/__etext, riscv64: _stext/_etext).
            #[cfg(feature = "x86_64")]
            extern "C" {
                static __stext: u8;
                static __etext: u8;
            }
            #[cfg(not(feature = "x86_64"))]
            extern "C" {
                static _stext: u8;
                static _etext: u8;
            }
            #[cfg(feature = "x86_64")]
            let lo = core::ptr::addr_of!(__stext) as u64;
            #[cfg(feature = "x86_64")]
            let hi = core::ptr::addr_of!(__etext) as u64;
            #[cfg(not(feature = "x86_64"))]
            let lo = core::ptr::addr_of!(_stext) as u64;
            #[cfg(not(feature = "x86_64"))]
            let hi = core::ptr::addr_of!(_etext) as u64;
            for k in 0..200 {
                let v = match win.and_then(|w2| w2.read_u64(thsp as usize + k * 8)) {
                    Some(v) => v,
                    None => break,
                };
                if v >= lo && v < hi && (v & 1) == 0 {
                    let _ = write!(w, " {:#x}", v);
                    shown += 1;
                    if shown >= 8 {
                        break;
                    }
                }
            }
            let _ = write!(w, "\n");
        }
    }

    let taint_str = self::taint_string_arr();
    let taint_display = core::str::from_utf8(&taint_str).unwrap_or("?");
    let _ = write!(w, "      Tainted: {}\n", taint_display);

    // Stack trace
    self::dump_stack();

    // Taint kernel
    self::add_taint(super::types::TaintFlags::DIE);
}


// ════════════════ switches ════════════════════════════
// Runtime-switchable DEBUG features.
//
// Compile-time features (`debug-lock-owner` in Cargo.toml) gate the
// hottest hooks — the spinlock owner field costs two atomic stores per
// lock/unlock pair and is therefore a build-time opt-in used by the
// wedge-hunting harness (test/hunt-wedge.sh).
//
// Switch catalog (see the RETIRED TOKENS note below for the ones
// that already moved to the unified trace framework):
//
// RETIRED TOKENS (debug-migrate rounds 4/4b/5, 2026-11): the hunt
// families and debug aids that lived here — rounds 4/4b:
// `schedtrace`, `fuxstuck`, `txnhunt`, `mmforensics`, `sysring`,
// `ohhunt`, `sigtrace`, `futextrace`, `epolltrace`, `taskdump`,
// `periodic`, `watchdog`, `memwatch`; round 5: `schb`, `parkdump`,
// `sleepsnap`, `ksprobe`, `wwcensus`, `bindertrace`,
// `syscalltrace`, `inputtrace`, `exthunt`, `sfddbg`, `dbtrace`,
// `datatap`, `ams46`, `vmap`, `poolcensus`, `slowtask`, `execcost`,
// `tfdcensus`, `scmtap`, `sysforensic`, `filerd`, `lostpark`,
// `allocwatch`, `r83scan`, `leakhunt`, `acqdiv`, `metawatch`,
// `scribble`, `monowatch`, `epollscan`, `treehunt`, `tcptrace`,
// `strace1`, `ripsample`, `socksnap`, `writetrace`, `ledger`,
// `pid1spin`, `steptrace` — moved into the unified trace framework
// (`kernel/core/src/trace/`): each family's gate is now a `rux:` trace
// event, its serial prints are echo consumers of that event's
// enable. Old `debug=` tokens for migrated families are GONE (user
// directive 2026-11-17: no compatibility aliases — use trace_event=).
// (`thpark`, `syncguard`, `syncwatch`, `pctlmon`) name instruments
// whose probe bodies no longer exist — see `dead_dfx_tokens`.
//
// FULLY DELETED (§7 retirement round, 2026-11-23): two of the
// migrated families' hunts CLOSED, so their instruments AND their
// trace events left the tree entirely — `r83scan` (the R74/R83
// task-header hunt; fix c1539f6ed) and `treehunt` (the E8-REPAR
// unlinked-snapshot hunt; fixes 7d4481320/6f5e397ad). Arming those
// two names now draws the unknown-event path, honestly. The
// adjudication record with exit criteria for the remaining KEEP
// families lives in docs/development/debug-retirement-review.md.
//
// What survives here is deliberate, not leftover:
//
// - BEHAVIOR INJECTION (`claiminject`, `oncpuinject`): these CHANGE
//   kernel scheduling/rejection behavior to reproduce wedge shapes —
//   they are fault injectors, not observability, and have no
//   tracepoint form.
// - GDB-PARK ACTIONS (`scribblepark`, `scribblepid1`,
//   `scribblesweep`): they PARK a CPU at a chosen moment so an
//   external gdb session can arm watchpoints — an action taken on
//   the machine, not an observation of it. They arm the `rux:scribble`
//   detection engine as a side effect.
// - ACTIVE DETECTOR (`wildwatch`): its detection mechanism IS state
//   mutation — arming write-protects kernel page-table regions and
//   sets CR0.WP. The #PF hit dump rides the existing kernel fault
//   printer; arming must stay a boot-time switch.
//
// Per the observability policy the table must not grow — new
// observability goes through tracepoints.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// The canonical switch table: enum variant → `debug=` cmdline spelling.
///
/// This macro is the single source of truth for the catalog. It generates
/// the enum→name mapping, the `ALL` enumeration used by the userspace
/// control surface (the debugfs knobs under `/sys/kernel/debug/`,
/// see `debug/mod.rs`) and the per-switch control files. Adding a switch
/// therefore means adding one line here plus the variant — nothing else
/// has to be kept in sync.
macro_rules! debug_switch_table {
    ($m:ident) => {
        $m! {
            ClaimInject => "claiminject",
            OncpuInject => "oncpuinject",
            ScribblePark => "scribblepark",
            ScribblePid1 => "scribblepid1",
            ScribbleSweep => "scribblesweep",
            WildWatch => "wildwatch",
        }
    };
}

pub(crate) use debug_switch_table;


const SWITCH_COUNT: usize = 6;

static SWITCHES: [AtomicBool; SWITCH_COUNT] = [const { AtomicBool::new(false) }; SWITCH_COUNT];

macro_rules! build_all {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// Every DEBUG switch, in the canonical order of the switch table.
        ///
        /// The order matches the `index()` mapping below, so
        /// `ALL[i].index() == i` holds for every entry.
        pub const ALL: [DfxSwitch; SWITCH_COUNT] = [$(DfxSwitch::$variant),*];
    };
}

debug_switch_table!(build_all);

const _: () = assert!(ALL.len() == SWITCH_COUNT);

macro_rules! build_name {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// The `debug=` cmdline spelling of this switch. Also the file name
        /// of its control file under `/sys/kernel/debug/`.
        pub fn name(self) -> &'static str {
            match self {
                $(DfxSwitch::$variant => $name),*
            }
        }
    };
}

impl DfxSwitch {
    fn index(self) -> usize {
        match self {
            // Renumbering note (debug-migrate round 5): the 43 rows that
            // moved to the unified trace framework (39 families + the
            // 4 dead tokens thpark/syncguard/syncwatch/pctlmon whose
            // probe bodies no longer exist) left the table, so the six
            // permanent switches (injection + gdb-park + the PTE-mutating
            // wildwatch) compact to 0..5 in table order. Cmdline lookup
            // is by name, so the shift is invisible to users; indices
            // only key the SWITCHES array.
            DfxSwitch::ClaimInject => 0,
            DfxSwitch::OncpuInject => 1,
            DfxSwitch::ScribblePark => 2,
            DfxSwitch::ScribblePid1 => 3,
            DfxSwitch::ScribbleSweep => 4,
            DfxSwitch::WildWatch => 5,
        }
    }

    debug_switch_table!(build_name);

    fn from_name(name: &str) -> Option<DfxSwitch> {
        ALL.iter().find(|s| s.name() == name).copied()
    }
}

/// Whether a runtime switch is enabled.
pub fn enabled(switch: DfxSwitch) -> bool {
    SWITCHES[switch.index()].load(Ordering::Relaxed)
}

/// OH hunt-print master gate (110th): the always-on hunt probes that
/// rode every perf boot — mtprobe / bindwalk / bindrec / sbxbind /
/// devopen-probe / mntslow / appclone / the drm ioctl census — printed
/// ~1.7k lines per boot at LOGLEVEL=4, and every line costs TWICE (the
/// busy-poll UART emit charged to the printing task, plus hilogd's
/// /dev/kmsg drain). They had only the `sprobe=0` silencer before, so
/// the default was ON. This gate flips the default: armed ONLY when
/// the unified-framework event `rux:ohhunt` is armed (`trace_event=`,
/// `set_event`, or the retired `rux:ohhunt (token debug=ohhunt)` boot token — see
/// or the legacy `sprobe=1` opt-in. Hunt
/// rigs that still want the prints add one of those tokens.
pub fn oh_hunt_prints_on() -> bool {
    crate::trace::EVENT_OHHUNT.armed()
        || crate::cmdline::get_param("sprobe").map(|v| v == "1").unwrap_or(false)
}

/// The retired debug= tokens — the debug-migrate migration ledger.
///
/// Rounds 4/4b/5 of the debug-trace migration moved the switch families
/// out of the bespoke table and into the unified trace framework: each
/// family's gate is now a `rux:` trace event (armed via `trace_event=`
/// or tracefs `set_event`), the bespoke serial prints became echo
/// consumers of that same enable, and the old `debug=` token survives as
/// a boot ALIAS that arms the events the family used to arm — the
/// retired family's own event FIRST, then the same reference-family
/// events `trace_alias` armed for it before the migration (so an
/// alias-armed boot records the identical timeline a pre-migration
/// `debug=` boot recorded, plus the new family event).
///
/// The switch table above SHRANK with every row that moved here (62 →
/// 49 in round 4, 49 → 6 in round 5); this table is the compatibility
/// tail, not a second switch catalog.
/// Retired debug= tokens whose instruments NO LONGER EXIST — the probe
/// bodies were removed when their hunts closed (debug-migrate round 5
/// found them dead: no call sites anywhere in the tree). The tokens
/// are recognized so old hunt rigs get an honest notice instead of the
/// generic unknown-switch warning, but there is nothing to arm:
///
/// - `thpark` — the user-stack park-dump rode the txn-hunt audit,
///   which the round-4 `rux:txnhang` migration replaced; the token
///   arms the audit timeline via trace events, so it
///   is NOT in this list.
/// - `syncguard` / `syncwatch` — the reference-parity same-proc
///   transaction rejection became UNCONDITIONAL kernel behavior
///   (9349d7459 "the wedge family's door"); the switch no longer
///   gated anything.
/// Force a switch on/off programmatically (used by the debugfs control
/// files under /sys/kernel/debug/).
pub fn set(switch: DfxSwitch, on: bool) {
    SWITCHES[switch.index()].store(on, Ordering::Relaxed);
    // The gdb-park switches park the machine for an external debugger
    // (an ACTION — why they stayed out of the trace framework) but
    // they also need the scribble detection engine live, so arming
    // them arms `rux:scribble` too.
    #[cfg(feature = "x86_64")]
    {
        if let DfxSwitch::ScribblePark = switch {
            self::scribble::PARK.store(on, Ordering::Relaxed);
            if on {
                crate::trace::EVENT_SCRIBBLE
                    .enabled
                    .store(true, Ordering::Relaxed);
            }
        }
        if let DfxSwitch::ScribblePid1 = switch {
            self::scribble::PARK_PID1.store(on, Ordering::Relaxed);
            if on {
                crate::trace::EVENT_SCRIBBLE
                    .enabled
                    .store(true, Ordering::Relaxed);
            }
        }
        if let DfxSwitch::ScribbleSweep = switch {
            self::scribble::PARK_SWEEP.store(on, Ordering::Relaxed);
            if on {
                crate::trace::EVENT_SCRIBBLE
                    .enabled
                    .store(true, Ordering::Relaxed);
            }
        }
    }
    #[cfg(not(feature = "x86_64"))]
    {
        let _ = (switch, on);
    }
}

/// Parse `debug=` from the kernel command line. Unknown names are ignored
/// with a warning rather than failing the boot.
pub fn init_from_cmdline() {
    // sc57: the fuxwatch write-watchpoint target is its own boot param
    // (fuxwatch=0xADDR[,DELAYSEC]) — parsed here so it is live from the
    // first censusd round.
    crate::api::sync::fuxwatch_parse();
    let raw = match crate::cmdline::get_param("debug") {
        Some(v) => v,
        None => return,
    };
    for name in raw.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        match DfxSwitch::from_name(name) {
            Some(s) => {
                set(s, true);
                crate::pr_info!("debug: runtime switch '{}' enabled", name);
            }
            None => {
                crate::pr_warn!(
                    "debug: unknown switch '{}' (use trace_event= instead)",
                    name
                );
            }
        }
    }
}


// ════════════════ ksyms ═══════════════════════════════
// Runtime kernel symbol table — the backing store for Linux's
// `/proc/kallsyms` and for symbolized stack traces.
//
// The table is produced at BUILD time: `build/gen-ksyms.py` runs `nm`
// over the linked kernel ELF, keeps the text symbols and resolves their
// demangled names from the DWARF info (`addr2line`), then writes
// `ksyms_generated.rs`.
//
// Two properties make that work:
//
// - Only TEXT symbols go into the table, so the table itself lives in
//   `.rodata` and the `.text` addresses it records stay identical
//   across the two-pass build (the table's own size does not move
//   `.text`).
// - The generated file records the target it was built for, so a tree
//   rebuilt for the other architecture cannot symbolize against the
//   wrong addresses — the table is then simply ignored.
//
// The demangled names matter: the kernel is built with Rust's v0
// mangling, so the raw ELF names (`_RNvMs3_NtNtCslPv9FFGBN4x_3rux...`)
// are unreadable. The generator resolves them from the debug info.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

#[path = "ksyms_generated.rs"]
mod generated;

/// The target triple the table was generated for.
pub const TARGET: &str = generated::TARGET;

/// The symbol names, NUL-separated, in index order.
///
/// The names live in the sibling binary file that the build's symbol pass
/// writes; `include_bytes!` embeds them exactly once (a byte-string
/// literal would be emitted twice into .rodata).
pub static NAMES: &[u8] = include_bytes!("ksyms.bin");

/// The symbol index: (name offset into NAMES, address, size, ELF type).
pub static INDEX: &[(u32, u64, u64, u8)] = generated::INDEX;

/// Is the table usable for the running kernel?
///
/// A table generated for the other architecture has addresses that match
/// nothing — worse than no table, because a lookup would land on a
/// neighbouring symbol. The target check rejects it outright.
pub fn table_valid() -> bool {
    if INDEX.is_empty() {
        return false;
    }
    #[cfg(target_arch = "riscv64")]
    const WANT: &str = "riscv64";
    #[cfg(feature = "aarch64")]
    const WANT: &str = "aarch64";
    #[cfg(target_arch = "x86_64")]
    const WANT: &str = "x86_64";
    TARGET.starts_with(WANT)
}

/// The name of the symbol at `index` (the name blob entry, NUL ended).
fn name_at(index: usize) -> &'static str {
    let (off, _addr, _size, _typ) = INDEX[index];
    let start = off as usize;
    let rest = &NAMES[start..];
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    core::str::from_utf8(&rest[..end]).unwrap_or("?")
}

/// The name of the symbol at `index`, verbatim.
///
/// The build's symbol pass stores the full demangled name (e.g.
/// "rux::arch::trap::do_trap" for kernel symbols, "memset" for C/asm
/// ones); nothing is added at print time so /proc/kallsyms and
/// backtraces show the symbol's real name, like Linux.
pub fn symbol_name(index: usize) -> &'static str {
    name_at(index)
}

/// Find the symbol that contains `pc`.
///
/// Returns (name, symbol address, symbol size). `None` when the
/// address is outside every known symbol.
pub fn lookup(pc: u64) -> Option<(&'static str, u64, u64)> {
    if !table_valid() {
        return None;
    }
    // Binary search for the last symbol whose address is <= pc.
    let mut lo = 0usize;
    let mut hi = INDEX.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if INDEX[mid].1 <= pc {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return None;
    }
    let idx = lo - 1;
    let (_off, addr, size, _typ) = INDEX[idx];
    let name = name_at(idx);
    // A known size that ends before the pc means the address is in the
    // gap between two functions (padding or a symbol we do not have).
    if size > 0 && pc >= addr.saturating_add(size) {
        return None;
    }
    Some((name, addr, size))
}

/// Linux-style symbol rendering: `func+0x1a/0x40`.
///
/// Linux prints the function name, the offset into it and the function
/// size, followed by `?` when the address could not be resolved. The
/// name is the stored symbol name verbatim — no prefix.
pub fn format_symbol(pc: u64) -> String {
    match lookup(pc) {
        Some((name, addr, size)) => {
            let off = pc - addr;
            if size > 0 {
                format!("{}+0x{:x}/0x{:x}", name, off, size)
            } else {
                format!("{}+0x{:x}", name, off)
            }
        }
        None => "?".to_string(),
    }
}


// ════════════════ taskdump ════════════════════════════
// Task-state snapshot for wedge diagnosis.
//
// Prints one line per task (pid, state, policy, comm) over the SBI
// console — deliberately NOT printk, because the usual caller is the
// spinlock deadlock watchdog spinning with local IRQs off and possibly
// with locks the printk path itself needs.
//
// Gated by the `rux:watchdog (retired debug=watchdog)` / `rux:taskdump (retired debug=taskdump)` runtime switches (see
// switches.rs); also available on demand through the UART trigger when
// `taskdump` is on.

// ============================================================================
// Line-atomic staging (obs-hygiene round)
// ============================================================================
//
// Every dump byte used to go out through uart_putc_raw with no lock (the
// crash-context discipline), so on SMP8 the OH kmsg flood — printk
// records holding CONSOLE_EMIT_LOCK ~0.4-17 ms per line — spliced dump
// bytes into flood records and vice versa: dump lines arrived mangled or
// "missing" (the dhardware lane's detour; two lanes lost time to it).
// The wake-drop lane fixed its own serial echoes the same way (b80f76a55,
// HuntLine: stage the full line, one write).
//
// Discipline now: every byte STAGES into a PER-CPU buffer under an
// irqsave stage lock; a '\n' (or a full buffer — long lines continue as
// whole chunks) copies the line out, DROPS the stage guard, and flushes
// the bytes in ONE batched TX pass under the printk console-emit lock,
// so a dump line and a printk record can never splice mid-line and two
// CPUs' dump lines can never splice with each other. The guard is
// dropped before the emit-lock wait so the wait never extends an
// IRQ-off window (the r2 boot's rcu-gp-frozen-cpu lesson). On
// emit-lock timeout (wedged holder) the line still goes out locklessly
// — readable at any cost beats atomic in a wedge.
//
// The stage is PER-CPU, not one shared buffer: a shared stage locked
// per-BYTE (the first cut of this round) still byte-mixed two
// CONCURRENT dumpers — the OH smoke boot caught a periodic dump and a
// rcu-gp-frozen-cpu dump interleaving inside the stage ("linke|dE=0N
// Q..." — both writers' bytes in one flushed line). Per-CPU stages
// make each dumper's line assembly private; flushes remain line-atomic
// per CPU. Same-CPU nested dumps (a dump interrupted by another dump's
// trigger on this CPU) can still mix within the one CPU's stage — that
// class predates this round and is accepted (the alternative, a
// whole-dump mutex, cannot be honored from the trap/NMI contexts the
// dumps exist for).
//
// All taskdump-family writers funnel here (putc/puts/put_hex/put_dec,
// taskdump_raw_line, taskdump_dec): hung-task FTX stacks, WQLEAK,
// memwatch census, small_alloc census lines, schb, the trap printers —
// no call site changes, only the write discipline changed.


static DUMP_STAGES: [crate::sync::types::Spinlock<DumpStage>; crate::config::MAX_CPUS] =
    [const {
        crate::sync::types::Spinlock::new(DumpStage { buf: [0; 512], n: 0 })
    }; crate::config::MAX_CPUS];

/// ONE atomic console burst for pre-staged bytes: bounded acquire of
/// printk's console-emit lock (100 µs — `try_console_emit_lock`'s
/// crash-context budget; the callers — evdev hunt echoes — run from
/// softirq/irq contexts where a longer IRQ-off wait is not
/// acceptable), one `puts_batch` pass, release. On timeout the same
/// bytes go out locklessly (the wedge fallback).
///
/// pstore visibility is unchanged: `puts_batch` sinks every byte to the
/// persistent record exactly like the per-byte `uart_putc_raw` did.
pub fn line_atomic_write(bytes: &[u8]) {
    flush_stage(bytes, EmitBudget::Brief);
}

fn putc(c: u8) {
    staged_putc(c, EmitBudget::Full)
}

/// The brief-budget variant for sched-path hunt prints that run with
/// the GRQ (or another scheduler-critical lock) HELD: same per-CPU
/// staging, but the flush acquires the console-emit lock with
/// `try_console_emit_lock`'s 100 µs crash-context budget — the GRQ hold
/// time must never grow to the dump funnel's 50 ms wait class. When the
/// flood holds the emit lock longer than the budget the flush falls
/// back to the staged lockless burst (still one whole line at the
/// UART, vs the per-byte char-by-char interleave the raw writes had).
///
/// Side effect of routing through the funnel: these prints stay
/// VISIBLE in release builds (putchar_no_lock compiles to a no-op
/// there — the 119th-loglevel lesson; the taskdump funnel is raw on
/// purpose). Every routed printer is boot-budgeted, so the extra
/// release-build output is bounded.
pub fn putc_brief(c: u8) {
    staged_putc(c, EmitBudget::Brief)
}


fn staged_putc(c: u8, budget: EmitBudget) {
    // cpu_id is clamped defensively: trap printers can run before the
    // per-CPU publish on bringup (R87's window) — a wrong-but-in-range
    // stage is harmless (still per-CPU-private for every published CPU).
    let cpu = (crate::arch::cpu_id() as usize) % crate::config::MAX_CPUS;
    // Stage the byte under the irqsave guard (a short critical section),
    // COPY the completed line out, and only then drop the guard and
    // flush. The emit-lock wait (up to 50 ms on a flooded console) must
    // never run inside the IRQ-off window: the r2 OH smoke boot tripped
    // rcu-gp-frozen-cpu exactly once per boot with the wait under the
    // guard — a dump flush spinning IRQs-off for the flood's record
    // time looks like a frozen CPU to the GP detector (and blows the
    // closed-cli-segment guard class). With the guard dropped first,
    // the wait runs with the CALLER's interrupt state (dumps: on).
    let mut line = [0u8; 512];
    let llen;
    {
        let mut st = DUMP_STAGES[cpu].lock_irqsave();
        let at = st.n;
        st.buf[at] = c;
        st.n = at + 1;
        if c == b'\n' || st.n == st.buf.len() {
            line[..st.n].copy_from_slice(&st.buf[..st.n]);
            llen = st.n;
            st.n = 0;
        } else {
            return;
        }
    }
    flush_stage(&line[..llen], budget);
}

/// ONE atomic console burst for pre-staged bytes; the emit-lock budget
/// is caller-chosen (see `EmitBudget`). On timeout the same bytes go
/// out locklessly (the wedge fallback — a torn line beats a missing
/// one in exactly the contexts these prints exist for).
fn flush_stage(bytes: &[u8], budget: EmitBudget) {
    if bytes.is_empty() {
        return;
    }
    let uart = crate::console::Uart::new();
    let locked = match budget {
        EmitBudget::Full => crate::printk::try_console_emit_lock_ms(50),
        EmitBudget::Brief => crate::printk::try_console_emit_lock(),
    };
    if locked {
        uart.puts_batch(bytes);
        crate::printk::console_emit_unlock();
    } else {
        uart.puts_batch(bytes);
    }
}

fn puts(s: &str) {
    for &b in s.as_bytes() {
        putc(b);
    }
}

fn put_hex(v: u64) {
    let mut shift = 64;
    while shift > 0 {
        shift -= 4;
        let nibble = (v >> shift) & 0xF;
        let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
        putc(c);
    }
}

fn put_dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    loop {
        buf[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        putc(buf[n]);
    }
}

/// DEBUG raw diagnostic line for other subsystems' one-shot probes — same
/// discipline as the dump itself (line-atomic staging, see `putc`).
pub fn taskdump_raw_line(s: &[u8]) {
    for &b in s {
        putc(b);
    }
}

/// DEBUG decimal helper matching taskdump_raw_line.
pub fn taskdump_dec(v: u64) {
    put_dec(v);
}

/// DEBUG: replay the forensic syscall ring for one pid (last 48 entries).
pub fn dump_syscall_ring_for(pid: u32) {    use crate::syscall::dispatch::{SYSCALL_CURSOR, SYSCALL_RING};
    let cur = SYSCALL_CURSOR.load(core::sync::atomic::Ordering::Relaxed);
    let len = SYSCALL_RING.len();
    taskdump_raw_line(b"FTX-RING pid=");
    taskdump_dec(pid as u64);
    taskdump_raw_line(b"\n");
    // Walk backwards from the oldest surviving entry to the newest.
    let start = if cur > len { cur - len } else { 0 };
    let mut shown = 0;
    for i in start..cur {
        let e = &SYSCALL_RING[i % len];
        let (p, nr, a0, a1, ret) = (
            e.pid.load(core::sync::atomic::Ordering::Relaxed),
            e.nr.load(core::sync::atomic::Ordering::Relaxed),
            e.a0.load(core::sync::atomic::Ordering::Relaxed),
            e.a1.load(core::sync::atomic::Ordering::Relaxed),
            e.ret.load(core::sync::atomic::Ordering::Relaxed),
        );
        if p == 0 || p != pid {
            continue;
        }
        taskdump_raw_line(b"FTX-SYS nr=");
        taskdump_dec(nr as u64);
        taskdump_raw_line(b" a0=");
        taskdump_dec(a0);
        taskdump_raw_line(b" ret=");
        taskdump_dec(ret as i64 as u64);
        taskdump_raw_line(b"\n");
        shown += 1;
        if shown >= 48 {
            break;
        }
    }
    if shown == 0 {
        taskdump_raw_line(b"FTX-RING-EMPTY\n");
    }
}

/// DEBUG: read one user u32 through a task's page tables (diagnostic).
#[cfg(feature = "debug-futex-trace")]
fn debug_task_read_u32(t: &crate::process::types::Task, va: u64) -> Option<u32> {
    let as_arc = t.address_space_arc()?;
    self::walk_read::<u32>(as_arc.root_ppn(), va)
}

/// DEBUG: one-line periodic snapshot of a tracked user word.
#[cfg(feature = "debug-futex-trace")]
fn debug_track_word(t: &crate::process::types::Task, va: u64) {
    match debug_task_read_u32(t, va) {
        Some(v) => {
            taskdump_raw_line(b"FTX-TRACK u=");
            taskdump_dec(va);
            taskdump_raw_line(b" v=");
            taskdump_dec(v as u64);
            taskdump_raw_line(b"\n");
        }
        None => {}
    }
}

/// DEBUG: dump a parked task's user stack words that look like code pointers,
/// read through the task's own page tables (its mm is not active here).
#[cfg(feature = "debug-futex-trace")]
fn debug_dump_user_stack(t: &crate::process::types::Task, usp: u64) {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::memory_layout::phys_to_virt;
    taskdump_raw_line(b"FTX-USTACK sp=");
    taskdump_dec(usp);
    taskdump_raw_line(b"\n");
    let as_arc = match t.address_space_arc() {
        Some(a) => a,
        None => {
            taskdump_raw_line(b"FTX-USTACK no-mm\n");
            return;
        }
    };
    let root = as_arc.root_ppn();
    // Read one user u32 through the task's page tables.
    // Diagnostic walk + linear-map reads through the family's one edge.
    let read_u32 = |va: u64| -> Option<u32> { self::walk_read::<u32>(root, va) };
    let read_u64 = |va: u64| -> Option<u64> { self::walk_read::<u64>(root, va) };
    // Xorg input-thread globals: guard ptr @0x1c9a58, input_mutex @0x1c81f0,
    // lock counter @0x206598.
    if let Some(p) = read_u64(0x1c9a58) {
        taskdump_raw_line(b"FTX-GUARDPTR=");
        taskdump_dec(p);
        if let Some(v) = read_u32(p) {
            taskdump_raw_line(b" *p=");
            taskdump_dec(v as u64);
        }
        taskdump_raw_line(b"\n");
    }
    if let Some(v) = read_u32(0x1c81f0) {
        taskdump_raw_line(b"FTX-MUTEXVAL=");
        taskdump_dec(v as u64);
        taskdump_raw_line(b"\n");
    }
    if let Some(v) = read_u32(0x206598) {
        taskdump_raw_line(b"FTX-LOCKCOUNT=");
        taskdump_dec(v as u64);
        taskdump_raw_line(b"\n");
    }
    let lo = usp.saturating_sub(0x200) & !0xfu64;
    let hi = usp + 0x900;
    let mut addr = lo;
    let mut shown = 0;
    while addr < hi && shown < 60 {
        {
            // Diagnostic walk + linear-map read (one edge for the family);
            // per-word walks keep the original skip-unmapped semantics.
            let v = match self::walk_read::<u64>(root, addr) {
                Some(v) => v,
                None => {
                    addr = (addr + 0x1000) & !0xfffu64;
                    continue;
                }
            };
            // User code pointers: Xorg image (< 0x1d0000) or shared libs
            // (0x0000_3000_xxxx_xxxx) or any canonical user text pointer.
            // sc119: OH app children park with library frames at
            // 0x0000_7ff0_xxxx_xxxx (sc118 USEGV rip 0x7ff000fe1b07 family)
            // — same 4-GB-prefix test as the other two families.
            let is_user_ptr = (v > 0x1000 && v < 0x4000_0000_0000)
                && (v < 0x00d0_0000 || (v >> 32) == 0x3000 || (v >> 32) == 0x3fff || (v >> 32) == 0x7ff0);
            if is_user_ptr {
                taskdump_raw_line(b"FTX-US +");
                taskdump_dec(addr.wrapping_sub(usp) as i64 as u64);
                taskdump_raw_line(b" =");
                taskdump_dec(v);
                taskdump_raw_line(b"\n");
                shown += 1;
            }
            addr += 8;
        }
    }
    if shown == 0 {
        taskdump_raw_line(b"FTX-USTACK none\n");
    }
    drop(as_arc);
}

/// Snapshot every task in the PID hash. One line per task, plus a header
/// so the dump is self-describing in a log full of interleaved output.
// ---- debug-taskdump-bt arch accessors --------------------------------
// The bt lines read the task's last trap frame: (user pc, user sp,
// syscall nr, arg0..arg2). Field names differ per arch: riscv64 keeps
// the RISC-V names (epc/sp/a7/orig_a0..a2), x86_64 keeps
// rip/rsp/orig_rax/rdi/rsi/rdx. nr==0 means "in pure user mode" on
// both (no syscall in flight).
#[cfg(feature = "debug-taskdump-bt")]
fn bt_frame(pr: &crate::arch::pt_regs::PtRegs) -> (u64, u64, u64, u64, u64, u64) {
    #[cfg(feature = "riscv64")]
    {
        (pr.epc, pr.sp, pr.a7, pr.orig_a0, pr.a1, pr.a2)
    }
    #[cfg(feature = "x86_64")]
    {
        (pr.rip, pr.rsp, pr.orig_rax, pr.rdi, pr.rsi, pr.rdx)
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    {
        (0, 0, 0, 0, 0, 0)
    }
}


// Kernel-text window for the stack-word return-address filter.
/// riscv64: kernel half from 0xffffffff80000000; x86_64: linked image
/// at 0xffffffff80200000 (boot log "image 0xffffffff80200000-...") with
/// headroom to 0xffffffff82000000 for the tail/bss — diagnostics only,
/// a stray extra word costs nothing.
#[cfg(feature = "debug-taskdump-bt")]
const fn bt_text_window() -> (u64, u64) {
    #[cfg(feature = "riscv64")]
    { (0xffff_ffff_8000_0000, 0xffff_ffff_8020_0000) }
    #[cfg(feature = "x86_64")]
    { (0xffff_ffff_8020_0000, 0xffff_ffff_8200_0000) }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    { (0, 0) }
}

/// Plausible-user-pointer window for uepc/usp (matches the historical
/// 0x1000..0x0000_4000_0000_0000 guards).
#[cfg(feature = "debug-taskdump-bt")]
const fn bt_user_window() -> (u64, u64) {
    (0x1000, 0x0000_4000_0000_0000)
}

pub fn vw_wait_counters_line() -> alloc::string::String {
    use core::sync::atomic::Ordering::Relaxed;
    alloc::format!(
        "VWWAIT enter={} sched={} resp={} timeout={}",
        crate::drivers::hotpath::VW_WAIT_ENTER.load(Relaxed),
        crate::drivers::hotpath::VW_WAIT_SCHED.load(Relaxed),
        crate::drivers::hotpath::VW_WAIT_RESP.load(Relaxed),
        crate::drivers::hotpath::VW_WAIT_TIMEOUT.load(Relaxed),
    )
}

pub fn dump_all_tasks(reason: &str) {
    puts("\n=== DEBUG TASK DUMP (");
    puts(reason);
    puts(") ===\n");
    // R57-scribble: one-time Task-field offset map — the pk= garbage
    // captures (ti_pick_state reading 91/112/158/122400/0x38E0000 on
    // live tasks) prove a writer lands inside the thread_info
    // neighborhood; the exact offsets identify which 8-byte field's
    // low/high half overlaps the pick-state word.
    {
        static OFFS_PRINTED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !OFFS_PRINTED.swap(true, core::sync::atomic::Ordering::AcqRel) {
            use crate::process::hotpath::task_offsets as to;
            puts("taskoff:");
            let offs: [(&str, usize); 11] = [
                (" ti_flags", to::TI_FLAGS),
                (" ti_kernel_sp", to::TI_KERNEL_SP),
                (" ti_user_sp", to::TI_USER_SP),
                (" ti_cpu", to::TI_CPU),
                (" cpus_allowed", to::TI_CPUS_ALLOWED),
                (" hung_task_since", to::TI_HUNG_TASK_SINCE),
                (" task_refcnt", to::TI_TASK_REFCNT),
                (" journal_handle", to::TI_JOURNAL_HANDLE),
                (" ti_on_cpu", to::TI_ON_CPU),
                (" running_on_cpu", to::TI_RUNNING_ON_CPU),
                (" ti_pick_state", to::TI_PICK_STATE),
            ];
            for (name, off) in offs {
                puts(name);
                puts("=0x");
                put_hex(off as u64);
            }
            putc(b'\n');
        }
    }
    // R56-diag: per-CPU slot currents (who each slot thinks is running).
    {
        let cs = crate::sched::hotpath::grq_diag_cpu_currents();
        puts("cpu slots:");
        for c in 0..crate::config::MAX_CPUS {
            putc(b' ');
            let cur = cs[c].2;
            if cur.is_null() {
                put_dec(999999);
            } else {
                // SAFETY (one adopt edge): the per-CPU current Task stays
                // allocated; pid is a stable field read.
                put_dec(unsafe { crate::process::types::TaskBorrow::from_raw(cur) }
                    .map(|t| t.pid() as u64)
                    .unwrap_or(999999));
            }
        }
        putc(b'\n');
    }

    // 4-thread hang hunt: the idle fast path trusts grq_nr_running() (the
    // SUM of the per-class queue depths). Print it against the atomic and
    // the per-CPU current slots so a counter/queue divergence is visible in
    // one snapshot.
    {
        let (nr, cfs, rt, dl) = crate::sched::hotpath::grq_diag_counts();
        puts("grq: nr_running=");
        put_dec(nr as u64);
        puts(" cfs=");
        put_dec(cfs);
        puts(" rt=");
        put_dec(rt);
        puts(" dl=");
        put_dec(dl);
        let cur = crate::sched::hotpath::grq_diag_cpu_currents();
        puts(" cpu[pid/idle]:");
        for c in 0..crate::config::MAX_CPUS {
            putc(b' ');
            if cur[c].0 == u32::MAX {
                puts("-");
            } else {
                put_dec(cur[c].0 as u64);
            }
            putc(b'/');
            putc(if cur[c].1 { b'i' } else { b'b' });
        }
        putc(b'\n');
        // Backtrace each CPU's current (even hash-invisible ones — the
        // post-exit spinner family). Same guard discipline as the per-task
        // bt below.
        #[cfg(feature = "debug-taskdump-bt")]
        for c in 0..crate::config::MAX_CPUS {
            let t = cur[c].2 as *mut crate::process::types::Task;
            if t.is_null() {
                continue;
            }
            // SAFETY (one adopt edge): the per-CPU current snapshot's Task
            // stays allocated (idle tasks and hash-invisible exiting tasks
            // are still valid Task objects); stable-field diagnostic read.
            let tk = match unsafe { crate::process::types::TaskBorrow::from_raw(t) } {
                Some(tk) => tk,
                None => continue,
            };
            let (pid, st, preempt) = (tk.pid(), tk.state().bits(), tk.preempt_count());
            puts("cpu");
            put_dec(c as u64);
            puts(" cur pid=");
            put_dec(pid as u64);
            puts(" state=");
            puts(state_name(st));
            puts(" preempt=");
            let pv = preempt as u64;
            if (pv as i64) < 0 {
                puts("-");
                put_dec((pv as i64).unsigned_abs());
            } else {
                put_dec(pv);
            }
            // Last user epc (pt_regs at kernel-stack top) — a userland
            // tight-loop vs mid-exit distinction.
            {
                if let Some(pr) = tk.pt_regs_ref() {
                    let (epc, _usp, _nr, _a0, _a1, _a2) = bt_frame(pr);
                    let (lo, hi) = bt_user_window();
                    if epc > lo && epc < hi {
                        puts(" uepc=");
                        put_hex(epc);
                    }
                }
            }
            puts(" @0x");
            put_hex(t as u64);
            {
                let sp = tk.thread().sp;
                // Bounded readability (kernel_va_readable): the old
                // kernel-half-only guard accepted the 64TB unmapped
                // window above RAM — a stale sp there faulted the dump
                // itself from the timer softirq (the OH taskdump panic,
                // "Fatal exception in interrupt").
                if (sp >> 48) == 0xffff
                    && (sp & 7) == 0
                    && sp > 0x1000
                    && self::kernel_va_readable(sp as usize)
                {
                    puts(" sp=");
                    put_hex(sp);
                    // Stack-word scan through the bounds-checked window
                    // (INV-WINDOW-*): the task's stack pages stay mapped
                    // while the Task exists.
                    let win = tk.kernel_stack_top_checked().and_then(|top| {
                        crate::safety::frame::KernelStackWindow::new(
                            top - crate::config::KERNEL_STACK_SIZE,
                            top,
                        )
                    });
                    let mut shown = 0;
                    for k in 0..40 {
                        let v = match win.and_then(|w| w.read_u64((sp as usize).wrapping_sub(k))) {
                            Some(v) => v,
                            None => continue,
                        };
                        if v >= bt_text_window().0 && v < bt_text_window().1 && (v & 1) == 0 {
                            putc(b' ');
                            put_hex(v);
                            shown += 1;
                            if shown >= 6 {
                                break;
                            }
                        }
                    }
                }
            }
            putc(b'\n');
        }
    }

    let mut count: usize = 0;
    let mut linked_unknown: usize = 0;
    // Non-blocking iteration: this dump runs from the spinlock deadlock
    // watchdog (spinning, IRQs off) or the UART RX magic — exactly the
    // contexts where another CPU may be stuck HOLDING a pid-hash bucket
    // lock (the kill-broadcast / OOM paths call send_signal → wake_up
    // inside the for_each callback). Blocking on a bucket lock would wedge
    // the diagnostic itself, so busy buckets are skipped and the partial
    // snapshot is labeled below.
    //
    // SAFETY: pid_hash_for_each_task_try pins nothing and blocks on nothing;
    // the callback only reads stable Task fields (pid/state/policy/comm) of
    // tasks that remain in the hash while we hold the bucket lock. A task
    // exiting on another CPU concurrently may be missed (fine for a
    // snapshot) but can never be half-printed: the slot is freed only after
    // pid_hash_remove, which the iterator's bucket locks serialize against.
    let skipped =
        crate::process::hotpath::pid_hash_for_each_task_try(|task_ptr| {
            let t: &crate::process::types::Task = task_ptr;
            count += 1;
            puts("task @0x");
            put_hex(task_ptr as *const _ as u64);
            puts(" pid=");
            put_dec(t.pid() as u64);
            // Thread-group attribution: comm alone cannot say which
            // process an OS_FFRT_2_2 or OS_IPC_3 worker serves, and the
            // b140 hunt needed exactly that (accountmgr's FFRT worker
            // normal-exited mid-activation while its IPC threads kept
            // answering callbacks). tgid= names the owner unambiguously.
            puts(" tgid=");
            put_dec(t.tgid() as u64);
            puts(" state=");
            puts(state_name(t.state().bits()));
            puts(" ti_cpu=");
            put_dec(t.ti_cpu() as u64);
            puts(" on_cpu=");
            put_dec(t.on_cpu() as u64);
            // GNOME-wedge hunt: the CFS pick predicate refuses a task when
            // its running_on_cpu claim names another CPU, so a leaked claim
            // strands a linked task permanently (g2: 10 tasks linked, all
            // CPUs idle-looping, nothing picked for 35 min). The claim was
            // the ONE ownership field the dump never printed — add it.
            {
                let claim = t.running_on_cpu.load(core::sync::atomic::Ordering::Relaxed);
                puts(" claim=");
                if (claim as i64) < 0 {
                    putc(b'-');
                    put_dec((claim as i64).unsigned_abs());
                } else {
                    put_dec(claim as u64);
                }
            }
            // R57: the pick-state word (0=PICKED 1=LIVE 2=REVOKED) is what
            // arbitrates the slot-capture steal — printed beside the claim
            // it settles "in-flight pick vs live runner vs already stolen"
            // in one dump line (the n1-organic panic hinged on exactly
            // this distinction).
            puts(" pk=");
            let pkv = t.pick_state();
            put_dec(pkv as u64);
            if pkv > 2 {
                // Scribbler fingerprint: any other value is a foreign store
                // landing on this word. Make it loud and unmistakable.
                puts(" PK-GARBAGE");
            }
            puts(" preempt=");
            let pv = t.preempt_count() as u64;
            if (pv as i64) < 0 {
                putc(b'-');
                put_dec((pv as i64).unsigned_abs());
            } else {
                put_dec(pv);
            }
            // Authoritative linked-state (tree/list scan, not the flag) —
            // settles flag-desync vs really-off-queue in one shot. The scan
            // runs under the GRQ try-lock; a busy lock reports '?' (counted
            // below) — never a lockless tree walk, which races structural
            // mutation on other CPUs and corrupts the iterator.
            let linked = crate::sched::hotpath::grq_diag_cfs_linked(
                task_ptr as *const crate::process::types::Task as *mut crate::process::types::Task);
            match linked {
                Some(b) => {
                    puts(" linked=");
                    put_dec(b as u64);
                }
                None => {
                    puts(" linked=?");
                    linked_unknown += 1;
                }
            }
            puts(" on_rq=");
            put_dec(t.sched_entity().on_rq.load(Ordering::Relaxed) as u64);
            puts(" affinity=0x");
            put_hex(t.cpus_allowed() as u64);
            puts(" vruntime=");
            put_dec(t.sched_entity().get_vruntime());
            puts(" policy=");
            puts(policy_name(t));
            puts(" comm=");
            let comm = t.comm();
            let name = comm.split(|&b| b == 0).next().unwrap_or(b"?");
            for &b in name.iter().take(16) {
                if b >= 0x20 && b < 0x7f {
                    putc(b);
                } else {
                    putc(b'.');
                }
            }
            // Last syscall latched at dispatch ENTRY (per-task): for a
            // sleeping task this names the syscall it is parked in —
            // the discriminator for "which kernel object is this waiter
            // on" hunts (account-activation stall: epoll? futex? binder
            // ioctl? signalfd?).
            {
                let nr = t.ext.last_syscall_nr.load(core::sync::atomic::Ordering::Relaxed);
                if nr != 0 {
                    let a0 = t.ext.last_syscall_a0.load(core::sync::atomic::Ordering::Relaxed);
                    let a1 = t.ext.last_syscall_a1.load(core::sync::atomic::Ordering::Relaxed);
                    puts(" lastsys=");
                    put_dec(nr as u64);
                    puts("(");
                    put_hex(a0);
                    putc(b',');
                    put_hex(a1);
                    puts(")");
                } else {
                    puts(" lastsys=?");
                }
            }
            putc(b'\n');
            #[cfg(feature = "debug-taskdump-bt")]
            {
                // R38: coarse stack backtrace — print thread.sp and the first
                // code-region words scanning down the kernel stack (return
                // addresses), for wedged-sleeper and RUNNING-but-unscheduled
                // diagnosis. Guarded to plausible kernel-stack pointers only.
                // Feature-gated: the extra code shifts binary layout and
                // measurably widens the heap-lock race window — enable only
                // when hunting (DEBUG zero-cost-when-off principle).
                // Hunt addition: last user epc/sp — pinpoints the user
                // instruction a sleeper is blocked at (musl futex etc.).
                {
                    if let Some(pr) = t.pt_regs_ref() {
                        let (epc, usp, a7, a0, a1, a2) = bt_frame(pr);
                        let (ulo, uhi) = bt_user_window();
                        if epc > ulo && epc < uhi {
                            puts(" uepc=");
                            put_hex(epc);
                        }
                        if usp > ulo && usp < uhi {
                            puts(" usp=");
                            put_hex(usp);
                        }
                        // Last syscall args (nr, arg0..arg2) — identifies
                        // the futex word a sleeper is parked on. riscv64
                        // keeps the nr in a7 (98=futex); x86_64 in
                        // orig_rax (202=futex).
                        if a7 > 0 && a7 < 500 {
                            puts(" nr=");
                            put_dec(a7);
                            puts(" a0=");
                            put_hex(a0);
                            puts(" a1=");
                            put_hex(a1);
                            puts(" a2=");
                            put_hex(a2);
                        }
                        // Futex sleeper: dump the word + waiter chain truth.
                        // These helpers are defined under `debug-futex-trace`
                        // (they print through the FTX tracer) — the call
                        // sites must carry the SAME gate or the
                        // `debug-taskdump-bt` feature fails to link alone.
                        #[cfg(feature = "debug-futex-trace")]
                        {
                            #[cfg(feature = "riscv64")]
                            const FUTEX_NR: u64 = 98;
                            #[cfg(feature = "x86_64")]
                            const FUTEX_NR: u64 = 202;
                            #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
                            const FUTEX_NR: u64 = 0;
                            if a7 == FUTEX_NR && a0 > 0x1000 && (a0 & 0x3) == 0 {
                                if t.state().is_sleeping() {
                                    // sc119: the OH app-child hunt. Hundreds of
                                    // futex sleepers exist per dump; the word
                                    // read inside FTX-STATE lands in the
                                    // DUMPER's mm anyway (wrong address space)
                                    // and a full ustack per sleeper floods the
                                    // UART. Gate the expensive dumps to the
                                    // hunt page 0x7fffffffc000 (every process's
                                    // stack-top-minus-3rd page, where all 13
                                    // app children parked); the bucket-chain
                                    // walk inside FTX-STATE is kernel-global
                                    // and stays valid there.
                                    if (a0 & !0xfffu64) == 0x7fffffffc000 {
                                        crate::sync::hotpath::debug_dump_futex_state(
                                            a0 as usize,
                                            t.pid(),
                                        );
                                        debug_dump_user_stack(&t, usp);
                                    }
                                }
                            }
                        }
                    }
                }
                let sp = t.thread().sp;
                // Same bounded-readability guard as the per-CPU walk.
                if (sp >> 48) == 0xffff
                    && (sp & 7) == 0
                    && sp > 0x1000
                    && self::kernel_va_readable(sp as usize)
                {
                    puts("     sp=");
                    put_hex(sp);
                    // Stack-word scan through the bounds-checked window
                    // (INV-WINDOW-*): the walk's bucket lock keeps the
                    // task's slot alive for the read.
                    let win = t.kernel_stack_top_checked().and_then(|top| {
                        crate::safety::frame::KernelStackWindow::new(
                            top - crate::config::KERNEL_STACK_SIZE,
                            top,
                        )
                    });
                    let mut shown = 0;
                    for k in 0..40 {
                        let v = match win.and_then(|w| w.read_u64((sp as usize).wrapping_sub(k))) {
                            Some(v) => v,
                            None => continue,
                        };
                        if v >= bt_text_window().0 && v < bt_text_window().1 && (v & 1) == 0 {
                            putc(b' ');
                            put_hex(v);
                            shown += 1;
                            if shown >= 6 { break; }
                        }
                    }
                    puts("\n");
                }
            }
        });
    puts("=== ");
    put_dec(count as u64);
    puts(" tasks ===\n");
    if skipped > 0 {
        puts("warning: ");
        put_dec(skipped as u64);
        puts(" pid-hash buckets were locked (stuck holder?) and skipped\n");
    }
    if linked_unknown > 0 {
        puts("warning: ");
        put_dec(linked_unknown as u64);
        puts(" linked= probes skipped (GRQ lock busy)\n");
    }
    // Heap/page snapshot on every periodic dump: the leak curve rides the
    // same 5s cadence, and with rux:memwatch (retired debug=memwatch) the size-class growth table
    // and big-object call-site histogram follow (memwatch.rs). Only from
    // the periodic path — the watchdog variant may run while the heap lock
    // is the wedged one, and the stats walk takes allocator locks.
    if reason == "periodic" {
        // R85 (b148 crawl hunt): the ext4 biglock's own state beside the
        // task snapshot — the b148 shape (whole OH cohort D-parked, lock
        // apparently free, zero STALL prints) was invisible without it.
        crate::fs::hotpath::EXT4_BIG_LOCK.debug_census_line();
        self::dump_mem();
        // pa16 (magazine): the packing layer's per-class op counts and
        // class-lock acquisitions on the same cadence — locks vs
        // obj_allocs+obj_frees is the magazine's live traffic-ratio
        // readout (atomic loads + raw UART only, no allocator lock,
        // safe from the periodic path).
        crate::mm::hotpath::census_line();
    }
}

fn state_name(bits: u32) -> &'static str {
    use crate::process::types::TaskState as S;
    match bits {
        S::RUNNING => "RUNNING",
        // R52: a task between new_task_at and its first enqueue. Legit
        // only transiently inside do_clone/kernel_thread; a PERSISTENT
        // NEW+ti_cpu=-1 line means the creating fork never reached its
        // enqueue (or its CPU froze mid-fork) — while RUNNING+ti_cpu=-1
        // is now impossible from the constructor and points at an
        // out-of-protocol state-word write.
        S::TASK_NEW => "NEW(built)",
        S::INTERRUPTIBLE => "INTERRUPTIBLE(sleep)",
        S::UNINTERRUPTIBLE => "UNINTERRUPTIBLE(dsleep)",
        S::STOPPED => "STOPPED",
        S::TRACED => "TRACED",
        S::ZOMBIE => "ZOMBIE",
        S::DEAD => "DEAD",
        _ => {
            if bits & S::ZOMBIE != 0 {
                "ZOMBIE(mixed)"
            } else if bits & S::DEAD != 0 {
                "DEAD(mixed)"
            } else if bits & S::UNINTERRUPTIBLE != 0 {
                "UNINTERRUPTIBLE(mixed)"
            } else if bits & S::INTERRUPTIBLE != 0 {
                "INTERRUPTIBLE(mixed)"
            } else if bits & S::STOPPED != 0 {
                "STOPPED(mixed)"
            } else {
                "UNKNOWN"
            }
        }
    }
}

fn policy_name(t: &crate::process::types::Task) -> &'static str {
    use crate::process::types::SchedPolicy;
    match t.policy() {
        SchedPolicy::Normal => "CFS",
        SchedPolicy::Batch => "BATCH",
        SchedPolicy::Idle => "IDLE",
        SchedPolicy::Fifo => "FIFO",
        SchedPolicy::Rr => "RR",
        SchedPolicy::Deadline => "DL",
    }
}

// ============================================================================
// rcu-gp-frozen monopolist truth dump (52nd session, oh-crash lane)
// ============================================================================

/// Kernel-text window for the live stack scan (same bounds as
/// bt_text_window's x86_64 arm, kept independent of the
/// debug-taskdump-bt feature so this probe builds in the stock kernel).
#[cfg(feature = "x86_64")]
const FROZEN_TEXT_LO: u64 = 0xffff_ffff_8020_0000;
#[cfg(feature = "x86_64")]
const FROZEN_TEXT_HI: u64 = 0xffff_ffff_8200_0000;


#[cfg(feature = "x86_64")]
static FROZEN_ANSWER: FrozenAnswer = FrozenAnswer {
    answered: core::sync::atomic::AtomicBool::new(false),
    gs_cpu: core::sync::atomic::AtomicU32::new(u32::MAX),
    gs_magic: core::sync::atomic::AtomicU64::new(0),
    lapic: core::sync::atomic::AtomicU32::new(u32::MAX),
    int_rip: core::sync::atomic::AtomicU64::new(0),
    int_rsp: core::sync::atomic::AtomicU64::new(0),
    stack: [const { core::sync::atomic::AtomicU64::new(0) }; 12],
    stack_n: core::sync::atomic::AtomicUsize::new(0),
};

/// CSD callback: runs ON the frozen CPU, in CALL_FUNCTION IPI context.
/// Reads ONLY per-CPU/own-stack state — no locks the monopolist could
/// hold (CURRENT_PT_REGS is an atomic slot; the LAPIC id is an MSR/IO
/// read; the stack scan reads the interrupted stack downward from the
/// IPI frame — all mapped while the task runs).
#[cfg(feature = "x86_64")]
fn frozen_ipi_probe(_arg: *mut core::ffi::c_void) {
    use core::sync::atomic::Ordering;
    let a = &FROZEN_ANSWER;

    // (a) GS-derived cpu id + the GS-base magic word — the percpu
    // misdirection canary (51st session's prime suspect: cpu1's
    // cpu_id() resolving to 0 would land its QS reports in slot 0).
    let gs_cpu = crate::arch::x86_64::smp::gs_cpu_id() as u32;
    let gs_magic_q: u64;
    // SAFETY: %gs:PC_MAGIC reads this CPU's PerCpu.magic — a plain data
    // read of the active GS base, exactly what the trap stubs do.
    unsafe {
        core::arch::asm!(
            "mov {0}, qword ptr gs:[{1}]",
            out(reg) gs_magic_q,
            const crate::arch::x86_64::smp::PC_MAGIC,
            options(nomem, nostack, preserves_flags)
        );
    }
    a.gs_cpu.store(gs_cpu, Ordering::Relaxed);
    a.gs_magic.store(gs_magic_q, Ordering::Relaxed);

    // (b) LAPIC id — hardware CPU identity, completely independent of
    // the GS base. gs_cpu vs lapic mismatch convicts misdirection.
    a.lapic.store(crate::drivers::hotpath::intc::apic::this_lapic_id(), Ordering::Relaxed);

    // (c) The interrupted frame: CURRENT_PT_REGS[cpu] was swapped to
    // THIS IPI's PtRegs by trap_handler before dispatch, so its rip/rsp
    // are the monopolist loop's live instruction and stack pointer.
    let pr = crate::arch::trap::current_pt_regs() as *const crate::arch::pt_regs::PtRegs;
    if !pr.is_null() && (pr as u64) >= 0xffff_8000_0000_0000 {
        // SAFETY: CURRENT_PT_REGS[cpu] was swapped to this IPI's live
        // frame by trap_handler before dispatch — the one frame edge.
        let frame = unsafe { &*pr };
        let (rip, rsp) = (frame.rip, frame.rsp);
        a.int_rip.store(rip, Ordering::Relaxed);
        a.int_rsp.store(rsp, Ordering::Relaxed);

        // (d) Live stack scan: from the interrupted RSP upward (toward
        // the frame's callers), keep kernel-text return addresses.
        if rsp > 0x1000 && (rsp & 7) == 0 {
            let mut n = 0usize;
            // Upward scan of the interrupted task's own stack through the
            // bounds-checked window (INV-WINDOW-*); the stack stays mapped
            // while the task monopolizes the CPU.
            let win = KernelStackWindow::new(rsp as usize, (rsp + 64 * 8) as usize);
            for k in 0..64 {
                let v = match win.and_then(|w| w.read_u64((rsp + k * 8) as usize)) {
                    Some(v) => v,
                    None => continue,
                };
                if v >= FROZEN_TEXT_LO && v < FROZEN_TEXT_HI && (v & 1) == 0 {
                    a.stack[n].store(v, Ordering::Relaxed);
                    n += 1;
                    if n >= a.stack.len() {
                        break;
                    }
                }
            }
            a.stack_n.store(n, Ordering::Relaxed);
        }
    }
    a.answered.store(true, Ordering::Release);
}

/// One-shot cross-CPU truth dump for the frozen-CPU monopolist hunt.
/// Called from rcu.rs's grace-freeze one-shot on the WAITING cpu:
/// 1. prints every CPU's timer-tick count + last tick-entry RIP +
///    tick-entry preempt count (a stalled tick count on the frozen CPU
///    = IRQs off; an advancing count + frozen QS = a preempt-loop with
///    IRQs on — and its last tick-entry RIP names the loop directly);
/// 2. IPIs the stuck CPU with [`frozen_ipi_probe`], which answers with
///    the percpu canary (gs cpu id + magic vs LAPIC id) and the
///    interrupted RIP + live stack — or stays silent, which itself
///    convicts an IRQ-off monopolist.
/// Anomaly-gated one-shot per boot (caller latches); zero cost on a
/// healthy boot.
pub fn dump_frozen_cpu_truth(stuck: usize) {
    #[cfg(feature = "x86_64")]
    {
        puts("\n=== FROZEN-CPU TRUTH (stuck=");
        put_dec(stuck as u64);
        puts(") ===\n");
        for c in 0..crate::config::MAX_CPUS {
            if !crate::sched::hotpath::cpu_online(c) {
                continue;
            }
            puts("FROZEN-CPU ticks cpu");
            put_dec(c as u64);
            puts(": n=");
            put_dec(crate::fs::hotpath::timer_count(c));
            puts(" last_rip=");
            put_hex(
                crate::interrupt::preempt::TICK_ENTRY_RIP[c]
                    .load(core::sync::atomic::Ordering::Relaxed),
            );
            puts(" entry_pc=");
            put_hex(
                crate::interrupt::preempt::TICK_ENTRY_PC[c]
                    .load(core::sync::atomic::Ordering::Relaxed) as u64,
            );
            putc(b'\n');
        }

        if stuck >= crate::config::MAX_CPUS
            || !crate::sched::hotpath::cpu_online(stuck)
        {
            return;
        }

        FROZEN_ANSWER.answered.store(false, core::sync::atomic::Ordering::Release);
        let ok = crate::arch::x86_64::ipi::smp_call_function_timeout(
            stuck,
            frozen_ipi_probe,
            core::ptr::null_mut(),
            200,
        );
        if !ok {
            puts("FROZEN-CPU ipi: cpu");
            put_dec(stuck as u64);
            puts(" DID NOT ANSWER in 200ms — IRQs off or CSD-queue wedge (tick line above disambiguates)\n");
            // IF=0 fallback: the stuck CPU's current task (sched slot) is
            // the victim; its LIVE frames sit BELOW the sp saved at its
            // last switch-in (t.thread().sp — the restore point; the task
            // has only called deeper since). Scan downward and print
            // kernel-text words: names the spin loop and, via the frames
            // above it, the lock's owner path. Read-side only, from this
            // CPU; races cost at worst a torn-but-bounded word.
            let cur = crate::sched::hotpath::grq_diag_cpu_currents();
            let t = cur[stuck].2 as *mut crate::process::types::Task;
            if !t.is_null() {
                // SAFETY (one adopt edge): as the per-CPU walk — the
                // current Task stays allocated; stable-field reads.
                let tk = match unsafe { crate::process::types::TaskBorrow::from_raw(t) } {
                    Some(tk) => tk,
                    None => return,
                };
                let sp = tk.thread().sp;
                puts("FROZEN-CPU victim pid=");
                put_dec(tk.pid() as u64);
                puts(" preempt=");
                put_dec(tk.preempt_count() as u64);
                puts(" saved_sp=");
                put_hex(sp);
                if (sp >> 48) == 0xffff && (sp & 7) == 0 && sp > 0x1000 {
                    puts(" stack:");
                    let win = tk.kernel_stack_top_checked().and_then(|top| {
                        crate::safety::frame::KernelStackWindow::new(
                            top - crate::config::KERNEL_STACK_SIZE,
                            top,
                        )
                    });
                    let mut shown = 0usize;
                    for k in 0..96 {
                        let v = match win.and_then(|w| w.read_u64((sp as usize).wrapping_sub(k))) {
                            Some(v) => v,
                            None => continue,
                        };
                        if v >= FROZEN_TEXT_LO && v < FROZEN_TEXT_HI && (v & 1) == 0 {
                            putc(b' ');
                            put_hex(v);
                            shown += 1;
                            if shown >= 16 {
                                break;
                            }
                        }
                    }
                }
                putc(b'\n');
            }
            return;
        }
        use core::sync::atomic::Ordering;
        let a = &FROZEN_ANSWER;
        if !a.answered.load(Ordering::Acquire) {
            // Defensive: completion without the answer flag is not
            // reachable (callback sets both), but never wedge the dump.
            return;
        }
        puts("FROZEN-CPU ipi cpu");
        put_dec(stuck as u64);
        puts(": gs_cpu=");
        put_dec(a.gs_cpu.load(Ordering::Relaxed) as u64);
        puts(" gs_magic=");
        put_hex(a.gs_magic.load(Ordering::Relaxed));
        puts(" lapic=");
        put_hex(a.lapic.load(Ordering::Relaxed) as u64);
        puts(" int_rip=");
        put_hex(a.int_rip.load(Ordering::Relaxed));
        puts(" int_rsp=");
        put_hex(a.int_rsp.load(Ordering::Relaxed));
        let n = a.stack_n.load(Ordering::Relaxed);
        puts(" stack:");
        for k in 0..n.min(a.stack.len()) {
            putc(b' ');
            put_hex(a.stack[k].load(Ordering::Relaxed));
        }
        putc(b'\n');
    }
    #[cfg(not(feature = "x86_64"))]
    {
        let _ = stuck;
    }
}

/// 53rd session (52nd §6a): parked-kstack symbolizer for S-state tasks.
///
/// A task sleeping in the kernel parks with its full call chain intact
/// between its saved switch sp (`t.thread().sp`, the restore point of
/// the last context switch, set inside schedule()) and the entry frame
/// at the stack top. Walking UPWARD from the saved sp and symbolizing
/// every kernel-text word names the innermost wait function — wait
/// queue, completion, SleepMutex, bio page wait — which the syscall
/// number alone cannot (hilogd main parked `lastsys=9` for 600 s with
/// no way to see what inside the kernel it sleeps on).
///
/// Dedup: a (pid, saved-sp) pair prints its line once; a parked task's
/// stack is frozen while it sleeps, so repeats carry zero information.
/// Bounded ring of 256 entries; allocation-free (raw atoms).
///
/// SAFETY contract: the caller holds no pid-hash bucket lock while the
/// RETURNED STRING is printed (sc207); the walk itself only
/// read_volatile's words inside the sleeping task's own kernel-stack
/// pages, which the Task pins. A task that wakes mid-walk costs a torn
/// word at worst.
pub fn parked_kstack_syms(t: &crate::process::types::Task) -> alloc::string::String {
    use core::fmt::Write as _;
    use crate::process::types::Task;

    // Kernel text window (same bounds discipline as the R76 HUNG-STK
    // scan; the two arch linker scripts spell the bounds differently).
    #[cfg(feature = "x86_64")]
    extern "C" {
        static __stext: u8;
        static __etext: u8;
    }
    #[cfg(not(feature = "x86_64"))]
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    #[cfg(feature = "x86_64")]
    let lo = core::ptr::addr_of!(__stext) as u64;
    #[cfg(feature = "x86_64")]
    let hi = core::ptr::addr_of!(__etext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let lo = core::ptr::addr_of!(_stext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let hi = core::ptr::addr_of!(_etext) as u64;

    let mut s = alloc::string::String::new();
    let thsp = t.thread().sp;
    // Guard: canonical kernel stack pointer, 8-aligned, inside the
    // task's own stack window (checked against kernel_stack_bottom so
    // the upward walk cannot leave the task's pages).
    let bottom = t.kernel_stack_bottom();
    if (thsp >> 48) != 0xffff || (thsp & 7) != 0 || thsp < 0x1000 {
        return s;
    }
    if bottom != 0
        && (thsp < bottom as u64
            || thsp >= (bottom + crate::config::KERNEL_STACK_SIZE) as u64)
    {
        return s;
    }
    // Dedup ring: (pid, sp) already printed -> skip everything.
    {
        const N: usize = 256;
        static SEEN_PID: [core::sync::atomic::AtomicU32; N] =
            [const { core::sync::atomic::AtomicU32::new(0) }; N];
        static SEEN_SP: [core::sync::atomic::AtomicU64; N] =
            [const { core::sync::atomic::AtomicU64::new(0) }; N];
        static CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        let pid = t.pid() as u32;
        for i in 0..N {
            if SEEN_PID[i].load(Ordering::Relaxed) == pid
                && SEEN_SP[i].load(Ordering::Relaxed) == thsp
            {
                return s; // same park, already reported
            }
        }
        let idx = CUR.fetch_add(1, Ordering::Relaxed) % N;
        SEEN_PID[idx].store(pid, Ordering::Relaxed);
        SEEN_SP[idx].store(thsp, Ordering::Relaxed);
    }
    let base = thsp as *const u64;
    let mut shown = 0usize;
    // Segment discipline: printk records cap at 256 bytes, and the
    // sleepsnap emitter chunks on newline boundaries — so emit the walk
    // as newline-separated segments of at most ~230 payload bytes each
    // (header carries tid for reassembly). ohc25's unsegmented lines
    // all died at the record cap mid-symbol.
    let mut seg = alloc::string::String::new();
    let mut seg_len = 0usize;
    let _ = write!(seg, "parkstk: tid {} sp={:#x}:", t.pid(), thsp);
    seg_len = seg.len();
    let mut flush = |seg: &mut alloc::string::String, s: &mut alloc::string::String| {
        seg.push('\n');
        s.push_str(seg);
        seg.clear();
    };
    // Walk upward: live caller frames of the parked chain, bounded by
    // the task's own stack top.
    // ohc27 lesson: kernel stacks are 128 KiB (config::KERNEL_STACK_SIZE)
    // and the reference-parity wait site can sit many KiB above the saved
    // switch sp — a 256-word (2 KiB) window found only the switch
    // machinery. Walk to the stack top (up to 4096 words = 32 KiB).
    let limit = if bottom != 0 {
        (((bottom + crate::config::KERNEL_STACK_SIZE) as u64 - thsp) / 8).min(4096) as usize
    } else {
        4096
    };
    // Upward walk of the sleeping task's own stack through the
    // bounds-checked window (INV-WINDOW-*); the Task pins the pages.
    let win = if bottom != 0 {
        crate::safety::frame::KernelStackWindow::new(
            bottom,
            bottom + crate::config::KERNEL_STACK_SIZE,
        )
    } else {
        None
    };
    for k in 0..limit {
        let v = match win.and_then(|w| w.read_u64((base as usize) + k * 8)) {
            Some(v) => v,
            None => break,
        };
        if v >= lo && v < hi && (v & 1) == 0 {
            let sym = self::format_symbol(v);
            if seg_len + sym.len() + 1 > 230 {
                flush(&mut seg, &mut s);
                let _ = write!(seg, "parkstk+: tid {}:", t.pid());
                seg_len = seg.len();
            }
            let _ = write!(seg, " {}", sym);
            seg_len += sym.len() + 1;
            shown += 1;
            if shown >= 16 {
                break;
            }
        }
    }
    if shown > 0 {
        flush(&mut seg, &mut s);
    }
    s
}

// ---------------------------------------------------------------------------
// Deferred census emission (54th): the S-state census collects under the
// timer softirq, where a printk emitter carries a 100us console-emit
// budget while the OH hilog flood holds the console ~0.4-17ms per line —
// ohc36 lost ~30% of the uvma map lines (and other CPUs' /dev/console
// writes spliced mid-census-record). Producers push pre-chunked LINES
// (each already under the printk record cap) here; a task-context
// kthread drains and printk()s them — task context waits up to 2s for
// the console and always gets through (the mmprofd discipline, lb332).
// Drop-oldest when full: census lines are diagnostic, a stuck boot
// regenerates them every snapshot.
// ---------------------------------------------------------------------------


static DEFER_Q: crate::sync::types::Spinlock<DeferredEmit> =
    crate::sync::types::Spinlock::new(DeferredEmit {
        ring: [const { None }; DEFER_LINE_MAX],
        head: 0,
        tail: 0,
        dropped: 0,
    });

// 55th: focus lane for the park-chain evidence lines (ufw/uss/uvma). The
// bulk census flood evicted them on ohc43 (2k+ lines dropped per window —
// the samgr FFRT-worker walks never reached the serial). A separate ring,
// drained FIRST by censusd; drop-oldest only within its own class.
// DEFER_FOCUS_MAX — the definition moved to debug/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.


static DEFER_FOCUS_Q: crate::sync::types::Spinlock<DeferredFocus> =
    crate::sync::types::Spinlock::new(DeferredFocus {
        ring: [const { None }; DEFER_FOCUS_MAX],
        head: 0,
        tail: 0,
        dropped: 0,
    });

// 71st: PRIORITY focus ring for the RWFRZ adoption dump (the frozen
// word holder's return-address chain — the founder evidence). ohw90:
// routing it through the shared focus class let the NBR-PARK mass
// latch (~50 tgids x deep dumps) evict exactly the adoption dump's
// lines (drop-oldest within one class — the ohw13 uvma trap again,
// now self-inflicted). Its own ring, drained before everything.
const DEFER_FOCUS_A_MAX: usize = DEFER_FOCUS_MAX;
static DEFER_FOCUS_A_Q: crate::sync::types::Spinlock<DeferredFocus> =
    crate::sync::types::Spinlock::new(DeferredFocus {
        ring: [const { None }; DEFER_FOCUS_A_MAX],
        head: 0,
        tail: 0,
        dropped: 0,
    });

/// Queue one census line for task-context emission. Producers may be
/// softirq (timer heartbeat census). The line must already be under the
/// printk record text cap (240 bytes) — the sleep_snapshot chunker
/// guarantees that for everything it feeds here.
pub fn defer_emit_line(line: alloc::string::String) {
    if line.is_empty() {
        return;
    }
    // 55th: park-chain evidence lines go to the focus ring (see its
    // declaration) so the bulk census flood cannot evict them.
    // 61st (ohw13): uvma map dumps move to the BULK ring — the wedge
    // window mass-latches ~50 tgids whose ONE-TIME dumps enqueue
    // thousands of lines in a single burst, filling the 512-slot focus
    // ring and evicting exactly the walk lines the ring exists for
    // (ohw13: 2811 focus drops per burst, tid 344's frames never
    // printed). The dump tables are deterministic per image and
    // reconstructible from any boot; walk lines are not.
    let focus = line.starts_with("ufw:")
        || line.starts_with("uss:")
        || line.starts_with("uhp:")
        // 71st (70th item 2): the RWFRZ park-stack dump lines join the
        // focus lane — direct pr_warn emission interleaved MID-RECORD
        // with the other CPU's census flood (ohw85: "ret[55]=0x7ff000824"
        // spliced into "parkstk+: tid 378"), eating the exact return-
        // address chain the dump exists for. censusd is the single
        // drainer; the focus ring drops only within its own class.
        // "rwfza:" (adoption dump) rides the PRIORITY ring above.
        || line.starts_with("rwfz:");
    if let true = line.starts_with("rwfza:") {
        let mut q = DEFER_FOCUS_A_Q.lock_irqsave();
        let next = (q.tail + 1) % DEFER_FOCUS_A_MAX;
        if next == q.head {
            let h = q.head;
            q.ring[h] = None;
            q.head = (h + 1) % DEFER_FOCUS_A_MAX;
            q.dropped += 1;
        }
        let t = q.tail;
        q.ring[t] = Some(line);
        q.tail = next;
        drop(q);
        ensure_defer_thread();
        return;
    }
    if focus {
        let mut q = DEFER_FOCUS_Q.lock_irqsave();
        let next = (q.tail + 1) % DEFER_FOCUS_MAX;
        if next == q.head {
            let h = q.head;
            q.ring[h] = None;
            q.head = (h + 1) % DEFER_FOCUS_MAX;
            q.dropped += 1;
        }
        let t = q.tail;
        q.ring[t] = Some(line);
        q.tail = next;
    } else {
        let mut q = DEFER_Q.lock_irqsave();
        let next = (q.tail + 1) % DEFER_LINE_MAX;
        if next == q.head {
            // Full: drop the OLDEST line.
            let h = q.head;
            q.ring[h] = None;
            q.head = (h + 1) % DEFER_LINE_MAX;
            q.dropped += 1;
        }
        let t = q.tail;
        q.ring[t] = Some(line);
        q.tail = next;
    }
    ensure_defer_thread();
}

/// Spawn (once) the task-context consumer kthread for `defer_emit_line`.
pub fn ensure_defer_thread() {
    static THREAD_ON: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    if THREAD_ON.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    crate::api::process::kthread_run(
        defer_emit_fn,
        core::ptr::null_mut(),
        "censusd",
    );
}

extern "C" fn defer_emit_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::types::{Task, TaskState};
    loop {
        if crate::api::process::kthread_should_stop() {
            break;
        }
        // 56th: BEFORE draining, dump any latched-but-mapless tgid's
        // vma map (kthread context can take the sleepable vma read for
        // block; the softirq snapshot's try_read lost every attempt
        // for samgr in ohc53). The dump's lines land IN the focus ring
        // this pass drains. Cheap when nothing is pending: two ring
        // scans of atomics.
        crate::api::ipc::dump_latched_maps_taskctx();
        // sc57 (57th): fuxwatch arm round — clears W on the watched
        // word's page in every mapping mm (kthread context; TLB
        // shootdown via flush_tlb_all is safe here).
        crate::sync::hotpath::fuxwatch_arm_round();
        // 83rd: drive the OPEN sqwalk pass (frozen-rwlock reader
        // naming) — kthread context ONLY: ohw151's 594 MB-window bursts
        // in the timer softirq held softirq-context locks across
        // seconds of page walks and deadlocked both CPUs. The slice
        // self-gates to one burst per ~500 ms.
        crate::api::sync::sqwalk_resume();
        // 61st: the ONE-SHOT ffrt::mutex heap map — armed by the first
        // closed chain walk, consumed ONCE here (kthread/sleepable
        // context; the full-heap walk is seconds of page-table walks
        // and must never run in the timer path — ohw20). On an empty
        // result (transient sig failure) the arm is RESTORED so the
        // next round retries; ohw21/22 burned the arm on the first
        // failure and the map never printed.
        {
            static HM_TRIES: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let arm = crate::sync::hotpath::HEAPMAP_ARM.load(core::sync::atomic::Ordering::Relaxed);
            if arm != 0 {
                let samgr = crate::ipc::hotpath::binder_sareg_samgr_pid();
                if samgr != 0 {
                    let out = crate::api::sync::ffrt_mutex_heap_map(samgr, arm);
                    if !out.is_empty() {
                        crate::sync::hotpath::HEAPMAP_ARM.store(
                            0,
                            core::sync::atomic::Ordering::Relaxed,
                        );
                        crate::pr_warn!("FFRT-HEAPMAP tgid={} base={:#x}{}", samgr, arm, out);
                    } else if crate::api::sync::ffrt_mutex_heap_map_done() {
                        // Completed with ZERO qualifying locks — the arm
                        // fired pre-wedge (no waiters yet). Retry up to
                        // 4 times: reset the burn gates and let the next
                        // rounds re-scan as the wedge deepens.
                        if HM_TRIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 4 {
                            crate::api::sync::ffrt_mutex_heap_map_rearm();
                        } else {
                            crate::sync::hotpath::HEAPMAP_ARM.store(
                                0,
                                core::sync::atomic::Ordering::Relaxed,
                            );
                        }
                    }
                }
            }
        }
        // Drain everything queued first: the ADOPTION ring (frozen-word
        // founder evidence) beats the focus ring (park-chain evidence)
        // which beats bulk census.
        loop {
            let line = {
                let mut q = DEFER_FOCUS_A_Q.lock_irqsave();
                if q.head == q.tail {
                    if q.dropped > 0 {
                        let d = q.dropped;
                        q.dropped = 0;
                        Some(alloc::format!(
                            "censusd: {} A-focus lines dropped (queue full)",
                            d
                        ))
                    } else {
                        None
                    }
                } else {
                    let h = q.head;
                    let l = q.ring[h].take();
                    q.head = (h + 1) % DEFER_FOCUS_A_MAX;
                    l
                }
            };
            match line {
                // 97th (hp97): KERN_WARNING, not KERN_INFO — the rig
                // boots run LOGLEVEL=4 (the proven +55s LL7 tax made LL4
                // the rig default) and printk drops level>console BEFORE
                // the ring, so every deferred census line (ufw/uss/uvma
                // park-chain evidence included) was invisible on exactly
                // the boots that hunt with it. Hunt-boots-only traffic;
                // standard boots queue nothing.
                Some(l) => crate::printk::printk_ln(
                    crate::printk::loglevel::KERN_WARNING,
                    format_args!("{}", l),
                ),
                None => break,
            }
        }
        loop {
            let line = {
                let mut q = DEFER_FOCUS_Q.lock_irqsave();
                if q.head == q.tail {
                    if q.dropped > 0 {
                        let d = q.dropped;
                        q.dropped = 0;
                        Some(alloc::format!(
                            "censusd: {} focus lines dropped (queue full)",
                            d
                        ))
                    } else {
                        None
                    }
                } else {
                    let h = q.head;
                    let l = q.ring[h].take();
                    q.head = (h + 1) % DEFER_FOCUS_MAX;
                    l
                }
            };
            match line {
                // 97th: KERN_WARNING — see the A-focus drain note above.
                Some(l) => crate::printk::printk_ln(
                    crate::printk::loglevel::KERN_WARNING,
                    format_args!("{}", l),
                ),
                None => break,
            }
        }
        loop {
            let line = {
                let mut q = DEFER_Q.lock_irqsave();
                if q.head == q.tail {
                    if q.dropped > 0 {
                        let d = q.dropped;
                        q.dropped = 0;
                        Some(alloc::format!(
                            "censusd: {} deferred lines dropped (queue full)",
                            d
                        ))
                    } else {
                        None
                    }
                } else {
                    let h = q.head;
                    let l = q.ring[h].take();
                    q.head = (h + 1) % DEFER_LINE_MAX;
                    l
                }
            };
            match line {
                // 97th: KERN_WARNING — see the A-focus drain note above.
                Some(l) => crate::printk::printk_ln(
                    crate::printk::loglevel::KERN_WARNING,
                    format_args!("{}", l),
                ),
                None => break,
            }
        }
        // Sleep 250 ms (jiffies discipline as mmprofd).
        let pid = match crate::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return 0,
        };
        let wake_at = crate::timer::get_jiffies() + 25;
        let timer_id = crate::timer::add_timer_wakeup(wake_at, pid);
        // Re-derived borrow (never held across schedule() below).
        crate::sched::hotpath::current().map(|t| t.set_state(TaskState::new(TaskState::INTERRUPTIBLE)));
        crate::arch::cpu::restore_irq(true);
        crate::sched::hotpath::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }
    }
    0
}

/// Early-cadence wrapper for the S-state sleep snapshot (53rd): the
/// khungtaskd cadence (60 s) sees a 7.4 s park only at 61 s; this runs
/// the same snapshot every ~5 s for the first ~60 s of boot so the
/// park-time kernel stack is captured while the boot is still young.
/// Self-gated on rux:sleepsnap (retired debug=sleepsnap) inside sleep_snapshot itself.
pub fn sleep_snapshot_early() {
    use core::sync::atomic::Ordering;
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let now = crate::timer::get_jiffies() as u64;
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 500 || N.load(Ordering::Relaxed) >= 12 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    N.fetch_add(1, Ordering::Relaxed);
    crate::api::ipc::sleep_snapshot();
    // 53rd drain hunt: park-side AF_UNIX queue/wait census on the same
    // cadence (self-gated on rux:sleepsnap (retired debug=sleepsnap) inside).
    crate::api::net::unix_park_audit();
    // 53rd timer-drive hunt: armed timerfd wheel entries + waiter census
    // (self-gated on rux:sleepsnap (retired debug=sleepsnap) inside).
    crate::syscall::support::timerfd_wheel_audit();
}

/// pid=1 RUNNING-spin audit (69th, rux:pid1spin (retired debug=pid1spin)): ohw75 proved the
/// 105-family's dead leg is init itself going 100%-CPU RUNNING and
/// silent (no LoopEvent prints, no param reads) from t=34.7 to the end
/// of the boot while the rest of the system stays alive. This audit
/// samples pid 1 on the heartbeat cadence: whenever it finds the init
/// task on-CPU RUNNING, it prints the kernel-entry frame at the top of
/// its kernel stack — the user RIP (inside the spin loop for a pure
/// userspace spin; at the syscall instruction for a syscall loop) and
/// the raw syscall number/args. Two consecutive audits landing on the
/// same RIP at >2s distance name a steady spin; the first sample after
/// a stall onset usually lands mid-loop and already symbolizes.
/// Rate-capped (first 40 audits, then every 10th); zero cost when off.
pub fn pid1spin_audit() {
    use core::sync::atomic::Ordering;
    if !crate::trace::EVENT_PID1SPIN.armed() {
        return;
    }
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    static PREV_RIP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies() as u64;
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 200 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let n = N.fetch_add(1, Ordering::Relaxed);
    // Every 10th audit also dumps init's epoll interests with live
    // per-child readiness — the discriminator between a purged
    // interest, a stuck EPOLLOUT-only mask, and a scan divergence
    // (see pid1_epoll_interest_dump).
    if n % 10 == 0 {
        crate::syscall::support::pid1_epoll_interest_dump(1);
    }
    // Short diagnostic read of init (pid 1) — TaskBorrow's lookup
    // contract; reads only task fields + its mapped kernel stack
    // (sleep_snapshot contract, sc261).
    let tk = match crate::process::types::TaskBorrow::lookup(1) {
        Some(t) => t,
        None => return,
    };
    let running = tk.state().is_running();
    let on_cpu = tk.on_cpu();
    if !running && !on_cpu {
        // Reset the steady-spin tracker when init sleeps normally.
        PREV_RIP.store(0, Ordering::Relaxed);
        return;
    }
    if n >= 40 && n % 10 != 0 {
        return;
    }
    let (sys, rip, a0, a1, a2) = match tk.pt_regs_ref() {
        None => (0u64, 0u64, 0u64, 0u64, 0u64),
        Some(pr) => {
            #[cfg(feature = "x86_64")]
            {
                (pr.orig_rax as u64, pr.rip as u64, pr.rdi as u64, pr.rsi as u64, pr.rdx as u64)
            }
            #[cfg(feature = "riscv64")]
            {
                (pr.a7 as u64, pr.epc as u64, pr.a0 as u64, pr.a1 as u64, pr.a2 as u64)
            }
            #[cfg(feature = "aarch64")]
            {
                (pr.syscallno as u64, pr.pc as u64, pr.regs[0] as u64, pr.regs[1] as u64, pr.regs[2] as u64)
            }
        }
    };
    let prev = PREV_RIP.swap(rip, Ordering::Relaxed);
    let steady = prev != 0 && prev == rip;
    crate::trace::trace_pid1spin(0, 1, sys, rip);
    crate::pr_warn!(
        "PID1SPIN #{} jiffies={} running={} on_cpu={} sys={} rip={:#x} a0={:#x} a1={:#x} a2={:#x}{}",
        n,
        now,
        running as u8,
        on_cpu as u8,
        sys,
        rip,
        a0,
        a1,
        a2,
        if steady { " STEADY" } else { "" }
    );
}


// ════════════════ memwatch ════════════════════════════
// Heap-leak hunting instrumentation (rux:memwatch (token debug=memwatch)).
//
// Two near-zero-cost layers, both fed from the `#[global_allocator]`:
//
// 1. Per-size-class alloc/free counters (always on while memwatch is
//    enabled) — the DELTA between dumps gives the net growth per power-
//    of-two size class, which pins the leaked object size.
// 2. Call-site histogram for the "big object" class (2048 < size <=
//    4096): the first 4 frame-pointer return addresses are hashed into a
//    fixed table. Every 2560-byte-ish allocation leaves a count at its
//    allocation site; the periodic DEBUG dump prints the hottest entries
//    with raw PCs for offline addr2line.
//
// Output rides the taskdump raw-UART path (no printk, no allocation) so
// it is safe from the timer softirq context that hosts the periodic dump.

// debug-migrate: the runtime switch state now lives in the unified
// trace framework — `rux:memwatch` IS the switch (armed via
// trace_event=, tracefs set_event, or the retired rux:memwatch (token debug=memwatch) boot
// alias). No local static: armed() reads the event directly.

/// Dealloc-refusal diagnostics (buddy_allocator.rs dealloc early-returns).
/// Every refused free permanently leaks the block's pages — a nonzero,
/// growing counter is a direct heap-exhaustion contributor.
pub static DEALLOC_OUTSIDE: AtomicUsize = AtomicUsize::new(0);
pub static DEALLOC_BIGORDER: AtomicUsize = AtomicUsize::new(0);
pub static DEALLOC_MISALIGNED: AtomicUsize = AtomicUsize::new(0);

/// Number of size-class buckets: sizes 1..=2^(NUM_BINS-1), plus overflow.
pub const NUM_BINS: usize = 13; // buckets: 1,2,4,...,4096 (>4096 clamps to last)

pub static ALLOC_BINS: [AtomicUsize; NUM_BINS] =
    [const { AtomicUsize::new(0) }; NUM_BINS];
pub static FREE_BINS: [AtomicUsize; NUM_BINS] =
    [const { AtomicUsize::new(0) }; NUM_BINS];

/// Histogram slots for allocation call sites.
pub const SITE_SLOTS: usize = 192;


static MEMWATCH_SITES: [Site; SITE_SLOTS] = [const {
    Site {
        key: AtomicUsize::new(0),
        count: AtomicUsize::new(0),
        bucket: AtomicUsize::new(0),
        size: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; SITE_SLOTS];

/// Bucket index for an allocation size (floor(log2(size)), clamped).
#[inline]
fn bucket(size: usize) -> usize {
    if size <= 1 {
        return 0;
    }
    let log2 = usize::BITS - 1 - size.leading_zeros();
    (log2 as usize).min(NUM_BINS - 1)
}

/// Whether memwatch is armed (cheaper than a Relaxed load? same — but keeps
/// the allocator prologue short and single-sourced).
#[inline]
pub fn memwatch_armed() -> bool {
    crate::trace::EVENT_MEMWATCH.armed()
}/// Realloc call-site histogram: (new_size bucket, frame chain) -> count.
/// Captured in the allocator's explicit realloc() via an 8-deep frame
/// walk (see buddy_allocator.rs) — deep enough to clear the whole
/// raw_vec/alloc growth machinery and land in the kernel caller.
static RSITES: [Site; 64] = [const {
    Site {
        key: AtomicUsize::new(0),
        count: AtomicUsize::new(0),
        bucket: AtomicUsize::new(0),
        size: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; 64];

/// Ascend the -O0 frame-pointer chain starting at `fp` (s0 of the frame to
/// record first), storing each frame's saved return address into `out`:
/// out[0] = return address of the frame at `fp`, out[1] = its caller's,
/// etc. Stops at the first fp that is not a plausible kernel stack frame
/// (canonical kernel address, 8-aligned, strictly ascending). Only live
/// frames are touched, so validated reads cannot fault.
///
/// MUST be called with the frame at `fp` live on the current call stack
/// (it is: callers pass their own s0 before doing anything else).
/// Is `fp` a frame pointer the walk may dereference? It must point at
/// READABLE kernel memory: the linear map (kernel stacks are heap
/// allocations there) or the kernel image window (boot-time static
/// stacks in .bss).
///
/// The old check (any canonical kernel address, `fp >= 0x8000...`) also
/// accepted the very top of the canonical space — 0xffffffffffffff80 is
/// canonical but sits in the unmapped guard region. A stale slot with
/// that value made the diagnostic walk itself page-fault with no fixup
/// (alloc-crash a1: kernel fault addr=0xffffffffffffff78, rip inside
/// walk_fp_chain, from the alloc_page_table memwatch hook).
/// Bounded linear-map predicate for DEBUG diagnostic walkers: the direct
/// map only covers DISCOVERED RAM (e820 descriptors, config fallback
/// before parsing). The architectural `is_linear_mapping` window is
/// 1<<46 (64 TB) on x86_64 / KERN_VIRT_SIZE on riscv64 — every byte
/// above phys_end inside that window is UNMAPPED, and a stale or
/// scribbled frame pointer landing there passed the old range-only
/// gate and page-faulted the diagnostic walk itself. The speed lane
/// double-confirmed the shape on OH: rux:memwatch (token debug=memwatch) panicked with
/// "Fatal exception in interrupt" at t=7.9s — the allocator hook runs
/// from softirq, and the fault arrives with IRQ context set. Shared
/// by memwatch's fp walk and taskdump's kernel-stack word walks so
/// every DEBUG reader agrees on what is actually dereferenceable.
pub fn kernel_va_readable(va: usize) -> bool {
    #[cfg(feature = "x86_64")]
    {
        // kernel image window first (static .bss boot stacks live here)
        if (0xffff_ffff_8000_0000..0xffff_ffff_8200_0000).contains(&va) {
            return true;
        }
        let po = crate::arch::x86_64::mm::PAGE_OFFSET;
        if va >= po {
            let phys = va - po;
            return phys < crate::mm::hotpath::max_pfn() * 4096;
        }
        false
    }
    #[cfg(feature = "riscv64")]
    {
        // The kernel image is linked inside the linear region on
        // riscv64 (PAGE_OFFSET + phys), so the RAM-bounded leg covers
        // both the image and the direct map — nothing above phys_end.
        let po = crate::arch::mm::PAGE_OFFSET;
        if va >= po {
            let phys = va.wrapping_sub(po);
            return phys < crate::mm::hotpath::max_pfn() * 4096;
        }
        false
    }
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    {
        let _ = va;
        false
    }
}

#[cfg(any(feature = "x86_64", feature = "riscv64"))]
pub fn plausible_fp(fp: u64) -> bool {
    kernel_va_readable(fp as usize)
}

#[cfg(feature = "aarch64")]
pub fn plausible_fp(fp: u64) -> bool {
    // aarch64: kernel stacks live in the TTBR1 half (linear map, vmemmap
    // or the image); both sit above 0xffff_8000_0000_0000.
    fp >= 0xffff_8000_0000_0000
        && crate::arch::mm::is_linear_mapping(fp as usize)
}

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

fn chain_key(bucket: usize, frames: &[u64; SITE_FRAMES]) -> usize {
    // keep 0 as the "empty slot" sentinel
    let mut key = bucket.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    for (i, &f) in frames.iter().enumerate() {
        key ^= (sanitized_ra(f) as usize).rotate_left((i as u32) * 13 + 3);
    }
    key
}

/// Kernel-image return-address window for chain keying (x86_64).
///
/// The fp walk stores the ra of every frame UNCHECKED — only the frame
/// pointer chain is validated (plausible_fp). The LAST ra of a chain
/// routinely reads the entry frame's saved junk (interrupt entry, syscall
/// entry, or simply the frame above the last real one), and that junk
/// VALUE varies call to call at the same site: pt6's dump showed one
/// caller (the anon-demand-fault family) interned as FOUR distinct psite
/// slots whose f1..f7 were byte-identical and only f8 differed
/// (0x00000011f63d4e2e / 0x00000023901eae73 / ...). At 64 slots this
/// fragmentation saturated the psite table with boot-time chains, so the
/// runtime allocators (the fake-OOM drainer among them) never got a slot
/// and MEMPSITES showed nothing for them. Keying on the sanitized chain
/// (non-kernel-text ras -> 0) makes a site's key stable across calls.
#[cfg(feature = "x86_64")]
fn sanitized_ra(ra: u64) -> u64 {
    if (0xffff_ffff_8000_0000..0xffff_ffff_8200_0000).contains(&ra) {
        ra
    } else {
        0
    }
}

/// riscv64: no sanitizer yet (kernel text lives inside the linear map, so
/// the x86_64 image-window test does not transfer). The audit boots that
/// need this are x86_64; keep the riscv64 twin's keying unchanged.
#[cfg(feature = "riscv64")]
fn sanitized_ra(ra: u64) -> u64 {
    ra
}

/// aarch64: kernel text lives in the image window (not the linear map),
/// so the x86_64-style image-window test transfers directly.
#[cfg(feature = "aarch64")]
fn sanitized_ra(ra: u64) -> u64 {
    if (0xffff_ffff_8000_0000..0xffff_ffff_8200_0000).contains(&ra) {
        ra
    } else {
        0
    }
}

/// Record one realloc observation. `frames[0]` is the return address out
/// of GlobalAlloc::realloc (into __rust_realloc); deeper entries climb
/// the growth machinery into the kernel caller.
pub fn note_realloc(new_size: usize, frames: &[u64; SITE_FRAMES]) {
    if !memwatch_armed() {
        return;
    }
    let b = bucket(new_size);
    let key = chain_key(b, frames);
    let s = &RSITES[key % 64];
    if s.key.load(Ordering::Relaxed) == key {
        s.count.fetch_add(1, Ordering::Relaxed);
    } else if s.key.load(Ordering::Relaxed) == 0 {
        if s.key
            .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            s.bucket.store(b, Ordering::Relaxed);
            s.size.store(new_size, Ordering::Relaxed);
            for i in 0..SITE_FRAMES {
                s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
            }
            s.count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Record one successful alloc observation. `frames[0]` is the caller's
/// return address captured from `ra` in the allocator prologue (before
/// any helper call can clobber it); `frames[1..]` come from the s0 chain.
/// `ptr`/`heap_start` feed the page->site table so the matching free can
/// be attributed back to THIS site (live counting).
pub fn note_alloc(ptr: *mut u8, heap_start: usize, size: usize, frames: &[u64; SITE_FRAMES]) {
    if !memwatch_armed() {
        return;
    }
    let b = bucket(size);
    ALLOC_BINS[b].fetch_add(1, Ordering::Relaxed);

    // alloc-crash r12-16: ring of the most recent BIG allocations. The
    // live-site table saturates its 250 slots within the first minutes of
    // an OH boot, so the >2KB class (b12, everything clamped) can hold
    // hundreds of MB invisibly — net=2631 live objects at the a3 death
    // while livebins showed 14. The ring is append-only and never
    // saturates: the last 32 allocations > 64KB are always nameable.
    if size > 65536 {
        let pos = BIG_RING_POS.fetch_add(1, Ordering::Relaxed) % 32;
        BIG_RING[pos].size.store(size, Ordering::Relaxed);
        BIG_RING[pos].ptr.store(ptr as usize, Ordering::Relaxed);
        BIG_RING[pos].pid.store(crate::sched::hotpath::get_current_pid() as usize, Ordering::Relaxed);
        for i in 0..4.min(frames.len()) {
            BIG_RING[pos].frames[i].store(frames[i] as usize, Ordering::Relaxed);
        }
    }

    if b >= 7 {
        let key = chain_key(b, frames);
        let s = &MEMWATCH_SITES[key % SITE_SLOTS];
        if s.key.load(Ordering::Relaxed) == key {
            s.count.fetch_add(1, Ordering::Relaxed);
        } else if s.key.load(Ordering::Relaxed) == 0 {
            // claim the empty slot; a lost race only mis-files a count
            if s.key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                s.bucket.store(b, Ordering::Relaxed);
                s.size.store(size, Ordering::Relaxed);
                for i in 0..SITE_FRAMES {
                    s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
                }
                s.count.fetch_add(1, Ordering::Relaxed);
            }
        }
        // else: hash collision — count is dropped (diagnostic only)
    }

    // Live accounting: mark every heap page of this block with the site id
    // so the later free() of the block decrements the RIGHT site. Hunt-v3:
    // threshold lowered from b>=7 to b>=3. The buddy heap is PAGE-granular
    // (a 9-byte Box<[u8]> pins a whole 4096B block), so SMALL-object leaks
    // dominate real heap growth — the per-exec exe_path leak was a b3
    // (8..15-byte) site invisible under the old >=128B tracking. b0..b2
    // (1..4-byte) churn stays excluded to keep the 250-slot table from
    // saturating at boot (see LIVE_SLOTS comment).
    if b >= 3 {
        let sid = live_intern(b, size, frames);
        if sid != 0 {
            LIVE_SITES[sid - 1].allocs.fetch_add(1, Ordering::Relaxed);
            LIVE_BINS[b].fetch_add(1, Ordering::Relaxed);
            mark_pages(ptr as usize, heap_start, size, sid, true);
        }
    }
}

/// Live count per size class (tracked b>=7 blocks only).
pub static LIVE_BINS: [AtomicUsize; NUM_BINS] =
    [const { AtomicUsize::new(0) }; NUM_BINS];

// ---------------------------------------------------------------------------
// Big-allocation ring (alloc-crash r12-16): the last 32 allocations
// larger than 64 KiB, with the first 4 frame-chain entries. See the
// note_alloc comment for why the live-site table alone cannot see them.
// ---------------------------------------------------------------------------
// BigRingSlot — the definition moved to debug/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.

pub static BIG_RING: [BigRingSlot; 32] = [const {
    BigRingSlot {
        size: AtomicUsize::new(0),
        ptr: AtomicUsize::new(0),
        pid: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; 4],
    }
}; 32];
static BIG_RING_POS: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Live-object accounting (page -> site-id table)
//
// The histograms above count EVENTS, which cannot distinguish a 5246x/s
// alloc/free churn site from a 4.6x/s never-freed leak. Here every heap
// page carries the id of the site that allocated its block; on free the
// site's live counter drops. The dump then prints `live=` per site —
// a monotonic `live` growth IS the leak, regardless of churn.
// ---------------------------------------------------------------------------

/// Heap pages tracked by the page->site live table. Sized for the FULL
/// kernel heap window (Kernel.toml kernel_heap_size=1024 → 1 GiB / 4 KiB
/// = 262144 pages); the early-boot image mapping covers KERNEL_SIZE=64MB
/// and .bss ends well under it, so the 256 KiB static array boots fine.
/// out-of-range indices are simply not tracked.
///
/// alloc-crash r12-16: with the old 128MB table (32768 pages) every
/// allocation landing past the 128MB mark interned a site and bumped
/// allocs while its free could never be attributed back (PAGE_SITE had
/// no entry) — the MEMLIVE dump showed the entire late boot as
/// "leaking" and the table was useless exactly when the heap fills.
pub const TRACKED_PAGES: usize = 262144;
pub static PAGE_SITE: [AtomicU8; TRACKED_PAGES] = [const { AtomicU8::new(0) }; TRACKED_PAGES];

/// Extended per-site live counters. Index+1 is the site id stored in
/// PAGE_SITE (0 = untracked). LIVE_SLOTS <= 255.
///
/// Only classes >= 128B are interned: tiny-class churn interns thousands
/// of distinct chains within seconds and saturates the table before the
/// leak class ever gets a slot (observed: live table frozen at boot-time
/// entries while b8 leaked unnoticed).
pub const LIVE_SLOTS: usize = 250;

pub static LIVE_SITES: [LiveSite; LIVE_SLOTS] = [const {
    LiveSite {
        key: AtomicUsize::new(0),
        allocs: AtomicUsize::new(0),
        frees: AtomicUsize::new(0),
        size: AtomicUsize::new(0),
        bucket: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; LIVE_SLOTS];

/// Map a chain to a stable live-site id (1..=LIVE_SLOTS), or 0 if the
/// table is full / disabled. Same relaxed-races-are-fine discipline as
/// the histogram tables above.
fn live_intern(bucket: usize, size: usize, frames: &[u64; SITE_FRAMES]) -> usize {
    let key = chain_key(bucket, frames);
    let h = key % LIVE_SLOTS;
    // Check the home slot and one neighbor: linear probing keeps distinct
    // chains distinct without a full scan.
    for off in 0..2 {
        let s = &LIVE_SITES[(h + off) % LIVE_SLOTS];
        let k = s.key.load(Ordering::Relaxed);
        if k == key {
            s.size.store(size, Ordering::Relaxed);
            return (h + off) % LIVE_SLOTS + 1;
        }
        if k == 0 {
            if s.key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                s.bucket.store(bucket, Ordering::Relaxed);
                s.size.store(size, Ordering::Relaxed);
                for i in 0..SITE_FRAMES {
                    s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
                }
                return (h + off) % LIVE_SLOTS + 1;
            }
            // lost the race: re-read below
            if s.key.load(Ordering::Relaxed) == key {
                return (h + off) % LIVE_SLOTS + 1;
            }
        }
    }
    0
}

/// Set/clear the site id on every page covered by [ptr, ptr+size).
fn mark_pages(ptr: usize, heap_start: usize, size: usize, sid: usize, set: bool) {
    if heap_start == 0 || ptr < heap_start {
        return;
    }
    let first = (ptr - heap_start) / 4096;
    if first >= TRACKED_PAGES {
        return;
    }
    // span the whole order-rounded block: the buddy allocator hands out
    // 2^order pages; without rounding, a 2560B alloc marks 1 page but the
    // free below would try to clear by size again — consistent either way,
    // so plain page-count from size rounded up is enough.
    let pages = (size + 4095) / 4096;
    let mut i = first;
    while i < first + pages && i < TRACKED_PAGES {
        if set {
            PAGE_SITE[i].store(sid as u8, Ordering::Relaxed);
        } else {
            PAGE_SITE[i].store(0, Ordering::Relaxed);
        }
        i += 1;
    }
}

/// Attribute one free to the site that allocated the block.
pub fn note_free(ptr: *mut u8, heap_start: usize, size: usize) {
    if !memwatch_armed() {
        return;
    }
    FREE_BINS[bucket(size)].fetch_add(1, Ordering::Relaxed);
    if heap_start == 0 || (ptr as usize) < heap_start {
        return;
    }
    let first = (ptr as usize - heap_start) / 4096;
    if first >= TRACKED_PAGES {
        return;
    }
    let sid = PAGE_SITE[first].load(Ordering::Relaxed);
    if sid != 0 && sid as usize <= LIVE_SLOTS {
        mark_pages(ptr as usize, heap_start, size, sid as usize, false);
        let s = &LIVE_SITES[sid as usize - 1];
        s.frees.fetch_add(1, Ordering::Relaxed);
        let b = s.bucket.load(Ordering::Relaxed);
        if b < NUM_BINS {
            LIVE_BINS[b].fetch_sub(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Physical-page accounting (alloc_pages/free_pages funnel + alloc_page_table)
//
// The exec-leak signature (heap flat, page_desc used_pages +2.3MB/s) lives
// BELOW the GlobalAlloc heap: whole 4K pages. Same discipline as the heap
// live table: intern the allocation call-site chain, stamp every PFN of the
// block with the site id, un-stamp + count on free. A monotonic net growth
// per site in the dump IS the page leak.
//
// The PFN->site table is 512KB and CANNOT be static: the early-boot page
// tables map the kernel image only up to the 8MB mark and .bss sits within
// ~23KB of it (a static table panics the boot with a pfault at
// 0xffffffff80800000). It is lazily carved out of the kernel heap on first
// use instead.
// ---------------------------------------------------------------------------

/// 6GB / 4KB pages (x86_64 q35 DRAM spans 0x0-0x7ffcd000 plus the
/// above-4G half 0x100000000-0x180000000; a 2 GiB window left every
/// high-RAM free unattributed — leaks measured right, churn sites'
/// nets inflated by exactly their high-RAM half). PFNs outside are
/// untracked. On riscv64 (qemu virt, DRAM at 0x80000000, 2 GiB max)
/// the extra tail is simply never indexed.
pub const PPAGE_TABLE_ENTRIES: usize = 786432;

static PPAGE_SITE_PTR: AtomicUsize = AtomicUsize::new(0); // 0=none, 1=failed, else ptr

fn ppage_site_table() -> *mut [AtomicU8; PPAGE_TABLE_ENTRIES] {
    let p = PPAGE_SITE_PTR.load(Ordering::Acquire);
    if p == 1 {
        return core::ptr::null_mut();
    }
    if p != 0 {
        return p as *mut [AtomicU8; PPAGE_TABLE_ENTRIES];
    }
    // First use: carve the table out of the kernel heap (the GlobalAlloc
    // heap is a pre-reserved region — this never re-enters alloc_pages).
    let mut v: alloc::vec::Vec<u8> = alloc::vec![0u8; PPAGE_TABLE_ENTRIES];
    let ptr = v.as_mut_ptr();
    match PPAGE_SITE_PTR.compare_exchange(0, ptr as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            core::mem::forget(v);
            ptr as *mut [AtomicU8; PPAGE_TABLE_ENTRIES]
        }
        Err(cur) => {
            if cur == 1 {
                core::ptr::null_mut()
            } else {
                cur as *mut [AtomicU8; PPAGE_TABLE_ENTRIES]
            }
        }
    }
}

/// 256 slots (fake-OOM audit, session 22): at 64 slots the table
/// saturated with boot-time chains before the runtime allocators — the
/// services that drain the zone never interned, and MEMPSITES showed
/// nothing for them (pt6: top-10 nets summed to ~96k against a 558k-page
/// other_used census). Combined with chain sanitization (see
/// sanitized_ra) 256 covers the real site population of an OH boot.
pub const PSITE_SLOTS: usize = 256;

/// Raw free_pages() funnel count while armed — 0 means the page free side
/// is entirely dead (no page ever returns to the zone allocator).
pub static PGFREE_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// MmStruct::drop invocations (user address spaces fully released).
pub static MM_DROPS: AtomicUsize = AtomicUsize::new(0);
/// MmStruct user-space creations — the creation-side twin of MM_DROPS
/// so a census can state live_mm = MM_NEWS - MM_DROPS directly
/// (leg-17 spawn-residue leak test on the c1024 population).
pub static MM_NEWS: AtomicUsize = AtomicUsize::new(0);
/// free_user_page_tables entries (teardown walks started).
pub static FUT_CALLS: AtomicUsize = AtomicUsize::new(0);
/// page-table pages actually returned by free_page_table_checked.
pub static FUT_TABLES: AtomicUsize = AtomicUsize::new(0);
/// data pages actually returned by the teardown walk (put_page -> 0).
pub static FUT_PAGES: AtomicUsize = AtomicUsize::new(0);

pub static PSITES: [PageSite; PSITE_SLOTS] = [const {
    PageSite {
        key: AtomicUsize::new(0),
        allocs: AtomicUsize::new(0),
        frees: AtomicUsize::new(0),
        order: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; PSITE_SLOTS];

fn psite_intern(frames: &[u64; SITE_FRAMES]) -> usize {
    let key = chain_key(0, frames);
    let h = key % PSITE_SLOTS;
    for off in 0..2 {
        let s = &PSITES[(h + off) % PSITE_SLOTS];
        let k = s.key.load(Ordering::Relaxed);
        if k == key {
            return (h + off) % PSITE_SLOTS + 1;
        }
        if k == 0 {
            if s.key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                for i in 0..SITE_FRAMES {
                    s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
                }
                return (h + off) % PSITE_SLOTS + 1;
            }
            if s.key.load(Ordering::Relaxed) == key {
                return (h + off) % PSITE_SLOTS + 1;
            }
        }
    }
    0
}

/// Record one physical-page allocation (order-sized block).
pub fn note_page_alloc(phys: usize, order: usize, frames: &[u64; SITE_FRAMES]) {
    if !memwatch_armed() || phys == 0 {
        return;
    }
    let table = ppage_site_table();
    if table.is_null() {
        return;
    }
    let sid = psite_intern(frames);
    if sid == 0 {
        return;
    }
    PSITES[sid - 1].allocs.fetch_add(1, Ordering::Relaxed);
    PSITES[sid - 1].order.store(order, Ordering::Relaxed);
    let first = tracked_index(phys);
    if first == usize::MAX {
        return;
    }
    let pages = 1usize << order;
    let mut i = first;
    while i < first + pages && i < PPAGE_TABLE_ENTRIES {
        unsafe {
            (*table)[i].store(sid as u8, Ordering::Relaxed);
        }
        i += 1;
    }
}

/// Record one physical-page free: attribute it back to the site that
/// stamped the block's first PFN.
pub fn note_page_free(phys: usize, order: usize) {
    if !memwatch_armed() || phys == 0 {
        return;
    }
    PGFREE_TOTAL.fetch_add(1, Ordering::Relaxed);
    let first = tracked_index(phys);
    if first == usize::MAX {
        return;
    }
    let table = ppage_site_table();
    if table.is_null() {
        return;
    }
    let sid = unsafe { (*table)[first].load(Ordering::Relaxed) };
    if sid == 0 || sid as usize > PSITE_SLOTS {
        return;
    }
    let pages = 1usize << order;
    let mut i = first;
    while i < first + pages && i < PPAGE_TABLE_ENTRIES {
        unsafe {
            (*table)[i].store(0, Ordering::Relaxed);
        }
        i += 1;
    }
    PSITES[sid as usize - 1].frees.fetch_add(1, Ordering::Relaxed);
}

/// DRAM-relative page index for the PFN->site table. Physical addresses
/// are absolute (0x800000000+ on qemu virt); the table indexes from the
/// DRAM base. usize::MAX = outside tracked DRAM.
fn tracked_index(phys: usize) -> usize {
    let base = if crate::mm::hotpath::is_kernel_layout_initialized() {
        crate::mm::hotpath::kernel_layout().phys_base
    } else {
        0x8000_0000
    };
    if phys < base {
        return usize::MAX;
    }
    let idx = (phys - base) / 4096;
    // The PFN->site table is a fixed 2 GiB window (PPAGE_TABLE_ENTRIES).
    // On x86_64 q35 DRAM spans 0x0-0x7ffcd000 + 0x100000000-0x180000000,
    // so a PFN past 2 GiB (high RAM) is outside the tracked range; an
    // unguarded return made note_page_free's `table[first]` load panic
    // ("index out of bounds: len 524288, index 1572173", boot u5) the
    // moment a high-RAM page was freed (rux:memwatch (token debug=memwatch) boots died ~180
    // log lines in).
    if idx >= PPAGE_TABLE_ENTRIES {
        return usize::MAX;
    }
    idx
}

fn memwatch_put_dec(v: u64) {
    taskdump_dec(v);
}

/// Print the kernel root's low-half (vpn2[0..1]) non-leaf population:
/// `l1` non-leaf PGD entries and `l0` non-leaf L1 entries beneath them.
/// Read-only walk; diagnostic noise under concurrency is acceptable.
fn kroot_pollution() {
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

fn memwatch_put_hex(v: u64) {
    taskdump_raw_line(b"0x");
    let mut shift = 64;
    while shift > 0 {
        shift -= 4;
        let nib = (v >> shift) & 0xF;
        let c = if nib < 10 {
            b'0' + nib as u8
        } else {
            b'a' + (nib - 10) as u8
        };
        taskdump_raw_line(&[c]);
    }
}

/// One MEM line + the bin/site tables for the periodic DEBUG dump.
/// Raw UART, allocation-free, safe from softirq context.
pub fn dump_mem() {
    taskdump_raw_line(b"\nMEM heap ");
        // r-bwslow census (bw_tcp zero-window deadlock hunt): every tcp
        // slot's state / recv_buffer len / advertised window, so a stuck
        // boot names the deadlock side (data-at-client-unread vs
        // data-never-arrived) in one line.
        {
            taskdump_raw_line(b"\nTCPCEN ");
            let mut n = 0;
            for row in crate::api::net::tcp_census_rows() {
                taskdump_raw_line(b"[");
                taskdump_dec(row.fd as u64);
                taskdump_raw_line(b":st=");
                taskdump_dec(row.state as u64);
                taskdump_raw_line(b",rx=");
                taskdump_dec(row.rx_len);
                taskdump_raw_line(b",wnd=");
                taskdump_dec(row.rcv_wnd);
                taskdump_raw_line(b",lp=");
                taskdump_dec(row.local_port.to_be() as u64);
                taskdump_raw_line(b"] ");
                n += 1;
                if n >= 8 { break; }
            }
        }
        // r-strand instrument (lat_fs conviction round): the sync-wait
        // phase counts on this proven raw path (pr_warn in the timer
        // softirq was dropped by printk on loaded boots).
        taskdump_raw_line(b"\nVWWAIT enter=");
        taskdump_dec(crate::drivers::hotpath::VW_WAIT_ENTER.load(core::sync::atomic::Ordering::Relaxed));
        taskdump_raw_line(b" sched=");
        taskdump_dec(crate::drivers::hotpath::VW_WAIT_SCHED.load(core::sync::atomic::Ordering::Relaxed));
        taskdump_raw_line(b" resp=");
        taskdump_dec(crate::drivers::hotpath::VW_WAIT_RESP.load(core::sync::atomic::Ordering::Relaxed));
        taskdump_raw_line(b" timeout=");
        taskdump_dec(crate::drivers::hotpath::VW_WAIT_TIMEOUT.load(core::sync::atomic::Ordering::Relaxed));
    let st = crate::mm::hotpath::buddy_stats();
    memwatch_put_dec((st.used_bytes / 1024) as u64);
    taskdump_raw_line(b"K/");
    memwatch_put_dec((st.heap_size / 1024) as u64);
    // buddy_stats() does not maintain counters; use memwatch totals when
    // the switch is on (they started at debug init, close enough for rates)
    let (mut at, mut ft) = (0u64, 0u64);
    if memwatch_armed() {
        for i in 0..NUM_BINS {
            at += ALLOC_BINS[i].load(Ordering::Relaxed) as u64;
            ft += FREE_BINS[i].load(Ordering::Relaxed) as u64;
        }
    }
    taskdump_raw_line(b"K a=");
    memwatch_put_dec(at);
    taskdump_raw_line(b" f=");
    memwatch_put_dec(ft);
    let sl = crate::mm::hotpath::slab_stats();
    let mut sa = 0usize;
    let mut sf = 0usize;
    for c in sl.cache_stats.iter() {
        sa += c.alloc_count;
        sf += c.free_count;
    }
    taskdump_raw_line(b" | slab ");
    memwatch_put_dec(sl.total_pages as u64);
    taskdump_raw_line(b"pg a=");
    memwatch_put_dec(sa as u64);
    taskdump_raw_line(b" f=");
    memwatch_put_dec(sf as u64);
    let pg = crate::mm::hotpath::page_desc_stats();
    taskdump_raw_line(b" | pages u=");
    memwatch_put_dec(pg.used_pages as u64);
    taskdump_raw_line(b" r=");
    memwatch_put_dec(pg.reserved_pages as u64);
    // Dealloc-refusal diagnostics: each is a silent page leak.
    taskdump_raw_line(b" | dref out=");
    memwatch_put_dec(DEALLOC_OUTSIDE.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" big=");
    memwatch_put_dec(DEALLOC_BIGORDER.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" mis=");
    memwatch_put_dec(DEALLOC_MISALIGNED.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b"\n");

    if !memwatch_armed() {
        return;
    }
    // debug-migrate: the report totals also record into the unified
    // framework (rux:memwatch — the event is also the family's arm
    // switch). record() is atomics-only, safe from this raw-UART
    // context; the render happens at read time.
    crate::trace::trace_memwatch(
        (st.used_bytes / 1024) as u64,
        at / 1024,
        ft / 1024,
        (DEALLOC_OUTSIDE.load(Ordering::Relaxed)
            + DEALLOC_BIGORDER.load(Ordering::Relaxed)
            + DEALLOC_MISALIGNED.load(Ordering::Relaxed)) as u64,
    );
    dump_heap_ledger();
    dump_page_ledger();
}

/// The heap-side ledger tables ONLY: size-class net growth (MEMBINS),
/// hottest realloc sites, and the live-per-call-site table (MEMLIVE).
///
/// Pure atomics — no allocator locks, no allocation, no page-table walks,
/// no lazily-carved tables — so it is safe to call from the exhausted
/// heap path itself (alloc_error_handler / ALLOCTHROW), where taking the
/// buddy lock or allocating a single byte would wedge the very print we
/// die on.
pub fn dump_heap_ledger() {
    if !memwatch_armed() {
        return;
    }

    // net growth per size class since boot (allocs - frees)
    taskdump_raw_line(b"MEMBINS class:net\n");
    for i in 0..NUM_BINS {
        let a = ALLOC_BINS[i].load(Ordering::Relaxed);
        let f = FREE_BINS[i].load(Ordering::Relaxed);
        if a == 0 && f == 0 {
            continue;
        }
        let net = a.wrapping_sub(f) as i128;
        taskdump_raw_line(b" b");
        memwatch_put_dec(i as u64);
        taskdump_raw_line(b"(size>");
        memwatch_put_dec(if i == 0 { 0 } else { 1 << (i - 1) } as u64);
        taskdump_raw_line(b"):a=");
        memwatch_put_dec(a as u64);
        taskdump_raw_line(b":f=");
        memwatch_put_dec(f as u64);
        taskdump_raw_line(b":net=");
        if net < 0 {
            taskdump_raw_line(b"-");
            memwatch_put_dec((-net) as u64);
        } else {
            memwatch_put_dec(net as u64);
        }
        taskdump_raw_line(b"\n");
    }

    // hottest realloc sites (top 6): b=<new size bucket> sz=<first-seen
    // exact size> + the 8-deep return-address chain (deepest kernel frames
    // are the leaking code; frames inside __rust_* / alloc::* / raw_vec
    // machinery come first)
    taskdump_raw_line(b"MEMRSITES cnt bucket sz f1..f8\n");
    let mut rtaken = [false; 64];
    for _ in 0..6 {
        let mut best: isize = -1;
        let mut best_cnt = 0usize;
        for i in 0..64 {
            if rtaken[i] {
                continue;
            }
            let c = RSITES[i].count.load(Ordering::Relaxed);
            let k = RSITES[i].key.load(Ordering::Relaxed);
            if k != 0 && c > best_cnt {
                best_cnt = c;
                best = i as isize;
            }
        }
        if best < 0 {
            break;
        }
        let i = best as usize;
        rtaken[i] = true;
        taskdump_raw_line(b" rsite cnt=");
        memwatch_put_dec(best_cnt as u64);
        taskdump_raw_line(b" b=");
        memwatch_put_dec(RSITES[i].bucket.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" sz=");
        memwatch_put_dec(RSITES[i].size.load(Ordering::Relaxed) as u64);
        for fr in RSITES[i].frames.iter() {
            taskdump_raw_line(b" ");
            memwatch_put_hex(fr.load(Ordering::Relaxed) as u64);
        }
        taskdump_raw_line(b"\n");
    }

    // LIVE sites, sorted by live count (top 12): live=allocs-frees per
    // call-site chain. A monotonically growing live value IS the leak;
    // churn-heavy sites show live ~0 despite huge alloc counts.
    taskdump_raw_line(b"MEMLIVE live allocs frees bucket sz f1..f12\n");
    taskdump_raw_line(b" livebins");
    for i in 0..NUM_BINS {
        if LIVE_BINS[i].load(Ordering::Relaxed) == 0 {
            continue;
        }
        taskdump_raw_line(b" b");
        memwatch_put_dec(i as u64);
        taskdump_raw_line(b"=");
        memwatch_put_dec(LIVE_BINS[i].load(Ordering::Relaxed) as u64);
    }
    taskdump_raw_line(b"\n");
    let mut ltaken = [false; LIVE_SLOTS];
    for _ in 0..12 {
        let mut best: isize = -1;
        let mut best_live: i64 = -1;
        for i in 0..LIVE_SLOTS {
            if ltaken[i] {
                continue;
            }
            let k = LIVE_SITES[i].key.load(Ordering::Relaxed);
            if k == 0 {
                continue;
            }
            let live = LIVE_SITES[i].allocs.load(Ordering::Relaxed) as i64
                - LIVE_SITES[i].frees.load(Ordering::Relaxed) as i64;
            if live > best_live {
                best_live = live;
                best = i as isize;
            }
        }
        if best < 0 {
            break;
        }
        let i = best as usize;
        ltaken[i] = true;
        taskdump_raw_line(b" lsite live=");
        memwatch_put_dec(best_live as u64);
        taskdump_raw_line(b" a=");
        memwatch_put_dec(LIVE_SITES[i].allocs.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" f=");
        memwatch_put_dec(LIVE_SITES[i].frees.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" b=");
        memwatch_put_dec(LIVE_SITES[i].bucket.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" sz=");
        memwatch_put_dec(LIVE_SITES[i].size.load(Ordering::Relaxed) as u64);
        for fr in LIVE_SITES[i].frames.iter() {
            taskdump_raw_line(b" ");
            memwatch_put_hex(fr.load(Ordering::Relaxed) as u64);
        }
        taskdump_raw_line(b"\n");
    }

    // alloc-crash r12-16: allocator page-flow + refused frees + the big-alloc
    // ring. All plain atomics — safe on the exhausted-heap path.
    taskdump_raw_line(b"MEMFLOW net pages per order (alloc - free)\n");
    let mut net_total = 0usize;
    for o in 0..=crate::config::BUDDY_MAX_ORDER {
        let n = crate::mm::hotpath::NET_PAGES[o].load(Ordering::Relaxed);
        if n != 0 {
            taskdump_raw_line(b" o");
            memwatch_put_dec(o as u64);
            taskdump_raw_line(b"=");
            memwatch_put_dec(n as u64);
            taskdump_raw_line(b"pages(");
            memwatch_put_dec((n * 4096 / 1024) as u64);
            taskdump_raw_line(b"K) ");
            net_total += n * 4096;
        }
    }
    taskdump_raw_line(b"\n netpages=");
    memwatch_put_dec((net_total / 1024) as u64);
    taskdump_raw_line(b"K\n");
    taskdump_raw_line(b"MEMREFUSE out_of_win dblfree ordmis tasklive ordbig unaligned\n ");
    for i in 0..6 {
        memwatch_put_dec(crate::mm::hotpath::FREE_REFUSED[i].load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" ");
    }
    taskdump_raw_line(b"\n refused-ring (addr order metaord metafree):\n");
    for i in 0..16 {
        let slot = i * 4;
        let a = crate::mm::hotpath::REFUSED_RING[slot].load(Ordering::Relaxed);
        if a == 0 {
            continue;
        }
        memwatch_put_hex(a as u64);
        taskdump_raw_line(b" o");
        memwatch_put_dec(crate::mm::hotpath::REFUSED_RING[slot + 1].load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" mo");
        memwatch_put_dec(crate::mm::hotpath::REFUSED_RING[slot + 2].load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" mf");
        memwatch_put_dec(crate::mm::hotpath::REFUSED_RING[slot + 3].load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b"\n");
    }
    taskdump_raw_line(b"MEMBIG last-32 big allocs (size pid f1..f4):\n");
    for i in 0..32 {
        let sz = BIG_RING[i].size.load(Ordering::Relaxed);
        if sz == 0 {
            continue;
        }
        taskdump_raw_line(b" big sz=");
        memwatch_put_dec(sz as u64);
        taskdump_raw_line(b" pid=");
        memwatch_put_dec(BIG_RING[i].pid.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" ptr=");
        memwatch_put_hex(BIG_RING[i].ptr.load(Ordering::Relaxed) as u64);
        for f in 0..4 {
            taskdump_raw_line(b" ");
            memwatch_put_hex(BIG_RING[i].frames[f].load(Ordering::Relaxed) as u64);
        }
        taskdump_raw_line(b"\n");
    }
}

/// Physical-page accounting half of the periodic dump (MEMPSITES +
/// kroot pollution). Kept separate from dump_heap_ledger because it may
/// touch the lazily-carved PFN table (heap allocation) and walk page
/// tables — NOT safe on the exhausted-heap path.
fn dump_page_ledger() {
    // PHYSICAL-PAGE sites, sorted by net growth (top 10): net=allocs-frees
    // per allocation call-site chain. A monotonic net IS the page leak.
    taskdump_raw_line(b"MEMPSITES net allocs frees order f1..f12\n");
    taskdump_raw_line(b" pgfree_total=");
    memwatch_put_dec(PGFREE_TOTAL.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" mm_drops=");
    memwatch_put_dec(MM_DROPS.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" fut_calls=");
    memwatch_put_dec(FUT_CALLS.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" fut_tables=");
    memwatch_put_dec(FUT_TABLES.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" fut_pages=");
    memwatch_put_dec(FUT_PAGES.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b"\n");
    // Kernel-root low-half hygiene: valid non-leaf entries under
    // ROOT_PAGE_TABLE vpn2[0..1]. Every non-leaf L1 entry here costs one
    // L0-table copy per exec (copy_kernel_mappings), so growth in these
    // counters is the exec-leak amplifier.
    kroot_pollution();
    let mut ptaken = [false; PSITE_SLOTS];
    for _ in 0..16 {
        let mut best: isize = -1;
        let mut best_net: i64 = -1;
        for i in 0..PSITE_SLOTS {
            if ptaken[i] {
                continue;
            }
            let k = PSITES[i].key.load(Ordering::Relaxed);
            if k == 0 {
                continue;
            }
            let net = PSITES[i].allocs.load(Ordering::Relaxed) as i64
                - PSITES[i].frees.load(Ordering::Relaxed) as i64;
            if net > best_net {
                best_net = net;
                best = i as isize;
            }
        }
        if best < 0 {
            break;
        }
        let i = best as usize;
        ptaken[i] = true;
        taskdump_raw_line(b" psite net=");
        memwatch_put_dec(best_net as u64);
        taskdump_raw_line(b" a=");
        memwatch_put_dec(PSITES[i].allocs.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" f=");
        memwatch_put_dec(PSITES[i].frees.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" ord=");
        memwatch_put_dec(PSITES[i].order.load(Ordering::Relaxed) as u64);
        for fr in PSITES[i].frames.iter() {
            taskdump_raw_line(b" ");
            memwatch_put_hex(fr.load(Ordering::Relaxed) as u64);
        }
        taskdump_raw_line(b"\n");
    }
}


// ════════════════ ledger ══════════════════════════════
// Full physical-accounting ledger (fake-OOM family, rux:ledger (retired debug=ledger)).
//
// `oom_forensic_dump` answers "is the allocator's own state consistent?"
// This module answers the question one level up: WHERE DID THE RAM GO?
// Every managed page is classified into exactly one bucket (single
// descriptor pass, precedence-ordered), the buckets are summed back
// against the zone counters, and any gap is printed as an explicit
// mismatch line. At the moment of a fake OOM the ledger shows which
// category swallowed the "free" memory — anonymous, page tables, pinned
// page cache, or accounting drift (pages that are nobody's).
//
// Buckets (first match wins):
//   free_linked    refcount==0, OnFreelist          (should == zone free_cnt)
//   free_orphan    refcount==0, not linked, not reserved   <= LEAK
//   reserved       PageFlag::Reserved               (kernel image/heap/slab)
//   page_tables    PT_LEDGER live stamp             (exact bitmap)
//   anon           PageFlag::Anonymous
//   cow            PageFlag::Cow, not anonymous
//   mapped_file    mapcount>0, not anonymous        (page cache in a PTE)
//   lru_file       PageFlag::Lru, not anon/mapped   (clean page cache)
//   other_used     refcount>0, none of the above    (kernel objects,
//                  stacks, pipe/skb buffers, pinned frames, ...)
//
// Output goes through printk (println!) — like oom_forensic_dump it is
// meant for the OOM moment (task context, scheduler alive) and for the
// on-demand UART magic "LEDGER!" (deferred to the timer softirq, which
// already hosts the periodic dumps). It must NOT be called while the
// caller holds the zone lock (the census only reads descriptors, but a
// printk under the zone lock is the classic self-wedge).
//
// Zero heap allocation: counters are stack locals, and the compact line
// rides the raw-UART taskdump writer with a fixed stack buffer (the
// kernel heap cannot be trusted at the moment this dump exists for).

/// Set by the UART magic ("LEDGER!"); consumed by the timer softirq.
pub static PENDING: AtomicBool = AtomicBool::new(false);

/// Decimal -> fixed stack buffer; returns the written slice.
fn dec_buf(n: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    let mut n = n;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    &buf[i..]
}


impl Line {
    fn new(head: &[u8]) -> Self {
        let mut l = Line { buf: [0; 160], len: 0 };
        l.push(head);
        l
    }
    fn push(&mut self, s: &[u8]) {
        // len is capped at buf.len()-1, so `room` never underflows and
        // buf[len] stays in bounds for emit's newline store.
        if self.len >= self.buf.len() - 1 {
            return;
        }
        let room = self.buf.len() - 1 - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
        self.len += n;
    }
    fn dec(&mut self, n: u64) {
        let mut b = [0u8; 20];
        self.push(dec_buf(n, &mut b));
    }
    fn emit(self) {
        let mut me = self;
        me.buf[me.len] = b'\n';
        let end = me.len + 1;
        taskdump_raw_line(&me.buf[..end]);
    }
}

/// O(1) counters on the raw-UART path (printk-free, safe from any
/// context): zone free, PT pages, page cache, heap, slab, swap.
pub fn compact_line() {
    let mut l = Line::new(b"MEMLEDGER-COMPACT");
    if let Some(node) = crate::mm::hotpath::first_online_node() {
        l.push(b" memfree_pages=");
        l.dec(node.free_pages() as u64);
    }
    #[cfg(any(feature = "riscv64", feature = "x86_64"))]
    {
        l.push(b" pt_pages=");
        l.dec(crate::arch::mm::PtLedger::live_count() as u64);
    }
    l.push(b" pagecache_pages=");
    l.dec(crate::fs::hotpath::page_cache_total_pages() as u64);
    let heap = crate::mm::hotpath::buddy_stats();
    l.push(b" heapK=");
    l.dec((heap.used_bytes / 1024) as u64);
    l.push(b"/");
    l.dec((heap.heap_size / 1024) as u64);
    let slab = crate::mm::hotpath::slab_stats();
    l.push(b" slab_pages=");
    l.dec(slab.total_pages as u64);
    l.push(b" swap_active=");
    l.dec(crate::api::mm::nr_active_swap() as u64);
    l.emit();
}

/// Raw-UART census line (printk-free): the page-descriptor class counts
/// the compact line lacks. One extra O(nr_descs) walk — the dump path
/// already does one; this is the printk-wedge-proof copy of the numbers
/// the fake-OOM audit needs (anon vs pinned cache vs reserved). Emitted
/// by dump() next to compact_line(), and safe to call standalone.
pub fn census_line() {
    let st = crate::mm::hotpath::page_desc_stats();
    let mut l = Line::new(b"MEMLEDGER-CENSUS");
    l.push(b" total=");
    l.dec(st.total_pages as u64);
    l.push(b" used=");
    l.dec(st.used_pages as u64);
    l.push(b" free=");
    l.dec(st.free_pages as u64);
    l.push(b" reserved=");
    l.dec(st.reserved_pages as u64);
    l.push(b" anon=");
    l.dec(st.anonymous_pages as u64);
    l.push(b" cow=");
    l.dec(st.cow_pages as u64);
    l.push(b" mapped=");
    l.dec(st.mapped_pages as u64);
    l.push(b" dirty=");
    l.dec(st.dirty_pages as u64);
    l.emit();
}

/// Raw-UART one-liner for the page-allocator OOM forensic summary (the
/// printk copies above can be lost to a printk wedge — see oom_forensic_dump).
pub fn oomfx_line(
    zone_free: u64,
    zone_walkable: u64,
    zone_managed: u64,
    orphan: u64,
    inverse: u64,
    seed_alarms: u64,
    double_free: u64,
) {
    let mut l = Line::new(b"OOMFX-RAW");
    l.push(b" zone_free=");
    l.dec(zone_free);
    l.push(b" zone_walkable=");
    l.dec(zone_walkable);
    l.push(b" zone_managed=");
    l.dec(zone_managed);
    l.push(b" orphan=");
    l.dec(orphan);
    l.push(b" inverse=");
    l.dec(inverse);
    l.push(b" seed_alarms=");
    l.dec(seed_alarms);
    l.push(b" dblfree=");
    l.dec(double_free);
    l.emit();
}


/// The full census + reconciliation. printk-based; see the module doc for
/// context restrictions. `site` names the trigger (OOM moment, UART magic).
pub fn dump(site: &str) {
    // One dump at a time (the timer-softirq trigger and an OOM can race;
    // interleaved ledgers are worse than a dropped one).
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    crate::println!("==== MEMLEDGER @ {} ====", site);
    compact_line();
    census_line();

    // Zone counters (what the allocator believes).
    let mut zone_free: u64 = 0;
    let mut zone_managed: u64 = 0;
    if let Some(node) = crate::mm::hotpath::first_online_node() {
        for zt in [
            crate::mm::types::ZoneType::ZoneNormal,
            crate::mm::types::ZoneType::ZoneDma32,
            crate::mm::types::ZoneType::ZoneDma,
            crate::mm::types::ZoneType::ZoneMovable,
        ] {
            if let Some(zone) = node.zone(zt) {
                if !zone.is_initialized() {
                    continue;
                }
                zone_free += zone.nr_free() as u64;
                zone_managed += zone.managed_pages() as u64;
                crate::println!(
                    "MEMLEDGER zone {} free_cnt={} managed={} wmark={}/{}/{}",
                    zone.zone_type().name(),
                    zone.nr_free(),
                    zone.managed_pages(),
                    zone.watermark(crate::mm::types::WMARK_MIN),
                    zone.watermark(crate::mm::types::WMARK_LOW),
                    zone.watermark(crate::mm::types::WMARK_HIGH),
                );
            }
        }
        // LRU sizes (counter view; the census below walks the pages).
        for lru_i in 0..crate::mm::types::NR_LRU_LISTS {
            let sz = node.lru_sizes[lru_i].load(Ordering::Relaxed);
            if sz > 0 {
                crate::println!("MEMLEDGER lru[{}] size={}", lru_i, sz);
            }
        }
    }

    // Full descriptor census (bounded by what vmemmap actually mapped —
    // same dereference-safety rule as page_desc_stats).
    use crate::mm::{hotpath::nr_descs, hotpath::pfn_to_page, types::PageFlag, hotpath::MIN_PFN};
    let scan = nr_descs().min(crate::mm::hotpath::vmemmap_stats().nr_pages);
    #[cfg(any(feature = "riscv64", feature = "x86_64"))]
    let pt_count = crate::arch::mm::PtLedger::live_count() as u64;
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    let pt_count = 0u64;

    let mut c = Census::default();
    // other_used physical-range histogram (fake-OOM audit, session 22):
    // the OOM-moment dumps show other_used at 558600/736006 managed pages
    // (pt5, ~76% of the zone) while the psite table attributes under 100k
    // — WHERE the unflagged pages physically sit (low RAM vs the above-4G
    // half vs the kernel-heap neighborhood) discriminates funnel families
    // before the psite widening lands. 12 buckets x 512 MiB over 0..6G.
    const OW_BUCKETS: usize = 12;
    let mut ow_ranges = [0u64; OW_BUCKETS];
    for i in 0..scan {
        let page = pfn_to_page(MIN_PFN + i);
        if page.is_null() {
            continue;
        }
        // SAFETY: read-only descriptor access inside the mapped vmemmap
        // span; the flags atomics are the same ones every other reader
        // uses lock-free.
        let (rc, linked, reserved) = unsafe {
            (
                (*page).refcount(),
                (*page).test_flag(PageFlag::OnFreelist),
                (*page).is_reserved(),
            )
        };
        if rc == 0 {
            if reserved {
                c.reserved += 1;
            } else if linked {
                c.free_leader += 1;
            } else {
                c.free_member += 1;
            }
            continue;
        }
        if rc == 1 {
            c.rc_eq1 += 1;
        } else if rc <= 8 {
            c.rc_2_8 += 1;
        } else {
            c.rc_gt8 += 1;
        }
        if reserved {
            c.reserved += 1;
            continue;
        }
        #[cfg(any(feature = "riscv64", feature = "x86_64"))]
        {
            if crate::arch::mm::PT_LEDGER.peek((MIN_PFN + i) as u64) != 0 {
                c.page_tables += 1;
                continue;
            }
        }
        let (anon, cow, mapped, lru) = unsafe {
            (
                (*page).is_anonymous(),
                (*page).is_cow(),
                (*page).is_mapped(),
                (*page).test_flag(PageFlag::Lru),
            )
        };
        if anon {
            c.anon += 1;
        } else if cow {
            c.cow += 1;
        } else if mapped {
            c.mapped_file += 1;
        } else if lru {
            c.lru_file += 1;
        } else {
            c.other_used += 1;
            let phys = (MIN_PFN + i) as u64 * 4096;
            let b = (phys / (512 * 1024 * 1024)) as usize;
            if b < OW_BUCKETS {
                ow_ranges[b] += 1;
            }
        }
    }

    let mb = |p: u64| p * 4 / 1024;
    let free_census = c.free_leader + c.free_member;
    crate::println!(
        "MEMLEDGER census scan={} descs",
        scan
    );
    crate::println!(
        "MEMLEDGER   free={}L+{}M={} ({} MB) reserved={} ({} MB) page_tables={} exact={}{}",
        c.free_leader, c.free_member, free_census, mb(free_census),
        c.reserved, mb(c.reserved), c.page_tables, pt_count,
        if c.page_tables != pt_count { "  <== PT-BITMAP MISMATCH" } else { "" }
    );
    crate::println!(
        "MEMLEDGER   anon={} ({} MB) cow={} mapped_file={} lru_file={} other_used={}",
        c.anon, mb(c.anon), c.cow, c.mapped_file, c.lru_file, c.other_used
    );
    crate::println!(
        "MEMLEDGER   refcounts(rc>0): ==1:{} 2..8:{} >8:{}",
        c.rc_eq1, c.rc_2_8, c.rc_gt8
    );
    crate::println!(
        "MEMLEDGER   other_used phys ranges (512MiB buckets 0..6G): {:?}",
        ow_ranges
    );

    // Reconciliation: every descriptor is in exactly one bucket.
    let sum = free_census
        + c.reserved
        + c.page_tables
        + c.anon
        + c.cow
        + c.mapped_file
        + c.lru_file
        + c.other_used;
    crate::println!(
        "MEMLEDGER sum={} vs descs={}{}",
        sum,
        nr_descs(),
        if sum != nr_descs() as u64 { "  <== CENSUS GAP" } else { "" }
    );
    // free census vs zone counter: both count free PAGES (members of free
    // blocks included). A census EXCESS means refcount==0 pages nobody
    // owns (true orphans / leak); a counter excess means the zone believes
    // pages free whose descriptors say otherwise (inverse corruption) —
    // or transient churn while the non-atomic census ran (a few thousand
    // pages under load is race noise; a monotonic gap is not).
    crate::println!(
        "MEMLEDGER free_census={} vs zone_free_cnt={} drift={}",
        free_census,
        zone_free,
        free_census as i64 - zone_free as i64
    );
    crate::println!(
        "MEMLEDGER zone_managed={} vs descs-reserved={} (heap/image/slab live outside the zone span)",
        zone_managed,
        nr_descs() as u64 - c.reserved
    );
    // Zero-put / free-refusal attribution (freelist-drift family): which
    // release path's refcount-0 transitions did NOT pair with a zone free.
    self::report();
    crate::println!("==== MEMLEDGER end ====");

    // Raw-UART copy of the census buckets (the printk lines above are
    // lost when printk wedges — the exact storms this dump targets; the
    // pt1/pt2 fake-OOM audits recovered ONLY the raw lines). Same
    // numbers, one line, always survives.
    {
        let mut l = Line::new(b"MEMLEDGER-BUCKETS");
        l.push(b" freeL=");
        l.dec(c.free_leader);
        l.push(b" freeM=");
        l.dec(c.free_member);
        l.push(b" reserved=");
        l.dec(c.reserved);
        l.push(b" pt=");
        l.dec(c.page_tables);
        l.push(b" anon=");
        l.dec(c.anon);
        l.push(b" cow=");
        l.dec(c.cow);
        l.push(b" mapped_file=");
        l.dec(c.mapped_file);
        l.push(b" lru_file=");
        l.dec(c.lru_file);
        l.push(b" other_used=");
        l.dec(c.other_used);
        l.push(b" rc1=");
        l.dec(c.rc_eq1);
        l.push(b" rc2_8=");
        l.dec(c.rc_2_8);
        l.push(b" rc>8=");
        l.dec(c.rc_gt8);
        l.emit();
        let mut l = Line::new(b"MEMLEDGER-OWRS");
        // Descending bucket order: the above-4G half (r8..r11) is the
        // funnel discriminator this line exists for — print it first so a
        // pathological spread truncates the LOW tail, never the high one.
        for bi in (0..OW_BUCKETS).rev() {
            let v = ow_ranges[bi];
            if v == 0 {
                continue;
            }
            l.push(b" r");
            l.dec(bi as u64);
            l.push(b"=");
            l.dec(v);
        }
        l.emit();
    }

    BUSY.store(false, Ordering::Release);
}

/// Deferred trigger: the UART RX magic sets PENDING; the timer softirq
/// (which already hosts the periodic DEBUG dumps and may printk) runs it.
pub fn run_pending() {
    if PENDING.swap(false, Ordering::AcqRel) {
        dump("uart-magic");
    }
    series_tick();
}

/// Periodic raw time series (fake-OOM audit): one compact + one census
/// line every SERIES_PERIOD while rux:ledger (retired debug=ledger) is armed — the anon-growth
/// curve that discriminates a leak (monotone growth after service
/// steady-state) from genuine consumption. Raw UART on purpose: the
/// printk path can wedge during exactly the storms this instruments.
/// Called from run_pending (timer softirq cadence; timer.rs stays
/// untouched — territory), throttled internally.
static LAST_SERIES: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
const SERIES_PERIOD: u64 = 30 * 100; // 30 s at 100 Hz

pub fn series_tick() {
    if !crate::trace::EVENT_LEDGER.armed() {
        return;
    }
    let current = crate::timer::get_jiffies();
    if current.saturating_sub(LAST_SERIES.load(Ordering::Relaxed)) < SERIES_PERIOD {
        return;
    }
    LAST_SERIES.store(current, Ordering::Relaxed);
    crate::trace::trace_ledger(0, 0, current, 0);
    compact_line();
    census_line();
}


// ════════════════ driftwatch ══════════════════════════
// Zero-put / free-refusal attribution counters (fake-OOM freelist-drift
// family, rux:ledger (retired debug=ledger)).
//
// The MEMLEDGER census proved the drift mechanism — pages reach
// refcount 0 without ever re-entering the buddy — but a post-hoc census
// cannot say WHICH release site orphaned them. This module counts, per
// call site, every `put_page()` that lands the count on 0 (the moment a
// frame becomes free-able) and every `free_pages()` refusal branch (the
// paths that drop a free on the floor). The ledger prints the vector;
// drift attribution is then arithmetic:
//
//   sum(ZERO_PUT[site]) - freed_pages - refused_pages = leaked at site
//
// The counters are always-on `Relaxed` atomics (one increment per
// zero-put — rare — and one per refused free); no locks, no printk, safe
// from any context including the zone-lock-held free path.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Release sites that can land a refcount on 0. Keep in sync with
/// SITE_NAMES and the `self::note_zero_put(SITE_*)` call sites.
pub const SITES: usize = 16;

pub const S_TEARDOWN_RV: usize = 0; // riscv64 release_user_leaf
pub const S_TEARDOWN_X86_L1: usize = 1; // x86_64 free_user_page_tables PTE leaf
pub const S_TEARDOWN_X86_L2: usize = 2; // x86_64 2MB leaf
pub const S_TEARDOWN_X86_L3: usize = 3; // x86_64 1GB leaf
pub const S_COW_RV: usize = 4; // riscv64 handle_cow_fault old-page put
pub const S_COW_X86: usize = 5; // x86_64 COW old-page put
pub const S_UNMAP_X86: usize = 6; // x86_64 unmap_pages leaf put
pub const S_UNMAP_RV: usize = 7; // riscv64 unmap/teardown aux sites
pub const S_VMSCAN_SWAP: usize = 8; // vmscan swap-out final put
pub const S_SHM_DROP: usize = 9; // sysv_shm ShmPages::drop
pub const S_ASHMEM_DROP: usize = 10; // ashmem AshmemPages::drop
pub const S_BINDER_DROP: usize = 11; // binder BinderAlloc::drop
pub const S_BINDER_ZAP: usize = 12; // binder reserve-window anon zap
pub const S_IOURING_DROP: usize = 13; // io_uring ring region drop
pub const S_PAGECACHE: usize = 14; // page-cache release paths (unused: cache
//                                  frames free via free_page with rc reset
//                                  by the zone, not via put_page)
pub const S_OTHER: usize = 15; // any site not yet tagged

pub const DRIFTWATCH_SITE_NAMES: [&str; SITES] = [
    "teardown-rv",
    "teardown-x86-pte",
    "teardown-x86-2m",
    "teardown-x86-1g",
    "cow-rv",
    "cow-x86",
    "unmap-x86",
    "unmap-rv",
    "vmscan-swap",
    "shm-drop",
    "ashmem-drop",
    "binder-drop",
    "binder-zap",
    "iouring-drop",
    "pagecache",
    "other",
];

/// put_page() transitions to 0, per site.
pub static ZERO_PUT: [AtomicUsize; SITES] = [const { AtomicUsize::new(0) }; SITES];

/// free_pages() outcomes (page_alloc::free_pages):
/// 0 = returned to a zone, 1 = refused still-MAPPED, 2 = WILD addr,
/// 3 = refused boot-reserved, 4 = refused text-stamp foreign,
/// 5 = no zone matched (fell off the end).
pub static FREE_OK: AtomicUsize = AtomicUsize::new(0);
pub static FREE_REFUSED_MAPPED: AtomicUsize = AtomicUsize::new(0);
pub static FREE_DROPPED_WILD: AtomicUsize = AtomicUsize::new(0);
pub static FREE_REFUSED_RESERVED: AtomicUsize = AtomicUsize::new(0);
pub static FREE_REFUSED_TEXT: AtomicUsize = AtomicUsize::new(0);
pub static FREE_NO_ZONE: AtomicUsize = AtomicUsize::new(0);
/// free_pages() calls with addr == 0 (legal no-op).
pub static FREE_NULL: AtomicUsize = AtomicUsize::new(0);
/// put_page() underflow refusals (rc already 0) — the inverse drift
/// signal (a page freed twice through the refcount side).
pub static PUT_UNDERFLOW: AtomicUsize = AtomicUsize::new(0);
/// mm-side descriptor puts that reached 0 on a PCACHE (cache-backed)
/// mapping leaf — the phantom-put signal (pcachewedge lane, 2026-11-15).
/// Each one leaked a frame instead of freeing it (see
/// page_alloc::pcache_leaf_phantom_zero_put); nonzero here names the
/// raw-free double-hand family's producer.
pub static PCACHE_LEAF_PHANTOM_PUT: AtomicUsize = AtomicUsize::new(0);
/// Cache-side releases REFUSED because the frame still carries a live
/// PT-ledger stamp (double-owned cache entry; see
/// page_cache::release_page_frame). Each refusal leaked one frame and
/// prevented a RAW-FREE of a live page table.
pub static PCACHE_LIVE_PT_REFUSED: AtomicUsize = AtomicUsize::new(0);
/// Frames quarantined by the mmforensics lane's double-tracking guards
/// (PtLedger::poison_table): raw free of a live-stamped frame, cache-side
/// live-PT release refusal, phantom zero-put leak, ledger DOUBLE-* —
/// one-way marks; a nonzero value names corruption contained by leak.
pub static MM_POISONED: AtomicUsize = AtomicUsize::new(0);
/// Free attempts REFUSED because the frame is quarantined (the free-side
/// half of the quarantine: poisoned frames never re-enter the allocator).
pub static MM_POISON_FREES: AtomicUsize = AtomicUsize::new(0);

#[inline]
pub fn note_zero_put(site: usize) {
    ZERO_PUT[site.min(SITES - 1)].fetch_add(1, Ordering::Relaxed);
}
/// One line per nonzero counter (moved to the debug crate):
/// same-named wrapper for the ledger's historical call site; dispatches
/// the hook row.
pub fn report() {
    crate::api::debug::driftwatch_report();
}


#[cfg(feature = "x86_64")]
pub mod scribble {
    // ════════════════ scribble ════════════════════════════
    // (was scribble.rs — the module was picked by #[cfg(feature = "x86_64")] on
    // its mod declaration; it stays one inline module of hotpath.rs, its
    // attributes and doc inside, per §2c.3's cfg exception)
    //! MIT License
    //!
    //! Copyright (c) 2026 Fei Wang
    //!
    //! Byte-level scribble hunter (x86 fork/exec storm corruption).
    //!
    //! Tracks a shadow copy of the scheduling-critical fields of every live
    //! Task — `thread.{fs_base, gs_base, sp, callee.ret_addr}` plus the
    //! parked `pt_regs` control words (rip/cs/rflags/rsp/ss) — and flags any
    //! change that does not flow through a legitimate write point:
    //!
    //! - **switch-out quiesce check** (`quiesce` called from
    //!   `x86_switch_publish`): the task is fully saved at that instant, so
    //!   any field that differs from its shadow was scribbled WHILE the
    //!   task was running (or between its last quiesce and now).
    //! - **periodic verify** (`verify` called from the timer tick): catches
    //!   scribbles landing on quiescent sleeping tasks — the observed
    //!   `gs_base <- small int` family, where the garbage value is loaded
    //!   into MSR_KERNEL_GS_BASE at the task's NEXT switch-in.
    //!
    //! All reporting goes through the raw console path (`taskdump_*` /
    //! `put_hex`) so it works with locks held or state half-torn.
    //!
    //! GDB hook: on the first confirmed hit the detector calls
    //! `scribble_park(field_addr)` (a `#[no_mangle]` no-inline function that
    //! just returns). Under `qemu -s` a gdb script breaks on that symbol,
    //! reads `$rdi` (the scribbled field's address) and arms QEMU-watchpoints
    //! (Z2/Z4 — honored by TCG, unlike guest DR registers) on the victim
    //! neighborhood to catch the writer's next store with its RIP.

    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    use crate::process::types::Task;

    pub const SCRIBBLE_SLOTS: usize = 512;

    /// Direct-map heap window that holds heap-allocated Task structs and
    /// kernel stacks (x86_64: phys 0x40000000 + VA_PA_OFFSET, sized by
    /// KERNEL_HEAP_SIZE — 512MB with the OH profile; the old hardcoded
    /// 128MB ceiling silently skipped every task past 0x48000000).
    #[cfg(feature = "x86_64")]
    pub(crate) const HEAP_WINDOW: core::ops::Range<usize> =
        0xffff_8880_4000_0000..(0xffff_8880_4000_0000 + crate::config::KERNEL_HEAP_SIZE);

    const OFF_THREAD: usize = core::mem::offset_of!(Task, thread);
    const OFF_FS: usize = OFF_THREAD + core::mem::offset_of!(crate::arch::thread::ThreadStruct, fs_base);
    const OFF_GS: usize = OFF_THREAD + core::mem::offset_of!(crate::arch::thread::ThreadStruct, gs_base);
    const OFF_SP: usize = OFF_THREAD + core::mem::offset_of!(crate::arch::thread::ThreadStruct, sp);
    const OFF_RET: usize =
        OFF_THREAD + core::mem::offset_of!(crate::arch::thread::CalleeSaved, ret_addr);
    /// Ownership word: {ti_on_cpu: bool, running_on_cpu: i32} share one
    /// aligned qword (R65 hunt). A single 8-byte store of a kernel-static
    /// pointer here yields {false, -1} — the double-run seed observed as
    /// ring "PUB" events without a preceding publish-out (x86-scribble3 g3:
    /// victim word held 0xffffffff_80661100, a .rodata ops pointer).
    const OFF_ONCPU: usize = core::mem::offset_of!(Task, ti_on_cpu);
    const OFF_CLAIM: usize = core::mem::offset_of!(Task, running_on_cpu);

    use crate::debug::types::ScribbleSlot;

    static SLOTS: [ScribbleSlot; SCRIBBLE_SLOTS] = [const {
        ScribbleSlot {
            task: AtomicU64::new(0),
            armed: AtomicU32::new(0),
            gen: AtomicU32::new(0),
            sh_fs: AtomicU64::new(0),
            sh_gs: AtomicU64::new(0),
            sh_sp: AtomicU64::new(0),
            sh_ret: AtomicU64::new(0),
            sh_rip: AtomicU64::new(0),
            sh_rsp: AtomicU64::new(0),
            sh_cs: AtomicU64::new(0),
            armed_swapped: AtomicU32::new(0),
        }
    }; SCRIBBLE_SLOTS];

    pub static HITS: AtomicU32 = AtomicU32::new(0);
    /// Park for the gdb breakpoint after the first hit (rux:scribblepark (retired debug=scribblepark)).
    pub static PARK: AtomicBool = AtomicBool::new(false);
    /// Park when pid 1 registers (rux:scribblepid1 (retired debug=scribblepid1)) so gdb can arm
    /// watchpoints on the long-lived init task's Task struct + kernel
    /// stack at their boot-fixed addresses.
    pub static PARK_PID1: AtomicBool = AtomicBool::new(false);
    /// Park when the 64th task registers (early in the fork/exec storm) so
    /// gdb can arm quiet-region watchpoints (stack bottoms, tp_value,
    /// exception_sp) on the whole live set before the scribbler strikes.
    pub static PARK_SWEEP: AtomicBool = AtomicBool::new(false);
    static REG_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static VERIFY_TICK: AtomicU32 = AtomicU32::new(0);
    static DIVERGE: AtomicU32 = AtomicU32::new(0);
    static GS_FIX_ANNOUNCED: AtomicBool = AtomicBool::new(false);
    static BADIRET_ANNOUNCED: AtomicBool = AtomicBool::new(false);
    // R68: Task-header canary global one-shot (see verify()).
    static HEADER_HIT_ANNOUNCED: AtomicBool = AtomicBool::new(false);

    // ---- pick/publish event ring (rux:scribble (retired debug=scribble)) -------------------------
    // Every mark_picked_on_cpu and x86_switch_publish appends here; the CONS
    // report dumps the tail, showing the last picks/switches around an
    // abandoned pick (on_cpu=1, claim=-1, re-linked, owned by no slot).
    // RingEntry — the definition moved to debug/types.rs; every impl and
    // function that uses it stays here. Re-exported so the paths are unchanged.
    pub use crate::debug::types::RingEntry;

    pub static RING: [RingEntry; 128] = [const {
        RingEntry {
            stamp: AtomicU64::new(0),
            cpu: AtomicU32::new(0),
            kind: AtomicU32::new(0),
            a: AtomicU64::new(0),
            b: AtomicU64::new(0),
        }
    }; 128];

    static RING_CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    /// Append a pick/publish event (lock-free, best-effort ordering).
    pub fn ring_log(cpu: usize, kind: u32, a: u64, b: u64) {
        if !crate::trace::EVENT_SCRIBBLE.armed() {
            return;
        }
        let i = RING_CUR.fetch_add(1, Ordering::Relaxed) % 128;
        let e = &RING[i];
        e.stamp.store(crate::timer::read_time(), Ordering::Relaxed);
        e.cpu.store(cpu as u32, Ordering::Relaxed);
        e.kind.store(kind, Ordering::Relaxed);
        e.a.store(a, Ordering::Relaxed);
        e.b.store(b, Ordering::Relaxed);
    }

    /// Dump the ring tail (raw console).
    pub fn ring_dump() {
        let cur = RING_CUR.load(Ordering::Relaxed);
        let start = cur.saturating_sub(40);
        puts(b"SCRIBBLE-RING cur=");
        put_dec(cur as u64);
        puts(b"\n");
        for i in start..cur {
            let e = &RING[i % 128];
            let kind = e.kind.load(Ordering::Relaxed);
            if kind == 0 {
                continue;
            }
            put_dec((i % 128) as u64);
            puts(b": j=");
            put_dec(e.stamp.load(Ordering::Relaxed));
            puts(b" cpu=");
            put_dec(e.cpu.load(Ordering::Relaxed) as u64);
            match kind {
                1 => puts(b" PICK "),
                2 => puts(b" PUB  "),
                3 => puts(b" ENQ  "),
                _ => puts(b" ??   "),
            }
            put_hex(e.a.load(Ordering::Relaxed));
            puts(b" ");
            put_hex(e.b.load(Ordering::Relaxed));
            puts(b"\n");
        }
    }

    // ---- R69 allocator double-handoff audit (rux:scribble (retired debug=scribble)) -----------------
    // The x86 scribble morphology (live Task headers carrying another
    // allocation's bytes, live stacks carrying foreign small-integer data)
    // is what a buddy page handed to a SECOND owner while the first still
    // lives looks like from the outside. The ownership bitmap (sched.rs,
    // marked by alloc/free_task_slot since R69) lets the allocator itself
    // name that moment. Records are lock-free atomics so the anomaly sites
    // can run under the buddy lock; printing happens from the verify
    // cadence — never from the allocator (the R43/SBI and R34 lock-ordering
    // lessons).
    pub const AUDIT_KIND_HANDOFF: u32 = 1; // alloc returned a live Task page
    pub const AUDIT_KIND_FREE_LIVE: u32 = 2; // free targeted a live Task page
    pub const AUDIT_KIND_DOUBLE_FREE: u32 = 3; // free of an already-free block

    pub use crate::debug::types::AuditRec;

    const AUDIT_SLOTS: usize = 8;
    static ALLOC_AUDIT: [AuditRec; AUDIT_SLOTS] = [const {
        AuditRec {
            kind: AtomicU32::new(0),
            addr: AtomicU64::new(0),
            order: AtomicU32::new(0),
            cpu: AtomicU32::new(0),
            frames: [const { AtomicU64::new(0) }; 6],
        }
    }; AUDIT_SLOTS];
    static AUDIT_DROPPED: AtomicU32 = AtomicU32::new(0);

    /// Walk the frame-pointer chain at the call site (x86_64 rbp layout:
    /// [rbp] = saved rbp, [rbp+8] = return address).
    #[cfg(feature = "x86_64")]
    pub fn audit_frames() -> [u64; 6] {
        let mut out = [0u64; 6];
        let mut fp = super::current_frame_pointer();
        for slot in out.iter_mut() {
            if !(fp >= 0x8000_0000_0000_0000 && fp & 7 == 0) {
                break;
            }
            // SAFETY: diagnostic reads of the live call stack.
            let (ra, next) = unsafe {
                (
                    core::ptr::read_volatile((fp + 8) as *const u64),
                    core::ptr::read_volatile(fp as *const u64),
                )
            };
            if ra == 0 {
                break;
            }
            *slot = ra;
            if !(next > fp && next < 0xffff_ffff_ffff_f000) {
                break;
            }
            fp = next;
        }
        out
    }

    #[cfg(not(feature = "x86_64"))]
    pub fn audit_frames() -> [u64; 6] {
        [0u64; 6]
    }

    /// Record an allocator anomaly (lock-free, first empty slot).
    pub fn audit_record(kind: u32, addr: u64, order: u32, frames: &[u64]) {
        for slot in ALLOC_AUDIT.iter() {
            if slot
                .kind
                .compare_exchange(0, kind, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                slot.addr.store(addr, Ordering::Release);
                slot.order.store(order, Ordering::Release);
                slot.cpu.store(crate::arch::cpu_id() as u32, Ordering::Release);
                for (i, f) in frames.iter().enumerate().take(6) {
                    slot.frames[i].store(*f, Ordering::Release);
                }
                return;
            }
        }
        AUDIT_DROPPED.fetch_add(1, Ordering::Relaxed);
    }

    /// Print and clear pending audit records (verify cadence only).
    pub fn audit_drain() {
        let mut any = false;
        for slot in ALLOC_AUDIT.iter() {
            let kind = slot.kind.load(Ordering::Acquire);
            if kind == 0 {
                continue;
            }
            any = true;
            puts(b"\nALLOC-AUDIT ");
            match kind {
                AUDIT_KIND_HANDOFF => puts(b"HANDOFF"),
                AUDIT_KIND_FREE_LIVE => puts(b"FREE-LIVE"),
                AUDIT_KIND_DOUBLE_FREE => puts(b"DOUBLE-FREE"),
                _ => puts(b"?"),
            }
            puts(b" addr=");
            put_hex(slot.addr.load(Ordering::Relaxed));
            puts(b" order=");
            put_dec(slot.order.load(Ordering::Relaxed) as u64);
            puts(b" cpu=");
            put_dec(slot.cpu.load(Ordering::Relaxed) as u64);
            for i in 0..6 {
                let f = slot.frames[i].load(Ordering::Relaxed);
                if f == 0 {
                    break;
                }
                puts(b" ra");
                put_dec(i as u64 + 1);
                puts(b"=");
                put_hex(f);
            }
            puts(b"\n");
            slot.kind.store(0, Ordering::Release);
        }
        let d = AUDIT_DROPPED.swap(0, Ordering::AcqRel);
        if d != 0 {
            puts(b"ALLOC-AUDIT dropped=");
            put_dec(d as u64);
            puts(b"\n");
            any = true;
        }
        if any {
            ring_dump();
        }
    }
    /// Verify cadence: every N timer ticks per CPU.
    const VERIFY_EVERY: u32 = 4;

    // ---------------------------------------------------------------------------
    // R65 hunt: pin park + scheduler consumption tripwires
    // ---------------------------------------------------------------------------

    static PIN_ADDR: AtomicU64 = AtomicU64::new(0);
    static PIN_PARSED: AtomicBool = AtomicBool::new(false);
    static PIN_DONE: AtomicBool = AtomicBool::new(false);

    fn parse_hex_u64(s: &str) -> Option<u64> {
        let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
        if s.is_empty() || s.len() > 16 {
            return None;
        }
        let mut v: u64 = 0;
        for b in s.bytes() {
            let d = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return None,
            };
            v = (v << 4) | d as u64;
        }
        Some(v)
    }

    fn pin_park_check(task: *mut Task) {
        if PIN_DONE.load(Ordering::Relaxed) {
            return;
        }
        if !PIN_PARSED.swap(true, Ordering::AcqRel) {
            if let Some(v) = crate::cmdline::get_param("scribblepin") {
                if let Some(a) = parse_hex_u64(&v) {
                    PIN_ADDR.store(a, Ordering::Release);
                }
            }
        }
        let pin = PIN_ADDR.load(Ordering::Acquire);
        if pin != 0 && pin == task as u64 {
            PIN_DONE.store(true, Ordering::Release);
            puts(b"SCRIBBLE-PIN task=");
            put_hex(task as u64);
            puts(b" oncpu=");
            put_hex((task as usize + OFF_ONCPU) as u64);
            puts(b" claim=");
            put_hex((task as usize + OFF_CLAIM) as u64);
            puts(b" ksp=");
            put_hex((task as usize + 0x08) as u64);
            puts(b"\n");
            scribble_pin_park(task as u64, (task as usize + OFF_ONCPU) as u64);
        }
    }

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

    /// R65 garbage argument report: the switch path is about to consume (or
    /// was offered) a task pointer that fails the authenticity probe.
    pub fn bad_switch_arg(which: u64, bad: u64, other: u64) {
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
        puts(b"\nBAD-SWITCH-ARG which=");
        put_dec(which);
        puts(b" bad=");
        put_hex(bad);
        puts(b" other=");
        put_hex(other);
        puts(b" cpu=");
        put_dec(crate::arch::cpu_id());
        puts(b"\n");
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
    }

    /// R65 witness: x86_switch_publish observed prev WITHOUT its pick mark.
    /// With the asm-side release point the mark is always set here; a clear
    /// mark means the R49 heal (or a wild write) reset the ownership word
    /// inside the switch-out window. Rate-limited raw print.
    pub fn switch_window_heal(prev: u64) {
        // Idle/boot tasks are never pick-marked (mark_picked_on_cpu is not
        // called for them) — a clear mark at publish entry is their normal
        // state, not a heal witness. Heap tasks only. (Note the direction:
        // kernel-image addresses 0xffffffff8... are numerically ABOVE the
        // direct-map heap prefix 0xffff8880...)
        if !HEAP_WINDOW.contains(&(prev as usize)) {
            return;
        }
        static SEEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = SEEN.fetch_add(1, Ordering::AcqRel) + 1;
        if n > 8 {
            return;
        }
        puts(b"SWITCH-WINDOW-HEAL #");
        put_dec(n as u64);
        puts(b" prev=");
        put_hex(prev);
        puts(b" cpu=");
        put_dec(crate::arch::cpu_id());
        puts(b"\n");
    }

    /// GDB park: pin victim registered — arm watchpoints on $rsi (oncpu addr).
    #[no_mangle]
    #[inline(never)]
    pub extern "C" fn scribble_pin_park(_task: u64, _oncpu_addr: u64) {}

    /// GDB park: double-run seed caught at pick time.
    #[no_mangle]
    #[inline(never)]
    pub extern "C" fn scribble_seed_park(_task: u64) {}

    /// GDB park: garbage switch argument caught before dereference.
    #[no_mangle]
    #[inline(never)]
    pub extern "C" fn scribble_switch_park(_bad: u64, _which: u64) {}

    // ---------------------------------------------------------------------------
    // Raw console helpers (lock-free, usable from any context)
    // ---------------------------------------------------------------------------

    fn putc(c: u8) {
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

    fn puts(s: &[u8]) {
        for &b in s {
            putc(b);
        }
    }

    fn put_hex(v: u64) {
        let mut shift = 64;
        while shift > 0 {
            shift -= 4;
            let nibble = (v >> shift) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            putc(c);
        }
    }

    fn put_dec(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        if v == 0 {
            putc(b'0');
            return;
        }
        while v > 0 {
            i -= 1;
            buf[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        for &b in &buf[i..] {
            putc(b);
        }
    }

    /// Hex-dump `n` qwords at `base`, 4 per line, tagged.
    fn dump_qwords(tag: &[u8], base: usize, n: usize) {
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

    // ---------------------------------------------------------------------------
    // ScribbleSlot table
    // ---------------------------------------------------------------------------

    fn hash_of(task: usize) -> usize {
        // Task allocations are at least page aligned.
        (task >> 12) & (SCRIBBLE_SLOTS - 1)
    }

    fn slot_of(task: *mut Task) -> Option<usize> {
        let h = hash_of(task as usize);
        for i in 0..SCRIBBLE_SLOTS {
            let idx = (h + i) & (SCRIBBLE_SLOTS - 1);
            let cur = SLOTS[idx].task.load(Ordering::Acquire);
            if cur == task as u64 {
                return Some(idx);
            }
            if cur == 0 {
                return None;
            }
        }
        None
    }

    #[inline]
    fn rd(task: *mut Task, off: usize) -> u64 {
        // SAFETY: diagnostic read of a live Task field.
        unsafe { core::ptr::read_volatile((task as usize + off) as *const u64) }
    }

    #[inline]
    fn gen_of(idx: usize) -> u32 {
        SLOTS[idx].gen.load(Ordering::Acquire)
    }

    // ---------------------------------------------------------------------------
    // Lifecycle
    // ---------------------------------------------------------------------------

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

    /// Unregister a Task about to be freed (`free_task_slot`, before drop).
    pub fn unregister(task: *mut Task) {
        if !crate::trace::EVENT_SCRIBBLE.armed() || task.is_null() {
            return;
        }
        if let Some(idx) = slot_of(task) {
            SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
            SLOTS[idx].task.store(0, Ordering::Release);
        }
    }

    /// Absorb a legitimate in-run write to the thread fields (prctl
    /// ARCH_SET_GS/SET_FS, exec reset, fork child init) into the shadow.
    pub fn note_thread_write(task: *mut Task) {
        if !crate::trace::EVENT_SCRIBBLE.armed() || task.is_null() {
            return;
        }
        if let Some(idx) = slot_of(task) {
            SLOTS[idx].sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
            SLOTS[idx].sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
            SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
        }
    }

    // ---------------------------------------------------------------------------
    // Detection
    // ---------------------------------------------------------------------------

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

    /// Periodic GS sanity tripwire: with the per-CPU GS pairing intact, the
    /// ACTIVE GS base points at PER_CPU[cpu] whose first u32 is a small cpu
    /// number. A trap/syscall running with a wrong ACTIVE GS (0 / user value)
    /// reads the low IVT page instead — a huge garbage "cpu id" that then
    /// indexes PER_CPU/CURRENT_PT_REGS wildly (the byte-scribble family:
    /// register-frame data, IVT patterns 0xf000ff53, user pointers landing
    /// in Task pages). Runs once per timer tick per CPU.
    pub fn verify_gs() {
        if !crate::trace::EVENT_SCRIBBLE.armed() {
            return;
        }
        // GS-pairing self-heal counter (trap.S guards): non-zero proves
        // kernel execution with a mis-paired GS base occurred and was
        // neutralized. Announce the first occurrence.
        let fixes = scribble_gs_fixups.load(Ordering::Relaxed);
        if fixes != 0 && !GS_FIX_ANNOUNCED.swap(true, Ordering::AcqRel) {
            puts(b"SCRIBBLE-GS-FIXUPS n=");
            put_dec(fixes as u64);
            puts(b" (kernel ran with a wild GS base; stub guard healed)\n");
        }
        // Bad-iret repair counter: non-zero proves the mis-pairing SOURCE
        // (faulting user-return iretq) actually fires under this load.
        let badiret = scribble_badiret_fixups.load(Ordering::Relaxed);
        if badiret != 0 && !BADIRET_ANNOUNCED.swap(true, Ordering::AcqRel) {
            puts(b"SCRIBBLE-BADIRET-REPAIRS n=");
            put_dec(badiret as u64);
            puts(b" (faulting user-return iretq repaired: SIGSEGV routed)\n");
        }
        // SAFETY: one %gs-relative u32 read; MSR reads are data-safe.
        let (gscpu, gsbase, kgsbase) = unsafe {
            let id: u32;
            core::arch::asm!(
                "mov {0:e}, dword ptr gs:[0]",
                out(reg) id,
                options(nomem, nostack, preserves_flags)
            );
            let b1 = crate::arch::x86_64::cpu::rdmsr(0xC000_0101);
            let b2 = crate::arch::x86_64::cpu::rdmsr(0xC000_0102);
            (id, b1, b2)
        };
        if gscpu as usize >= crate::config::MAX_CPUS {
            let n = HITS.fetch_add(1, Ordering::AcqRel) + 1;
            puts(b"\nSCRIBBLE-GS-BAD #");
            put_dec(n as u64);
            puts(b" cpu=");
            put_dec(crate::arch::cpu_id() as u64);
            puts(b" gscpu_read=");
            put_hex(gscpu as u64);
            puts(b" gs_base=");
            put_hex(gsbase);
            puts(b" kernel_gs_base=");
            put_hex(kgsbase);
            puts(b"\n");
            if PARK.load(Ordering::Relaxed) && n == 1 {
                scribble_park(gsbase);
            }
        }
    }

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

    /// Double-run report: the running_on_cpu claim swap in
    /// x86_switch_publish detected a second switch-into a task whose
    /// continuation is still claimed by another CPU. This runs INSIDE the
    /// switch path — no polling races — and the two CPUs are both live.
    pub fn double_run(pid: u64, newcpu: u64, oldcpu: u64, prevpid: u64, next: u64) {
        // Bounded spin on the report lock (a concurrent panic path may hold
        // it; never wedge the switch).
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

        puts(b"\nDOUBLE-RUN pid=");
        put_dec(pid);
        puts(b" newcpu=");
        put_dec(newcpu);
        puts(b" oldcpu=");
        put_dec(oldcpu);
        puts(b" prevpid=");
        put_dec(prevpid);
        puts(b" next=");
        put_hex(next);
        puts(b"\n");
        ring_dump();

        REPORTING.store(false, Ordering::Relaxed);
        REPORT_LEN.store(REPORT_POS.load(Ordering::Relaxed), Ordering::Release);
        REPORT_LOCK.store(false, Ordering::Release);

        if PARK.load(Ordering::Relaxed) {
            puts(b"SCRIBBLE-DBLPARK task=");
            put_hex(next);
            puts(b" gs=");
            put_hex((next as usize + OFF_GS) as u64);
            puts(b" fs=");
            put_hex((next as usize + OFF_FS) as u64);
            puts(b" sp=");
            put_hex((next as usize + OFF_SP) as u64);
            puts(b" ret=");
            put_hex((next as usize + OFF_RET) as u64);
            puts(b" thread=");
            put_hex((next as usize + OFF_THREAD) as u64);
            puts(b"\n");
            scribble_park((next as usize + OFF_THREAD) as u64);
        }
    }

    // ---------------------------------------------------------------------------
    // Reporting + gdb park
    // ---------------------------------------------------------------------------

    /// Count of GS-pairing self-heals performed by the trap stubs and
    /// syscall entry (trap.S reads this symbol from asm). Non-zero during
    /// a run proves kernel code executed with a mis-paired (user/wild) GS
    /// base — the scribble engine — and that the guard neutralized it.
    #[no_mangle]
    pub static scribble_gs_fixups: AtomicU32 = AtomicU32::new(0);

    /// Count of bad-iret frame repairs (trap.S reads this symbol from asm):
    /// exceptions whose faulting rip is trap_exit's user-path iretq -- the
    /// mis-pairing source that fed the cross-stack scribble engine.
    #[no_mangle]
    pub static scribble_badiret_fixups: AtomicU32 = AtomicU32::new(0);

    /// GDB park hook for the #PF KernelPanic path (x86_64): `break
    /// kernpanic_park` under `qemu -s`; args are (badaddr, rip, rsp). The
    /// debugger then walks the live CR3 to see which root was active and
    /// which PTE was missing.
    #[no_mangle]
    pub extern "C" fn kernpanic_park(badaddr: u64, rip: u64, rsp: u64) {
        let _ = (badaddr, rip, rsp);
    }

    /// GDB park hook: `break scribble_park` under `qemu -s`; `$rdi` holds
    /// the most diagnostic scribbled field address to watch.
    #[no_mangle]
    #[inline(never)]
    pub extern "C" fn scribble_park(_field_addr: u64) {}

    pub use crate::debug::types::ReportBuf;

    unsafe impl Sync for ReportBuf {}
    /// Symbol gdb dumps after a run: `dump binary memory file &REPORT_BUF ..`
    pub static REPORT_BUF: ReportBuf = ReportBuf(core::cell::UnsafeCell::new([0; 4096]));
    pub static REPORT_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    static REPORTING: AtomicBool = AtomicBool::new(false);
    static REPORT_POS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    static REPORT_LOCK: AtomicBool = AtomicBool::new(false);

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

    fn report(task: *mut Task, idx: usize, phase: &[u8]) {
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
}



// ════════════════ monowatch ═══════════════════════════
// CLOCK_MONOTONIC jump tripwire (rux:monowatch (retired debug=monowatch)).
//
// The faultlat side-finding — an in-guest CLOCK_MONOTONIC that ran ~700 s
// ahead of real elapsed during one probe run — made every duration,
// timeout and animation-frame computation untrustworthy. Whether that
// was a real clock jump or unaccounted blocking time could not be
// adjudicated afterwards because nothing sampled the timekeeper. This
// watch makes the timebase self-monitoring on every hunt boot:
//
// Every OBSERVE_JIFFIES (~1 s), the timer softirq samples
// `read_time()` (the single source CLOCK_MONOTONIC, jiffies and all
// hres deadlines derive from) and checks against the last accepted
// sample:
//
// - backward:  new < last — the clock ran time BACKWARD;
// - spike:     delta > MAX_STEP_S — the clock jumped forward far
//              faster than any real interval between two timer ticks
//              (a hypervisor pause/migration resync, a scribbled
//              timebase offset, or a clocksource switch gone wrong).
//
// Reports name the ACTIVE clocksource (HPET / TSC / jiffies-PIT) so a
// jump is attributed on the spot, and carry both sample values in the
// nominal 10 MHz tick domain (×100 = ns). Rate-limited to 8 lines.
//
// Multi-CPU: samples from all harts interleave; the CAS accepts only
// forward-in-bounds advances, so a racing sample from another CPU is
// skipped, not misreported. On the alarm path the loser does not
// update `last` (the next tick re-reports against the same baseline
// until the clock passes it, capped by the rate limit).


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Observation period in jiffies (100 = 1 s at HZ=100).
const OBSERVE_JIFFIES: u64 = 100;
/// Largest legitimate advance between two observations (host pause
/// with the VM stopped freezes the virtual clock, so real deltas never
/// exceed the observation interval by much; 5 s absorbs deep
/// interrupt-latency stalls under TCG while still tripping on the
/// ~700 s class of jump by three orders of magnitude).
const MAX_STEP_TICKS: u64 = 5 * crate::config::TIMER_CLOCK_FREQ_HZ;
/// Report budget per boot.
const MAX_REPORTS: u64 = 8;

/// Last accepted read_time() sample (nominal ticks); 0 = not seeded.
static LAST: AtomicU64 = AtomicU64::new(0);
/// Jiffies of the last observation (dedupe: one sample per period).
static LAST_J: AtomicU64 = AtomicU64::new(0);
/// Reports printed so far.
static MONOWATCH_REPORTS: AtomicU64 = AtomicU64::new(0);

/// Name of the active timebase (x86_64: HPET / TSC / PIT-jiffies;
/// riscv64: SBI time CSR).
fn clocksource_name() -> &'static str {
    #[cfg(feature = "x86_64")]
    {
        if crate::drivers::hotpath::timer::hpet::ready() {
            "hpet"
        } else if crate::drivers::hotpath::timer::tsc_active() {
            "tsc"
        } else {
            "pit-jiffies"
        }
    }
    #[cfg(feature = "riscv64")]
    {
        "sbi-time"
    }
    #[cfg(feature = "aarch64")]
    {
        "cntvct"
    }
}

/// One observation; called from the timer softirq (any CPU).
pub fn monowatch_tick(current_jiffies: u64) {
    let last_j = LAST_J.load(Ordering::Acquire);
    // Dedupe inside the observation window (softirq runs per jiffy per
    // CPU; only the first CPU in each window samples).
    if current_jiffies.saturating_sub(last_j) < OBSERVE_JIFFIES && last_j != 0 {
        return;
    }
    let now = crate::timer::read_time();
    let last = LAST.load(Ordering::Acquire);

    if last == 0 {
        // Seed (the CAS below guards two CPUs racing the seed).
        if LAST
            .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            LAST_J.store(current_jiffies, Ordering::Release);
        }
        return;
    }

    if now < last {
        // Backward: report but do NOT pull `last` down — the clock may
        // recover; reporting against the old baseline keeps the record.
        monowatch_report("backward", last, now, current_jiffies);
        return;
    }
    let delta = now - last;
    if delta > MAX_STEP_TICKS {
        monowatch_report("spike", last, now, current_jiffies);
        // Accept the new value so one jump is one report, not MAX_REPORTS.
        LAST.store(now, Ordering::Release);
        LAST_J.store(current_jiffies, Ordering::Release);
        return;
    }
    // Forward-in-bounds: try to advance the baseline; a racing CPU that
    // already moved it further wins and we simply skip.
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        LAST_J.store(current_jiffies, Ordering::Release);
    }
}

fn monowatch_report(kind: &str, last: u64, now: u64, jiffies: u64) {
    let n = MONOWATCH_REPORTS.fetch_add(1, Ordering::Relaxed);
    if n >= MAX_REPORTS {
        return;
    }
    let delta_ms = (now.saturating_sub(last)) / (crate::config::TIMER_CLOCK_FREQ_HZ / 1000);
    crate::trace::trace_monowatch(0, (n + 1) as u32, last, now);
    crate::pr_err!(
        "MONOWATCH-JUMP: {} clocksource={} last={:#x} now={:#x} delta={}ms jiffies={} ({}/{})",
        kind,
        clocksource_name(),
        last,
        now,
        delta_ms,
        jiffies,
        n + 1,
        MAX_REPORTS
    );
}


// ════════════════ wildwatch ═══════════════════════════
// Wild-write producer attribution tripwire (wildwrite-hunt line).
//
// The residual wild-write engine (b77 hpet::BASE single-byte smash,
// wt1 BASE+BASE_LOCKED wide smash, b81 PER_CPU slots) survives every
// convicted-and-fixed engine (R70 DF, low-half clone, R83/R85 trap.S
// anchors, mremap interior, 32K stacks). Read-side immunity is in
// main; this module is the ATTRIBUTION side: catch the store in the
// act with the writer's rip handed to the existing kernel-#PF printer.
//
// Mechanism: page-level read-only guard. Quiet-after-init statics are
// placed in a page-exclusive cluster (see hpet::GUARD); after boot init
// this module clears the RW bit on the cluster's PTEs in every alias
// that can reach the frames —
//   1. the kernel-half image mapping (the statics' link-time VAs),
//   2. the linear/direct map alias (PAGE_OFFSET + phys),
//   3. the still-live low identity window (PML4[0], phys < 2GB),
// splitting a 2MiB leaf into 4KiB entries where needed.
// Any later store to those frames page-faults with the writer's RIP,
// and `kernel/core/src/arch/x86_64/mm/exception.rs` already prints
// rip + an rbp frame walk before the KERNPANIC dump — attribution
// without adding any per-hit infrastructure.
//
// Arming happens from debug::init: AFTER hpet::init has published the
// statics and BEFORE start_secondaries, so the split needs no TLB
// shootdown IPIs (APs load the shared kernel root afterwards).
//
// A canary page rides in front of the live members: DMA-class writers
// do not go through PTEs at all, so the RO trap cannot see them. The
// canary is pattern-filled at arm time and verified from the timer
// softirq cadence (rux:wildwatch (retired debug=wildwatch) gates everything; zero cost when off).

#[cfg(feature = "x86_64")]
pub use x86::*;
#[cfg(not(feature = "x86_64"))]
pub use stub::*;

#[cfg(not(feature = "x86_64"))]
mod stub {
    pub fn arm() {}
    pub fn verify() {}
    pub fn is_armed(_addr: usize) -> bool {
        false
    }
}

#[cfg(feature = "x86_64")]
mod x86 {
    use core::sync::atomic::{AtomicU64, Ordering};

    use crate::arch::mm::pagetable::PageTableEntry;

    /// Canary fill qword: "WILDWATC" + per-index salt, complement every
    /// other qword — a partial-line smash still leaves a checkable shape.
    const CANARY_MAGIC: u64 = 0x5749_4c44_5741_5453;

    /// Guarded regions (max 4 clusters x 8 pages each is plenty; the hpet
    /// cluster is 2 pages). Recorded for is_armed() and canary verify.
    const MAX_REGIONS: usize = 8;
    static ARMED_START: [AtomicU64; MAX_REGIONS] =
        [const { AtomicU64::new(0) }; MAX_REGIONS];
    static ARMED_END: [AtomicU64; MAX_REGIONS] =
        [const { AtomicU64::new(0) }; MAX_REGIONS];

    /// True when `addr` falls inside a wildwatch-guarded region (any
    /// alias whose VA range was registered — see arm()).
    pub fn is_armed(addr: usize) -> bool {
        let a = addr as u64;
        for i in 0..MAX_REGIONS {
            let s = ARMED_START[i].load(Ordering::Relaxed);
            if s == 0 {
                continue;
            }
            let e = ARMED_END[i].load(Ordering::Relaxed);
            if a >= s && a < e {
                return true;
            }
        }
        false
    }

    /// Arm all guards. Retried from the timer verify cadence until it
    /// succeeds (observed: at debug::init time the image PD for the guard
    /// chunk still reads as zeros — parts of the kernel image mapping
    /// settle after debug::init; the engine only fires minutes in, so a
    /// 1 s-late arming loses nothing). No-op unless rux:wildwatch (retired debug=wildwatch).
    pub fn arm() {
        static ARM_TRIES: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static ARMED_FLAG: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !super::enabled(crate::debug::types::DfxSwitch::WildWatch) {
            return;
        }
        if ARMED_FLAG.load(Ordering::Acquire) {
            return;
        }
        let tries = ARM_TRIES.fetch_add(1, Ordering::AcqRel);
        // Fill the hpet cluster's canary page BEFORE RO-ing it (the fill
        // doubles as the mapped-ness probe: it cannot fault).
        let (start, len) = crate::drivers::hotpath::timer::hpet::wild_guard_range();
        fill_canary(start);
        record(start, len);
        let mut ok_pages = 0usize;
        let mut fail_pages = 0usize;
        let mut guarded_aliases = 0usize;
        // SAFETY: the only linking store per split is one aligned 8-byte
        // store; concurrent readers stay consistent under either mapping.
        unsafe {
            // Alias 1: link-time kernel-half VAs (the statics themselves).
            for va in (start..start + len).step_by(4096) {
                if protect_page(va) {
                    ok_pages += 1;
                } else {
                    fail_pages += 1;
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
                    guarded_aliases += 1;
                }
                if phys < 0x8000_0000 {
                    if phys as usize != va && protect_page(phys as usize) {
                        guarded_aliases += 1;
                    }
                }
            }
        }
        if ok_pages > 0 {
            // CR0.WP: this kernel has never set it (verified live:
            // cr0=0x80000013, WP clear) — with WP=0 a ring-0 store
            // IGNORES the RW bit, so a read-only guard page is writable
            // by the kernel and the trap can never fire. Linux sets WP
            // early for exactly this discipline. Arm it on every CPU
            // that runs this code (BSP here; the verify cadence below
            // re-applies it on the others).
            enable_write_protect();
            ARMED_FLAG.store(true, Ordering::Release);
            crate::pr_info!(
                "debug: wildwatch armed (try {}): {} guard pages RO ({} failed), {} alias PTEs guarded",
                tries,
                ok_pages,
                fail_pages,
                guarded_aliases
            );
            // Self-test variant: smash immediately in THIS context (the
            // arming CPU, right after its invlpg) — isolates table
            // correctness from cross-CPU TLB questions.
            if crate::cmdline::get_param("wildwatch-selftest").is_some() {
                let victim = (start + 4096) as *mut u64; // GUARD.base_locked
                crate::pr_err!(
                    "WILDWATCH-SELFTEST(arm) firing: store to {:#x} on cpu {}",
                    start + 4096,
                    crate::arch::cpu_id()
                );
                // SAFETY: deliberate wild-store emulation onto the RO
                // guard page — this instruction MUST page-fault.
                unsafe { victim.write_volatile(0xdead_beef_dead_beef) };
                crate::pr_err!("WILDWATCH-SELFTEST(arm) FAULT NOT TAKEN");
            }
        } else if tries < 30 {
            // Retry from the next verify cadence (see verify()).
        } else {
            crate::pr_err!(
                "debug: wildwatch FAILED to arm after {} tries — guard pages NOT protected",
                tries
            );
            ARMED_FLAG.store(true, Ordering::Release);
        }
    }

    /// Set CR0.WP on the CURRENT cpu (idempotent). With WP clear, ring-0
    /// stores ignore page RW bits — the guard pages would be writable.
    fn enable_write_protect() {
        // SAFETY: one CR0 read-modify-write; WP is a boot-time-static bit.
        unsafe {
            let cr0 = crate::arch::cpu::read_cr0();
            crate::arch::cpu::write_cr0(cr0 | 0x1_0000);
        }
    }

    fn fill_canary(start: usize) {
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

    /// Verify canary pages (DMA-class writers bypass PTEs; the RO trap
    /// cannot see them). Called from the timer softirq cadence.
    pub fn verify() {
        // Per-CPU idempotent WP arm (APs boot with WP clear; see arm()).
        if ARMED_START[0].load(Ordering::Relaxed) != 0 {
            enable_write_protect();
        }
        // End-to-end self test (cmdline wildwatch-selftest): one cadence
        // after arming, deliberately smash a guarded member. The expected
        // outcome is the full attribution chain: #PF on the RO page ->
        // WILDWRITE-CAUGHT banner -> kernel-fault rip print + rbp walk ->
        // KERNPANIC dump; symbolize the dumped rip with addr2line and it
        // must name THIS function. Boot dies here — that is the design.
        if crate::cmdline::get_param("wildwatch-selftest").is_some() {
            static SELFTEST_FIRED: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            let armed = ARMED_START[0].load(Ordering::Relaxed) != 0;
            if armed && !SELFTEST_FIRED.swap(true, Ordering::AcqRel) {
                let (start, _len) = crate::drivers::hotpath::timer::hpet::wild_guard_range();
                let victim = (start + 4096) as *mut u64; // GUARD.base_locked
                crate::pr_err!("WILDWATCH-SELFTEST firing: deliberate store to {:#x}", start + 4096);
                // SAFETY: deliberate wild-store emulation onto the RO
                // guard page — this instruction MUST page-fault.
                unsafe { victim.write_volatile(0xdead_beef_dead_beef) };
                // If control returns here the trap did NOT fire — the
                // tripwire is broken; scream.
                crate::pr_err!("WILDWATCH-SELFTEST FAULT NOT TAKEN — guard page is NOT read-only!");
            }
        }
        // Retry arming from this cadence until it succeeds (see arm()).
        arm();
        static CANARY_REPORTED: [core::sync::atomic::AtomicBool; MAX_REGIONS] =
            [const { core::sync::atomic::AtomicBool::new(false) }; MAX_REGIONS];
        for r in 0..MAX_REGIONS {
            let s = ARMED_START[r].load(Ordering::Relaxed);
            if s == 0 {
                continue;
            }
            let e = ARMED_END[r].load(Ordering::Relaxed);
            let su = s as usize;
            let canary_end = (su + 4096).min(e as usize);
            let page = su as *const u64;
            for i in 0..((canary_end - su) / 8) {
                let expect = if i & 1 == 0 {
                    CANARY_MAGIC ^ ((i as u64) << 32)
                } else {
                    !(CANARY_MAGIC ^ ((i as u64) << 32))
                };
                // SAFETY: registered armed kernel page.
                let v = unsafe { page.add(i).read_volatile() };
                if v != expect {
                    if !CANARY_REPORTED[r].swap(true, Ordering::AcqRel) {
                        crate::pr_err!(
                            "WILDWATCH-CANARY addr={:#x} expected={:#x} got={:#x} (PTE-invisible writer: DMA-class?)",
                            su + i * 8,
                            expect,
                            v
                        );
                    }
                    return;
                }
            }
        }
    }

    fn record(start: usize, len: usize) {
        for i in 0..MAX_REGIONS {
            let s = ARMED_START[i].load(Ordering::Relaxed);
            if s == start as u64 {
                return; // already recorded (retry)
            }
            if s == 0 {
                if ARMED_START[i]
                    .compare_exchange(0, start as u64, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    ARMED_END[i].store((start + len) as u64, Ordering::Release);
                }
                return;
            }
        }
    }


    /// Translate a page-table phys to a VA this walk can dereference.
    ///
    /// The arch helper's translation is stage-dependent; at debug::init a
    /// linear-map read of an image-phys table returned 0 for an entry the
    /// MMU translates fine (PD[3] read 0x0 under a live 2MiB leaf). The
    /// boot-time tables come from the early pool INSIDE the kernel image,
    /// whose kernel-half VAs are valid by the constant VMA-LMA offset —
    /// use that translation for image-phys, arch helper otherwise.
    fn table_virt(phys: u64) -> *mut crate::arch::mm::pagetable::PageTable {
        use crate::arch::mm::memory_layout::{KERNEL_ENTRY, KERNEL_MAP};
        let end = crate::arch::mm::mmu_init::kernel_image_phys_end();
        // SAFETY: read-only access of the boot-constant VMA-LMA offset
        // (written once during mmu init, before any wildwatch code runs).
        let offset = unsafe { KERNEL_MAP.va_kernel_pa_offset } as u64;
        if phys >= KERNEL_ENTRY && phys < end {
            let va = phys.wrapping_add(offset);
            return va as *mut _;
        }
        // SAFETY: same contract as the arch helper.
        unsafe { crate::arch::mm::mmu_init::get_page_table_virt(phys) }
    }

    /// Resolve the physical address a kernel-half VA maps to (leaf walk).
    /// Full 4-level walk: PML4 -> PDPT -> PD -> PTE.
    fn resolve_phys(va: usize) -> Option<u64> {
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

    /// Clear RW on the 4KiB page covering `va`, splitting a 2MiB leaf if
    /// needed. Returns true when the leaf reads back read-only.
    ///
    /// SAFETY (caller): runs pre-SMP from debug::init on the BSP; the
    /// linking store is one aligned 8-byte store, so the old and new
    /// translations are both consistent for concurrent readers.
    unsafe fn protect_page(va: usize) -> bool {
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
}


// ════════════════ metawatch ═══════════════════════════
// Buddy-meta shadow scan (rux:metawatch (retired debug=metawatch)) — wildwrite-hunt line.
//
// The unified-engine hypothesis (session 4-12 archives) gives the wild
// writer two downstream faces: kernel-STATIC victims (watched by the
// gdb watchpoint chain) and buddy-heap METADATA corruption — the
// ww-v2 order-18 grind face, where an impossible coalescing scale ran
// with only ~229 MB in use. The buddy metadata (free-list links +
// per-page free bitmap) is megabytes and cannot be watchpointed; this
// module is its in-kernel witness.
//
// On a 10 s timer cadence the allocator derives a shadow free set from
// the free lists (see mm::buddy_allocator::buddy_meta_shadow_scan) and
// cross-checks INV-HEAP-1/2/3. Pass 1 is lock-free (transient
// mid-mutation states are expected under churn); any dirty pass 1 is
// re-audited UNDER the heap irqsave lock, and only a PERSISTENT
// mismatch is reported — a META-SMASH print with the finding details.
// A META-SMASH on any boot is the buddy-face capture for the hunt:
// record the boot, correlate with the watchpoint logs.
//
// Discipline: one arming print per boot; ZERO traffic until a hit
// (transient pass-1 states are counted, never printed).


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Scan cadence in jiffies (100 Hz clock → 10 s). One scan sweeps the
/// 4096-word bitmap three times plus the free-list walk — low-single-
/// digit ms under TCG, ~0.05% of wall time. Corruption is persistent,
/// so detection latency costs nothing.
const SCAN_PERIOD: u64 = 1000;
/// Findings buffered per scan (softirq stack discipline — 8 x ~48 B).
const MAX_FINDINGS: usize = 8;
/// Rate limit for repeat META-SMASH summaries after the first full
/// report (jiffies).
const REPEAT_LIMIT: u64 = 600;

static LAST_SCAN: AtomicU64 = AtomicU64::new(0);
static SCANS: AtomicUsize = AtomicUsize::new(0);
static TRANSIENTS: AtomicUsize = AtomicUsize::new(0);
static FIRED: AtomicBool = AtomicBool::new(false);
static LAST_LOUD: AtomicU64 = AtomicU64::new(0);
static METAWATCH_ARMED: AtomicBool = AtomicBool::new(false);
static SELFTEST_DONE: AtomicBool = AtomicBool::new(false);

/// Timer-softirq cadence entry (timer.rs calls this when the switch is
/// on; throttling is internal).
pub fn tick(current: u64) {
    if !SELFTEST_DONE.swap(true, Ordering::Relaxed) {
        // One-shot detector selftest (wildwatch-selftest precedent):
        // inject a single bad free_bits bit under the heap lock, expect
        // the audit to flag it, restore, expect clean. All before any
        // real scan — the boot either proves its tripwire or says so.
        if crate::cmdline::get_param("metawatch-selftest").is_some() {
            if hotpath::buddy_meta_selftest() {
                crate::pr_err!(
                    "METAWATCH-SELFTEST PASS: injected free_bits bit flagged (BitmapClearListed) and restored, re-audit clean"
                );
            } else {
                crate::pr_err!(
                    "METAWATCH-SELFTEST FAIL: injected bit not flagged (or pre/post audit dirty) — detector broken, do not trust clean scans"
                );
            }
        }
        crate::pr_info!(
            "METAWATCH-ARMED period={} jiffies (buddy-meta shadow audit)",
            SCAN_PERIOD
        );
        METAWATCH_ARMED.store(true, Ordering::Relaxed);
    }

    let last = LAST_SCAN.load(Ordering::Relaxed);
    if current.saturating_sub(last) < SCAN_PERIOD {
        return;
    }
    if LAST_SCAN
        .compare_exchange(last, current, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    scan(current);
}

fn scan(current: u64) {
    let mut findings = [EMPTY_FINDING; MAX_FINDINGS];
    match hotpath::buddy_meta_shadow_scan(&mut findings) {
        // Another CPU's scan is in flight; corruption persists, so the
        // next cadence observes it too. Not a miss.
        None => {}
        Some(outcome) => {
            SCANS.fetch_add(1, Ordering::Relaxed);
            if outcome.confirmed_findings > 0 {
                metawatch_report(&mut findings, outcome.confirmed_findings, current);
            } else if outcome.lockfree_findings > 0 {
                // Mid-mutation pass-1 state, clean under the lock —
                // expected under allocator churn. Counted, not printed.
                TRANSIENTS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn kind_str(kind: crate::mm::types::MetaFindKind) -> &'static str {
    use crate::mm::types::MetaFindKind::*;
    match kind {
        LinkOob => "LinkOob",
        OrderMismatch => "OrderMismatch",
        FreeByteClear => "FreeByteClear",
        Unaligned => "Unaligned",
        PrevBroken => "PrevBroken",
        BitmapSetUnlisted => "BitmapSetUnlisted",
        BitmapClearListed => "BitmapClearListed",
        DoubleListed => "DoubleListed",
        HopOverflow => "HopOverflow",
    }
}

fn metawatch_report(findings: &mut [MetaFinding], confirmed: usize, current: u64) {
    let first = !FIRED.swap(true, Ordering::Relaxed);
    if first {
        crate::pr_err!(
            "META-SMASH confirmed={} scans={} transients={} (persistent buddy-meta INV-HEAP-* violation — the buddy face of the wild writer)",
            confirmed,
            SCANS.load(Ordering::Relaxed),
            TRANSIENTS.load(Ordering::Relaxed)
        );
        let shown = confirmed.min(findings.len());
        for f in findings[..shown].iter_mut() {
            crate::pr_err!(
                "META-SMASH kind={} order={} idx={} prev={:#x} next={:#x} word={} bits={:#x} derived={:#x}",
                kind_str(f.kind),
                f.order,
                f.idx,
                f.prev,
                f.next,
                f.word,
                f.bits,
                f.derived
            );
        }
        LAST_LOUD.store(current, Ordering::Relaxed);
        return;
    }
    // Rate-limited heartbeat so a long hunt boot shows the corruption
    // is still live without flooding the serial log.
    let last = LAST_LOUD.load(Ordering::Relaxed);
    if current.saturating_sub(last) >= REPEAT_LIMIT {
        LAST_LOUD.store(current, Ordering::Relaxed);
        crate::pr_err!(
            "META-SMASH still-live confirmed={} total_scans={} transients={}",
            confirmed,
            SCANS.load(Ordering::Relaxed),
            TRANSIENTS.load(Ordering::Relaxed)
        );
    }
}


// ════════════════ syscost ═════════════════════════════
// Whole-syscall cost table + tick-mode accounting + kernel-rip page
// histogram for the systematic performance-audit line
// (`feature/perf-audit`, docs/development/perf-audit.md).
//
// Everything here is armed by the kernel cmdline `syscost=1` and costs
// nothing when off (one cached atomic load per syscall exit; the tick
// hooks are two relaxed adds). No `switches section (hotpath.rs)` bit is consumed —
// that file belongs to the wild-write line; this probe owns its own
// cmdline key so the branches can merge without coordination.
//
// What it measures and why:
//
// 1. `enter`/`exit` around `syscall::dispatch::syscall_handler` —
//    per-native-nr { count, wall cycles, max } plus a small ring of
//    >4 ms outliers with the caller comm. Wall cycles include blocking
//    (futex sleep, pipe waits) — deliberately: the ranking question is
//    "where does the 16-23 min OH boot spend wall time", and CPU-vs-
//    block is separated by the tick-mode counters below. Diverging
//    paths (do_exit from exit_group / fatal signals) skip the exit
//    hook, so those calls are undercounted; noted in the report.
//
// 2. Tick-mode counters per CPU — user / kernel-task / idle — from the
//    same timer-tick site the stime accounting uses. `idle` means the
//    tick landed with no user address space current (idle task or the
//    boot kernel context). This decomposes the boot into "CPU-saturated
//    in kernel", "CPU-saturated in user" and "idle-blocked" — the
//    first split of the systematic diagnosis.
//
// 3. A 256-slot kernel-text page histogram sampled on kernel-mode
//    ticks (same sample site as sc105's STIME_HIST, but global and
//    comm-free so no task starves the table; per-task attribution
//    comes from taskdump's utime/stime columns). Pages are symbolized
//    offline against the built ELF's nm(1) output.
//
// Dump: `tick()` is called from the timer softirq; every
// SYSCOST_DUMP_SECS it prints the tables (sorted, compact, one line
// each) plus the current uptime so host-side serial timestamps can be
// correlated with guest time.

/// Syscall-number slots (native x86_64 numbers go up to ~335; generic
/// table numbers stay below; 512 leaves headroom).
pub const NR_SLOTS: usize = 512;

/// Kernel-text page histogram slots.
pub const RIP_SLOTS: usize = 256;

/// Slow-op ring size (>4 ms wall).
pub const SLOW_RING: usize = 64;

/// Slow-op threshold in nominal ticks (100 ns each): 4 ms. A normal
/// fast syscall is 1-30 us; only genuine outliers cross this.
const SLOW_CYC: u64 = 40_000;

/// Dump cadence in jiffies (100 Hz -> 60 s).
const SYSCOST_DUMP_SECS: u64 = 60;


const EMPTY_SYS: SysStat = SysStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

pub static TABLE: [SysStat; NR_SLOTS] = [EMPTY_SYS; NR_SLOTS];


const EMPTY_RIP: RipSlot = RipSlot {
    page: AtomicU64::new(0),
    count: AtomicU64::new(0),
};

pub static RIP_HIST: [RipSlot; RIP_SLOTS] = [EMPTY_RIP; RIP_SLOTS];


const EMPTY_SLOW: SlowEnt = SlowEnt {
    nr: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
};

pub static SLOW: [SlowEnt; SLOW_RING] = [EMPTY_SLOW; SLOW_RING];
pub static SLOW_IDX: AtomicUsize = AtomicUsize::new(0);

/// Per-CPU tick modes: user / kernel-task / idle.
pub static TICK_USER: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
pub static TICK_KERN: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
pub static TICK_IDLE: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

static LAST_DUMP: AtomicU64 = AtomicU64::new(0);

/// Armed cache: 0 unknown, 1 off, 2 on (cmdline `syscost=1`).
static SYSCOST_ARMED: AtomicU8 = AtomicU8::new(0);


const EMPTY_EOF: EofSpin = EofSpin {
    comm: [const { AtomicU8::new(0) }; 16],
    path: [const { AtomicU8::new(0) }; 96],
    n: AtomicU64::new(0),
};

pub static EOF_SPIN: [EofSpin; 12] = [const { EMPTY_EOF }; 12];

pub static EAGAIN_SPIN: [EofSpin; 12] = [const { EMPTY_EOF }; 12];


const EMPTY_RD: RdDataSlot = RdDataSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    file_id: AtomicU64::new(0),
    fd: AtomicU64::new(0),
    ops: AtomicU64::new(0),
    n_cur: AtomicU64::new(0),
    n_total: AtomicU64::new(0),
    bytes: AtomicU64::new(0),
    path: [const { AtomicU8::new(0) }; 96],
};

pub static RD_DATA: [RdDataSlot; 16] = [const { EMPTY_RD }; 16];


const EMPTY_RDFAIL: RdFailSlot = RdFailSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    ret: AtomicI64::new(0),
    n: AtomicU64::new(0),
    fd_state: AtomicU8::new(0),
    fd: AtomicU64::new(0),
    ops: AtomicU64::new(0),
    path: [const { AtomicU8::new(0) }; 96],
};

pub static RD_FAIL: [RdFailSlot; 16] = [const { EMPTY_RDFAIL }; 16];

// ============================================================================
// pa15/pa16: F1 irq-exit dispatch quantification — IF=0 time decomposition,
// UART per-byte cli cost, per-comm printk volume, recv-family fd census.
//
// The pa13/pa14 finding: ~26% of kernel ticks land on the page holding
// enable_irq/restore_irq + inb/outb + FileBuffer, but that share is a
// SAMPLING ARTIFACT (queued ticks attribute to the re-enable site), so
// it does not say how much IF=0 time each producer actually costs.
// pa15 added direct IF=0 zone accounting; pa16 (fifth session) makes it
// (a) exact instead of an upper bound and (b) call-site attributed.
//
// v3 zone model (pa16), edge-triggered + self-healing:
//   - Every hook reads the REAL IF flag (EFLAGS/riscv twin) and
//     adjudicates the model against it. A modeled-open segment found
//     with IF=1 is a leak (the v2 failure mode: v2 had no IF ground
//     truth, so one stuck model-open state soaked every later zone on
//     that CPU — cpu1 inflation in smoke4/pa15); it closes into the
//     HEAL counters (zone cum stays clean) and the model resets.
//   - disable/enable hooks sit BEFORE the cli/sti asm: at the pre-cli
//     edge the still-readable flag tells whether the previous modeled
//     span was legitimate; at the pre-sti edge the flag confirms the
//     close. A disable seen with IF=0 and model closed is a MISSED
//     0-edge (raw cli / unhooked path) and opens unattributed.
//   - Hardware IRQ delivery clears IF before any hook: the irq_enter
//     hook opens the HARDIRQ segment (heal-closing any phantom first);
//     the irq_exit hook (end of preempt::irq_exit, before
//     irq_exit_maybe_preempt_kernel) closes it and hard-resets the
//     zone stack (an iret frame boundary ends every zone).
//   - __do_softirq entry splits the open segment (SOFTIRQ takes over;
//     the enclosing HARDIRQ resumes at its exit) — ksoftirqd dispatch
//     (IF=1, model closed) touches nothing.
//   - putc splits an open segment into a UART zone only when BOTH the
//     model is open AND the real flag is 0 (an IF=0 putc cannot
//     migrate, so the pair is balanced by construction).
//   - REST open sites are attributed with #[track_caller]: the probe
//     patch marks disable_irq / save_and_disable_irq / irq_save /
//     lock_irq* so Location::caller() propagates to the outermost
//     caller — the actual critical-section owner (file:line). The
//     global REST_SITES table (96 slots + counted overflow) decomposes
//     REST cum into the launcher line's per-segment fix list.
// Fault handlers never call irq_enter (verified: only LOCAL_TIMER/IPI/
// IRQ vectors do) and no NMI source exists on this board, so the only
// attribution hazards are the schedule window after irq_exit (accepted,
// sub-frame) and a hypothetical handler bug that stis mid-section
// (which the heal counters will expose loudly).
// Segments are stamped with the RAW TSC (read_counter): open and close
// land on the same CPU, so cross-CPU TSC offsets never enter a delta.
// Cumulatives are converted at dump time via get_counter_freq().
// ============================================================================

/// IF=0 zones (index into per-CPU CUM rows and the zone stack).
pub const ZONE_REST: u8 = 0;
pub const ZONE_HARDIRQ: u8 = 1;
pub const ZONE_SOFTIRQ: u8 = 2;
pub const ZONE_UART: u8 = 3;


const EMPTY_ZROW: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

// ---------------------------------------------------------------------------
// REST call-site histogram (pa16): the probe patch marks the irqsave entry
// points #[track_caller], so each REST segment knows the file:line of the
// critical section that opened it. Keyed by Location pointer identity;
// 96 slots + an overflow row that COUNTS what dropped (the pa4-8 lesson:
// never drop attribution silently).
// ---------------------------------------------------------------------------

pub const REST_SITES_N: usize = 96;


const EMPTY_SITE: RestSite = RestSite {
    loc: AtomicU64::new(0),
    n: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

/// Global (all-CPU) REST open-site table.
pub static REST_SITES: [RestSite; REST_SITES_N] = [const { EMPTY_SITE }; REST_SITES_N];
/// Overflow/miss row: sites beyond the table and unattributed (missed-edge)
/// opens. `n` counts opens, `cyc`/`max` their closed spans.
pub static REST_OF: RestSite = EMPTY_SITE;

/// Per-CPU marker for which REST_SITES row the open segment charges.
const SLOT_NONE: u8 = 0xff;
const SLOT_OF: u8 = 0xfe;


pub static IFZ: If0Zone = If0Zone {
    if0: [const { AtomicU8::new(0) }; crate::config::MAX_CPUS],
    t0: [const { AtomicU64::new(0) }; crate::config::MAX_CPUS],
    zstk: [const { [const { AtomicU8::new(0) }; ZSTK_DEPTH] }; crate::config::MAX_CPUS],
    zd: [const { AtomicU8::new(0) }; crate::config::MAX_CPUS],
    cum: [const { EMPTY_ZROW }; crate::config::MAX_CPUS],
    nseg: [const { EMPTY_ZROW }; crate::config::MAX_CPUS],
    seg_max: [const { EMPTY_ZROW }; crate::config::MAX_CPUS],
    slot: [const { AtomicU8::new(SLOT_NONE) }; crate::config::MAX_CPUS],
    heal_n: [const { AtomicU64::new(0) }; crate::config::MAX_CPUS],
    heal_cyc: [const { AtomicU64::new(0) }; crate::config::MAX_CPUS],
    miss_n: [const { AtomicU64::new(0) }; crate::config::MAX_CPUS],
};

/// Current zone (stack top, or REST when empty). Caller-CPU only.
#[inline]
pub fn zone_cur(cpu: usize) -> u8 {
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    if d == 0 {
        ZONE_REST
    } else {
        IFZ.zstk[cpu][d - 1].load(Ordering::Relaxed)
    }
}

/// Close the currently open segment into `z`; REST spans also charge the
/// open-site row (the site owns every piece of its span except spans a
/// nested UART/SOFTIRQ split took over).
#[inline]
fn seg_close(cpu: usize, z: u8, now: u64) {
    let dt = now.saturating_sub(IFZ.t0[cpu].load(Ordering::Relaxed));
    IFZ.cum[cpu][z as usize].fetch_add(dt, Ordering::Relaxed);
    IFZ.nseg[cpu][z as usize].fetch_add(1, Ordering::Relaxed);
    let mut m = IFZ.seg_max[cpu][z as usize].load(Ordering::Relaxed);
    while dt > m {
        match IFZ.seg_max[cpu][z as usize].compare_exchange_weak(
            m,
            dt,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(cur) => m = cur,
        }
    }
    if z == ZONE_REST {
        let s = IFZ.slot[cpu].load(Ordering::Relaxed);
        let row: &RestSite = if s == SLOT_OF {
            &REST_OF
        } else if (s as usize) < REST_SITES_N {
            &REST_SITES[s as usize]
        } else {
            return; // SLOT_NONE: opened by a non-REST hook — zone cum has it
        };
        row.cyc.fetch_add(dt, Ordering::Relaxed);
        let mut rm = row.max.load(Ordering::Relaxed);
        while dt > rm {
            match row.max.compare_exchange_weak(rm, dt, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(cur) => rm = cur,
            }
        }
    }
}

/// Claim/refresh the REST_SITES row for a section opening at `loc` now.
#[inline]
fn rest_site_open(cpu: usize, loc: usize) {
    let key = loc as u64;
    for i in 0..REST_SITES_N {
        let cur = REST_SITES[i].loc.load(Ordering::Relaxed);
        if cur == key {
            REST_SITES[i].n.fetch_add(1, Ordering::Relaxed);
            IFZ.slot[cpu].store(i as u8, Ordering::Relaxed);
            return;
        }
        if cur == 0 {
            match REST_SITES[i]
                .loc
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    REST_SITES[i].n.fetch_add(1, Ordering::Relaxed);
                    IFZ.slot[cpu].store(i as u8, Ordering::Relaxed);
                    return;
                }
                Err(_) => continue,
            }
        }
    }
    REST_OF.n.fetch_add(1, Ordering::Relaxed);
    IFZ.slot[cpu].store(SLOT_OF, Ordering::Relaxed);
}

/// Pre-`cli` edge hook (probe patch: cpu.rs `disable_irq`, BEFORE the
/// asm; `loc` = `Location::caller()` propagated through the track_caller
/// chain — disable_irq / save_and_disable_irq / irq_save / lock_irq*).
/// Edge-triggered with IF ground truth:
///   model open + IF=1  -> leak (v2's failure mode): heal-close.
///   model open + IF=0  -> nested disable: the outer owner keeps the
///                         section (its whole span is genuinely IF=0).
///   model closed + IF=1-> the normal open: REST segment at `loc`.
///   model closed + IF=0-> missed 0-edge (raw cli / unhooked path):
///                         open unattributed, count a miss.
///
/// perf-gap-hunt (2026-11-05): the hook pair fires on EVERY cli/sti in
/// the kernel (1331 inlined sites in the release-strong build), and the
/// outlined call + frame was the dominant fixed cost of CpuLocal::with
/// on unarmed boots. The wrapper below is `inline(always)` with a
/// fast gate that skips ONLY the settled-unarmed state (ARMED == 1):
/// ARMED == 0 (undetermined, pre-cmdline — pa15) and ARMED == 2 (armed)
/// both still run the full body via the outlined twin, so the on()
/// resolution semantics and the armed-boot accounting are unchanged.
#[inline(always)]
pub fn irq_note_off(loc: Option<&'static core::panic::Location<'static>>) {
    if SYSCOST_ARMED.load(Ordering::Relaxed) == 1 {
        return;
    }
    irq_note_off_full(loc);
}

/// Outlined body of [`irq_note_off`] (see the wrapper comment for the
/// gate contract).
#[cold]
#[inline(never)]
fn irq_note_off_full(loc: Option<&'static core::panic::Location<'static>>) {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) != 0 {
        if crate::arch::cpu::get_interrupts_state() {
            // Heal: the flag says enabled, so the modeled span is not
            // trustworthy end-to-end — charge it to the heal meters,
            // never to a zone or a site.
            let dt = crate::arch::cpu::read_counter()
                .saturating_sub(IFZ.t0[cpu].load(Ordering::Relaxed));
            IFZ.heal_n[cpu].fetch_add(1, Ordering::Relaxed);
            IFZ.heal_cyc[cpu].fetch_add(dt, Ordering::Relaxed);
            IFZ.if0[cpu].store(0, Ordering::Relaxed);
            IFZ.zd[cpu].store(0, Ordering::Relaxed);
            IFZ.slot[cpu].store(SLOT_NONE, Ordering::Relaxed);
        }
        return; // nested disable (or just healed): no new open
    }
    IFZ.t0[cpu].store(crate::arch::cpu::read_counter(), Ordering::Relaxed);
    IFZ.zstk[cpu][0].store(ZONE_REST, Ordering::Relaxed);
    IFZ.zd[cpu].store(1, Ordering::Relaxed);
    IFZ.if0[cpu].store(1, Ordering::Relaxed);
    match loc {
        Some(l) => rest_site_open(cpu, l as *const _ as usize),
        None => {
            IFZ.miss_n[cpu].fetch_add(1, Ordering::Relaxed);
            IFZ.slot[cpu].store(SLOT_OF, Ordering::Relaxed);
        }
    }
}

/// Pre-`sti` edge hook (probe patch: cpu.rs `enable_irq`, BEFORE the
/// asm; restore_irq(true) funnels here). Model open + IF=0 = normal
/// close; model open + IF=1 = the open was phantom (raw sti happened in
/// between) — heal-close. Model closed = nothing (double-sti idiom).
///
/// Same inline(always) fast gate as [`irq_note_off`] (see its comment):
/// settled-unarmed boots skip with a load+branch; ARMED 0/2 run the
/// outlined body, preserving the pa15 resolution semantics.
#[inline(always)]
pub fn irq_note_on() {
    if SYSCOST_ARMED.load(Ordering::Relaxed) == 1 {
        return;
    }
    irq_note_on_full();
}

/// Outlined body of [`irq_note_on`].
#[cold]
#[inline(never)]
fn irq_note_on_full() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) == 0 {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    let dt = now.saturating_sub(IFZ.t0[cpu].load(Ordering::Relaxed));
    if crate::arch::cpu::get_interrupts_state() {
        IFZ.heal_n[cpu].fetch_add(1, Ordering::Relaxed);
        IFZ.heal_cyc[cpu].fetch_add(dt, Ordering::Relaxed);
    } else {
        seg_close(cpu, zone_cur(cpu), now);
    }
    IFZ.if0[cpu].store(0, Ordering::Relaxed);
    IFZ.zd[cpu].store(0, Ordering::Relaxed);
    IFZ.slot[cpu].store(SLOT_NONE, Ordering::Relaxed);
}

/// Hardware IRQ delivery hook (probe patch: preempt::irq_enter). IF was
/// cleared by delivery before any hook could observe it, so the model
/// must open here (delivery requires IF=1, so a modeled-open state at
/// this point is phantom — heal it, then open the frame segment).
#[inline]
pub fn zone_hardirq_enter() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    if IFZ.if0[cpu].load(Ordering::Relaxed) != 0 {
        let dt = now.saturating_sub(IFZ.t0[cpu].load(Ordering::Relaxed));
        IFZ.heal_n[cpu].fetch_add(1, Ordering::Relaxed);
        IFZ.heal_cyc[cpu].fetch_add(dt, Ordering::Relaxed);
    }
    IFZ.t0[cpu].store(now, Ordering::Relaxed);
    IFZ.zstk[cpu][0].store(ZONE_HARDIRQ, Ordering::Relaxed);
    IFZ.zd[cpu].store(1, Ordering::Relaxed);
    IFZ.if0[cpu].store(1, Ordering::Relaxed);
    IFZ.slot[cpu].store(SLOT_NONE, Ordering::Relaxed);
}

/// End of the IRQ frame (probe patch: end of preempt::irq_exit, after
/// inline softirq dispatch, before irq_exit_maybe_preempt_kernel). The
/// iret boundary ends every zone: close whatever is open into its zone
/// (a non-HARDIRQ top here exposes a handler that leaked a section) and
/// hard-reset the stack.
#[inline]
pub fn zone_hardirq_exit() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) != 0 {
        seg_close(cpu, zone_cur(cpu), crate::arch::cpu::read_counter());
        IFZ.if0[cpu].store(0, Ordering::Relaxed);
    }
    IFZ.zd[cpu].store(0, Ordering::Relaxed);
    IFZ.slot[cpu].store(SLOT_NONE, Ordering::Relaxed);
}

/// __do_softirq entry (probe patch). Inline irq-exit dispatch runs with
/// the HARDIRQ frame's IF=0: split the open segment, SOFTIRQ takes over.
/// ksoftirqd dispatch (IF=1, model closed) touches nothing — handlers'
/// own irqsave sections open REST segments through the disable hook.
#[inline]
pub fn zone_softirq_enter() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) == 0 {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    seg_close(cpu, zone_cur(cpu), now);
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    if d < ZSTK_DEPTH {
        IFZ.zstk[cpu][d].store(ZONE_SOFTIRQ, Ordering::Relaxed);
        IFZ.zd[cpu].store((d + 1) as u8, Ordering::Relaxed);
    }
    IFZ.t0[cpu].store(now, Ordering::Relaxed);
}

/// __do_softirq exit (probe patch). Close the SOFTIRQ piece and resume
/// the enclosing frame (usually HARDIRQ). A non-SOFTIRQ top (a handler
/// left its own section open) keeps running — it closes at its own
/// restore, or at the frame boundary if the handler is buggy.
#[inline]
pub fn zone_softirq_exit() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) == 0 || zone_cur(cpu) != ZONE_SOFTIRQ {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    seg_close(cpu, ZONE_SOFTIRQ, now);
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    IFZ.zd[cpu].store(d.saturating_sub(1) as u8, Ordering::Relaxed);
    IFZ.t0[cpu].store(now, Ordering::Relaxed);
}

/// putc entry (probe patch: console.rs). Split the open segment into a
/// UART piece, but only when BOTH the model is open and the real flag
/// is 0 — an IF=0 putc cannot be migrated or preempted, so the
/// push/pop pair is balanced by construction; an IF=1 putc (task
/// context, preemptible) must not touch the zone stack at all.
#[inline]
pub fn zone_uart_push() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) == 0 || crate::arch::cpu::get_interrupts_state() {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    seg_close(cpu, zone_cur(cpu), now);
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    if d < ZSTK_DEPTH {
        IFZ.zstk[cpu][d].store(ZONE_UART, Ordering::Relaxed);
        IFZ.zd[cpu].store((d + 1) as u8, Ordering::Relaxed);
    }
    IFZ.t0[cpu].store(now, Ordering::Relaxed);
}

/// putc exit (probe patch): close the UART piece, resume the enclosing
/// zone. No-op unless the stack top is UART.
#[inline]
pub fn zone_uart_pop() {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) == 0 || zone_cur(cpu) != ZONE_UART {
        return;
    }
    let now = crate::arch::cpu::read_counter();
    seg_close(cpu, ZONE_UART, now);
    let d = IFZ.zd[cpu].load(Ordering::Relaxed) as usize;
    IFZ.zd[cpu].store(d.saturating_sub(1) as u8, Ordering::Relaxed);
    IFZ.t0[cpu].store(now, Ordering::Relaxed);
}

/// Is the current CPU modeled IF=0 right now (for the putc if0 split)?
#[inline]
/// SP-5 r5 frozen-tick trap: the CURRENTLY-OPEN IF=0 segment on `cpu` —
/// its opener site (file:line of the disable call) and raw-TSC age.
/// `None` when the probe is off or no segment is open on that CPU.
/// Cross-CPU diagnostic read (the trap fires on the other CPU's behalf
/// too); the location reference is a kernel static.
pub fn open_cli_segment(cpu: usize) -> Option<(&'static str, u32, u64)> {
    if !on() || cpu >= crate::config::MAX_CPUS {
        return None;
    }
    if IFZ.if0[cpu].load(Ordering::Relaxed) == 0 {
        return None;
    }
    let age = crate::arch::cpu::read_counter()
        .saturating_sub(IFZ.t0[cpu].load(Ordering::Relaxed));
    let s = IFZ.slot[cpu].load(Ordering::Relaxed);
    if s == SLOT_OF {
        return Some(("(overflow-row)", 0, age));
    }
    if (s as usize) >= REST_SITES_N {
        return Some(("(no-rest-slot)", 0, age));
    }
    let key = REST_SITES[s as usize].loc.load(Ordering::Relaxed);
    if key == 0 {
        return Some(("(site-key-0)", 0, age));
    }
    // Keys are &'static Location references stored at open time by the
    // disable hook on this kernel's own code (INV-FOR-3 edge).
    let l = crate::debug::r#unsafe::location(key as usize);
    let (file, line) = (l.file(), l.line());
    Some((file, line, age))
}

pub fn if0_now() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    cpu < crate::config::MAX_CPUS && IFZ.if0[cpu].load(Ordering::Relaxed) != 0
}

// ---------------------------------------------------------------------------
// pa15 UART per-byte cost: every console.rs putc reports its wall time
// (raw TSC, whole call incl. THRE poll + outb), its LSR poll count (one
// inb = one KVM exit), and whether it ran modeled IF=0. Two independent
// accumulator sets (if0 / if1) + log2 histograms give the launcher line
// the per-byte cli-cost distribution for the console-async decision.
// ---------------------------------------------------------------------------


const EMPTY_UART: UartStat = UartStat {
    n: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
    polls: AtomicU64::new(0),
    polls_gt1: AtomicU64::new(0),
    hist: [const { AtomicU64::new(0) }; UART_HIST],
};

pub static UART_IF0: UartStat = EMPTY_UART;
pub static UART_IF1: UartStat = EMPTY_UART;

/// Hook for console.rs putc (x86_64): `dt` raw TSC for the whole call,
/// `polls` = LSR reads issued, `if0` = modeled interrupts-off.
#[inline]
pub fn uart_putc_note(dt: u64, polls: u64, if0: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let s = if if0 { &UART_IF0 } else { &UART_IF1 };
    s.n.fetch_add(1, Relaxed);
    s.cyc.fetch_add(dt, Relaxed);
    let mut m = s.max.load(Relaxed);
    while dt > m {
        match s.max.compare_exchange_weak(m, dt, Relaxed, Relaxed) {
            Ok(_) => break,
            Err(cur) => m = cur,
        }
    }
    s.polls.fetch_add(polls, Relaxed);
    if polls > 1 {
        s.polls_gt1.fetch_add(1, Relaxed);
    }
    let b = (64 - dt.leading_zeros() as usize).min(UART_HIST - 1);
    s.hist[b].fetch_add(1, Relaxed);
}

// ---------------------------------------------------------------------------
// pa15 per-comm printk volume: who is printing. One 16-slot table keyed
// by comm (records reaching write_to_ring_buffer = everything that
// passed the loglevel filter, kernel prints AND /dev/kmsg writes; IRQ-
// context records go to a "<irq>" pseudo-slot so scheduler-interrupt
// prints do not smear random interrupted tasks). Console-emitted bytes
// are counted separately, split by modeled IF=0 (console emission is
// the only printk part that costs cli time in task context).
// ---------------------------------------------------------------------------


const EMPTY_PR: PrSlot = PrSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    recs: AtomicU64::new(0),
    ring_bytes: AtomicU64::new(0),
    con_bytes: AtomicU64::new(0),
    con_if0_bytes: AtomicU64::new(0),
};

pub static PRNT: [PrSlot; 16] = [const { EMPTY_PR }; 16];

/// printk() calls dropped by the console-loglevel pre-filter (they cost
/// only the filter check — the number sizes the filter's value).
pub static PRNT_FILTERED: AtomicU64 = AtomicU64::new(0);

#[inline]
fn pr_slot() -> Option<&'static PrSlot> {
    use core::sync::atomic::Ordering::Relaxed;
    let irq_ctx = crate::interrupt::preempt::in_interrupt();
    let comm: [u8; 16] = if irq_ctx {
        *b"<irq>\0\0\0\0\0\0\0\0\0\0\0"
    } else {
        match crate::sched::hotpath::current() {
            Some(t) => *t.comm(),
            None => *b"<nointask>\0\0\0\0\0\0",
        }
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    for i in 0..PRNT.len() {
        if PRNT[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        if (0..16).all(|j| PRNT[i].comm[j].load(Relaxed) == comm[j]) {
            return Some(&PRNT[i]);
        }
    }
    for i in 0..PRNT.len() {
        if PRNT[i].comm0.load(Relaxed) != 0 {
            continue;
        }
        for j in 0..16 {
            PRNT[i].comm[j].store(comm[j], Relaxed);
        }
        PRNT[i].comm0.store(comm0, Relaxed);
        return Some(&PRNT[i]);
    }
    None
}

/// Hook for printk() early return when level > console_loglevel.
#[inline]
pub fn note_printk_filtered() {
    if on() {
        PRNT_FILTERED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Hook at write_to_ring_buffer entry (every post-filter record).
#[inline]
pub fn note_printk_ring(len: usize) {
    if !on() {
        return;
    }
    if let Some(s) = pr_slot() {
        s.recs.fetch_add(1, Ordering::Relaxed);
        s.ring_bytes.fetch_add(len as u64, Ordering::Relaxed);
    }
}

/// Hook at emit_to_console tail (console-emitted bytes; `len` counts the
/// bytes actually pushed, CR-for-LF expansion included by the caller).
/// v3: reads the REAL flag (modeled was v2; self-healing keeps them
/// equal, real is the ground truth).
#[inline]
pub fn note_printk_con(len: usize) {
    if !on() {
        return;
    }
    let if0 = !crate::arch::cpu::get_interrupts_state();
    if let Some(s) = pr_slot() {
        s.con_bytes.fetch_add(len as u64, Ordering::Relaxed);
        if if0 {
            s.con_if0_bytes.fetch_add(len as u64, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// pa15 recv-family fd census (hilogd.server dual-drain evidence for the
// launcher line's ipc work — measurement only, no semantics change).
// Keyed by (comm, nr); the slot FOLLOWS the current fd: on fd/file_id
// change it retargets and resets the per-fd shape counters (RD_DATA
// pattern). Shape per current fd: data reads + bytes, EAGAINs, zeros,
// first-seen errno; plus flags/O_NONBLOCK/ops/desc resolved at retarget
// (unix sockets: kind + bound name + peer; symbolize ops host-side).
// ---------------------------------------------------------------------------


const EMPTY_RX: RxSlot = RxSlot {
    comm0: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    nr: AtomicU64::new(0),
    fd: AtomicU64::new(0),
    file_id: AtomicU64::new(0),
    flags: AtomicI64::new(0),
    nonblock: AtomicU8::new(0),
    n_total: AtomicU64::new(0),
    n_cur: AtomicU64::new(0),
    n_data: AtomicU64::new(0),
    data_bytes: AtomicU64::new(0),
    n_eagain: AtomicU64::new(0),
    n_zero: AtomicU64::new(0),
    err: AtomicI64::new(0),
    n_err: AtomicU64::new(0),
    ops: AtomicU64::new(0),
    desc: [const { AtomicU8::new(0) }; 96],
};

pub static RECV: [RxSlot; 16] = [const { EMPTY_RX }; 16];

/// Describe a recv-family fd for the census table (resolved once per
/// retarget, never on the hot path). Unix sockets get kind/bound/peer
/// (peer pid via SO_PEERCRED semantics for streams, the connected dgram
/// destination for dgrams); everything else records the ops-table
/// pointer for host-side nm symbolization.
fn recv_describe(fd: usize, out: &[AtomicU8; 96]) {
    let mut s = alloc::string::String::new();
    if let Some(desc) = crate::api::net::unix_recv_describe(fd) {
        s = desc;
    } else if let Some(file) = crate::fs::hotpath::get_file_fd(fd) {
        let ops = file.get_ops().map(|p| p as *const _ as u64).unwrap_or(0);
        let path = match file.get_dentry() {
            Some(dentry) => crate::fs::hotpath::dentry_user_path(dentry),
            None => alloc::string::String::from("<no-dentry>"),
        };
        s = alloc::format!("ops={:#x} path={}", ops, path);
    } else {
        s = alloc::format!("<closed fd {}>", fd);
    }
    for (k, b) in s.as_bytes().iter().take(96).enumerate() {
        out[k].store(*b, Ordering::Relaxed);
    }
}

/// pa15 recv census entry: called from the syscall wrapper for native
/// nr 47 (recvmsg) / 45 (recvfrom) with the caller's fd, flags and the
/// syscall's return value. Zero cost when syscost is off.
pub fn recv_census(nr: usize, fd: usize, flags: i32, ret: i64) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::hotpath::current() {
        Some(t) => *t.comm(),
        None => return,
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    let mut slot: Option<&RxSlot> = None;
    for i in 0..RECV.len() {
        if RECV[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        if (0..16).all(|j| RECV[i].comm[j].load(Relaxed) == comm[j])
            && RECV[i].nr.load(Relaxed) == nr as u64
        {
            slot = Some(&RECV[i]);
            break;
        }
    }
    if slot.is_none() {
        for i in 0..RECV.len() {
            if RECV[i].comm0.load(Relaxed) != 0 {
                continue;
            }
            for j in 0..16 {
                RECV[i].comm[j].store(comm[j], Relaxed);
            }
            RECV[i].comm0.store(comm0, Relaxed);
            RECV[i].nr.store(nr as u64, Relaxed);
            slot = Some(&RECV[i]);
            break;
        }
    }
    let s = match slot {
        Some(s) => s,
        None => return,
    };
    s.n_total.fetch_add(1, Relaxed);
    // Retarget on fd change (follow the current description, RD_DATA
    // pattern; the shape counters restart so the dump shows ONE fd's
    // loop shape at a time).
    let cur_file = crate::fs::hotpath::get_file_fd(fd);
    let fid = cur_file.as_ref().map(|f| f.file_id).unwrap_or(0);
    if s.file_id.load(Relaxed) != fid || s.fd.load(Relaxed) != fd as u64 {
        s.file_id.store(fid, Relaxed);
        s.fd.store(fd as u64, Relaxed);
        s.flags.store(flags as i64, Relaxed);
        let nb = cur_file
            .as_ref()
            .map(|f| {
                (f.flags.load(Relaxed)
                    & crate::fs::types::FileFlags::O_NONBLOCK)
                    != 0
            })
            .unwrap_or(false);
        s.nonblock.store(if nb { 1 } else { 0 }, Relaxed);
        s.ops.store(
            cur_file
                .as_ref()
                .and_then(|f| f.get_ops().map(|p| p as *const _ as u64))
                .unwrap_or(0),
            Relaxed,
        );
        for j in 0..96 {
            s.desc[j].store(0, Relaxed);
        }
        recv_describe(fd, &s.desc);
        s.n_cur.store(0, Relaxed);
        s.n_data.store(0, Relaxed);
        s.data_bytes.store(0, Relaxed);
        s.n_eagain.store(0, Relaxed);
        s.n_zero.store(0, Relaxed);
        s.n_err.store(0, Relaxed);
        s.err.store(0, Relaxed);
    }
    s.n_cur.fetch_add(1, Relaxed);
    if ret > 0 {
        s.n_data.fetch_add(1, Relaxed);
        s.data_bytes.fetch_add(ret as u64, Relaxed);
    } else if ret == 0 {
        s.n_zero.fetch_add(1, Relaxed);
    } else if ret == -(crate::errno::constants::EAGAIN as i64) {
        s.n_eagain.fetch_add(1, Relaxed);
    } else {
        if s.n_err.load(Relaxed) == 0 {
            s.err.store(ret, Relaxed);
        }
        s.n_err.fetch_add(1, Relaxed);
    }
}

/// pa10 failing-read census: call from the read wrapper when ret < 0
/// and ret != -EAGAIN. Records the errno, whether the fd still
/// resolves, and the path. Zero cost when syscost is off.
pub fn note_read_fail(fd: usize, ret: i64) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::hotpath::current() {
        Some(t) => *t.comm(),
        None => return,
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    // First-seen errno per comm is the story (a spin loops one errno).
    for i in 0..RD_FAIL.len() {
        if RD_FAIL[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        let same = (0..16).all(|j| RD_FAIL[i].comm[j].load(Relaxed) == comm[j]);
        if same {
            RD_FAIL[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
    for i in 0..RD_FAIL.len() {
        if RD_FAIL[i].comm0.load(Relaxed) != 0 {
            continue;
        }
        for j in 0..16 {
            RD_FAIL[i].comm[j].store(comm[j], Relaxed);
        }
        RD_FAIL[i].comm0.store(comm0, Relaxed);
        RD_FAIL[i].ret.store(ret, Relaxed);
        RD_FAIL[i].fd.store(fd as u64, Relaxed);
        RD_FAIL[i].n.store(1, Relaxed);
        match crate::fs::hotpath::get_file_fd(fd) {
            Some(file) => {
                let mode = file.flags.load(Relaxed) & crate::fs::types::FileFlags::O_ACCMODE;
                RD_FAIL[i].fd_state.store(
                    if mode == crate::fs::types::FileFlags::O_WRONLY { 2 } else { 1 },
                    Relaxed,
                );
                RD_FAIL[i].ops.store(
                    file.get_ops().map(|p| p as *const _ as u64).unwrap_or(0),
                    Relaxed,
                );
                let path = match file.get_dentry() {
            Some(dentry) => crate::fs::hotpath::dentry_user_path(dentry),
            None => alloc::string::String::from("<no-dentry>"),
        };
                let mut k = 0;
                for &b in path.as_bytes() {
                    if k >= 96 {
                        break;
                    }
                    RD_FAIL[i].path[k].store(b, Relaxed);
                    k += 1;
                }
            }
            None => {
                RD_FAIL[i].fd_state.store(0, Relaxed);
            }
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// launcher-errno lane: the per-(syscall, errno) census. The why-slow boot
// error-stream diff convicted five divergences from kmsg ERROR rows, but
// the launcher-leg freeze (8-17 s, foundation) produces ZERO kmsg rows —
// the remaining divergence class is silent errno answers. This census
// counts every syscall exit with a negative return (EAGAIN excluded: the
// known idle-park storm would drown the signal), keyed by (nr, errno),
// with first-seen boot-time, first comm and first a0 so a cell can be
// attributed to a window and a caller. Printed at every SYSCOST dump.
// ---------------------------------------------------------------------------


const EMPTY_ERR: ErrSlot = ErrSlot {
    nr: AtomicU64::new(0),
    err: AtomicI64::new(0),
    n: AtomicU64::new(0),
    first_ms: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
    a0: AtomicU64::new(0),
    path: [const { AtomicU8::new(0) }; 64],
};

pub static ERR_CENSUS: [ErrSlot; 96] = [const { EMPTY_ERR }; 96];

/// Boot time in ms (the dump() clock, cheaper than a full time-of-day).
fn boot_ms() -> u64 {
    crate::timer::get_jiffies() as u64 * 1000
        / crate::config::KERNEL_HZ as u64
}

/// Record one failing syscall (ret < 0, ret != -EAGAIN). Called from the
/// syscall exit path; zero cost when syscost is off. For path-taking
/// syscalls the first-seen PATH STRING is snapshotted (one safe 64-byte
/// user read, only at cell creation).
pub fn note_errno(nr: usize, ret: i64, a0: u64, a1: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    // Match phase (cheap): count the existing (nr, err) cell.
    for i in 0..ERR_CENSUS.len() {
        if ERR_CENSUS[i].nr.load(Relaxed) != nr as u64 {
            continue;
        }
        if ERR_CENSUS[i].err.load(Relaxed) != ret {
            continue;
        }
        ERR_CENSUS[i].n.fetch_add(1, Relaxed);
        return;
    }
    // Create phase: claim the first free cell (a racy double-claim only
    // wastes a slot; the count stays right because the loser matches on
    // the next call).
    let comm: [u8; 16] = match crate::sched::hotpath::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => *t.comm(),
        None => [0; 16],
    };
    // Which argument is the path (x86_64-generic table: open/stat family
    // paths in a0; *at family paths in a1). 0 = not a path syscall.
    let path_ptr = match nr {
        2 | 4 | 6 | 21 | 83 | 87 | 88 | 89 | 90 => a0, // open stat lstat access mkdir unlink symlink readlink chmod
        257 | 332 => a1,                                // openat statx
        _ => 0,
    };
    let mut path = [0u8; 64];
    if path_ptr != 0 && path_ptr > 0x1000 {
        if let Some(bytes) =
            crate::arch::uaccess_safe::read_user::<[u8; 64]>(path_ptr as usize)
        {
            path = bytes;
        }
    }
    for i in 0..ERR_CENSUS.len() {
        if ERR_CENSUS[i].n.load(Relaxed) != 0 {
            continue;
        }
        ERR_CENSUS[i].nr.store(nr as u64, Relaxed);
        ERR_CENSUS[i].err.store(ret, Relaxed);
        ERR_CENSUS[i].first_ms.store(boot_ms(), Relaxed);
        for j in 0..16 {
            ERR_CENSUS[i].comm[j].store(comm[j], Relaxed);
        }
        ERR_CENSUS[i].a0.store(a0, Relaxed);
        for j in 0..64 {
            ERR_CENSUS[i].path[j].store(path[j], Relaxed);
        }
        ERR_CENSUS[i].n.store(1, Relaxed);
        return;
    }
}

/// pa9 positive-read census: call on every read() that returned data.
/// See `RdDataSlot` for the design; zero cost when syscost is off.
pub fn note_read_data(file: &crate::fs::types::File, n: usize, fd: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::hotpath::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => *t.comm(),
        None => return,
    };
    let comm0 = u64::from_le_bytes(comm[..8].try_into().unwrap());
    let mut slot: Option<&RdDataSlot> = None;
    for i in 0..RD_DATA.len() {
        if RD_DATA[i].comm0.load(Relaxed) != comm0 {
            continue;
        }
        let same = (0..16).all(|j| RD_DATA[i].comm[j].load(Relaxed) == comm[j]);
        if same {
            slot = Some(&RD_DATA[i]);
            break;
        }
    }
    if slot.is_none() {
        for i in 0..RD_DATA.len() {
            if RD_DATA[i].comm0.load(Relaxed) != 0 {
                continue;
            }
            for j in 0..16 {
                RD_DATA[i].comm[j].store(comm[j], Relaxed);
            }
            RD_DATA[i].comm0.store(comm0, Relaxed);
            slot = Some(&RD_DATA[i]);
            break;
        }
    }
    let s = match slot {
        Some(s) => s,
        // Table full of foreign comms — this comm goes untracked.
        None => return,
    };
    s.n_total.fetch_add(1, Relaxed);
    if s.file_id.load(Relaxed) == file.file_id {
        s.n_cur.fetch_add(1, Relaxed);
        s.bytes.fetch_add(n as u64, Relaxed);
        return;
    }
    // Retarget: follow this description from now on.
    s.file_id.store(file.file_id, Relaxed);
    s.fd.store(fd as u64, Relaxed);
    s.ops.store(
        file.get_ops().map(|p| p as *const _ as u64).unwrap_or(0),
        Relaxed,
    );
    let path = match file.get_dentry() {
            Some(dentry) => crate::fs::hotpath::dentry_user_path(dentry),
            None => alloc::string::String::from("<no-dentry>"),
        };
    for j in 0..96 {
        s.path[j].store(0, Relaxed);
    }
    let mut k = 0;
    for &b in path.as_bytes() {
        if k >= 96 {
            break;
        }
        s.path[k].store(b, Relaxed);
        k += 1;
    }
    s.n_cur.store(1, Relaxed);
    s.bytes.store(n as u64, Relaxed);
}


#[inline]
pub fn on() -> bool {
    match SYSCOST_ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            // pa15: do NOT cache a verdict before the cmdline exists.
            // The first caller used to be the first syscall (long after
            // cmdline::init); the pa15 IF=0 zone hooks fire from the very
            // first boot-time irqsave spinlock — mm/heap init, well before
            // cmdline::init_from — and a cached "off" from that call kept
            // syscost dead for the whole boot (pa15smoke: zero dumps).
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => {
                    if crate::cmdline::get_param("syscost").as_deref() == Some("1") {
                        2
                    } else {
                        1
                    }
                }
            };
            SYSCOST_ARMED.store(v, Ordering::Relaxed);
            v == 2
        }
    }
}


/// pa4 EOF-spin census: (comm, path) of readers whose fd returned EOF,
/// once per comm (a spin candidate is a reader looping read()==0; pa3
/// proved hilogd.rd_kmsg's 92K/s never reaches /dev/kmsg's counters).
pub fn note_read_eof(file: &crate::fs::types::File) {
    use core::sync::atomic::Ordering::Relaxed;
    if !on() {
        return;
    }
    let comm: [u8; 16] = match crate::sched::hotpath::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => *t.comm(),
        None => return,
    };
    // Keyed by COMM ONLY (pa7 lesson: init alone opened 8 different
    // EOF files and starved the 12 slots before the interesting
    // spinners started). First EOF path per comm is recorded.
    for i in 0..EOF_SPIN.len() {
        let same = (0..16).all(|j| EOF_SPIN[i].comm[j].load(Relaxed) == comm[j]);
        if same {
            EOF_SPIN[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
    for i in 0..EOF_SPIN.len() {
        let mut empty = true;
        for j in 0..16 {
            if EOF_SPIN[i].comm[j].load(Relaxed) != 0 {
                empty = false;
                break;
            }
        }
        if empty {
            for j in 0..16 {
                EOF_SPIN[i].comm[j].store(comm[j], Relaxed);
            }
            // Reader-relative path (same shape as /proc/<pid>/fd/N).
            let path = match file.get_dentry() {
            Some(dentry) => crate::fs::hotpath::dentry_user_path(dentry),
            None => alloc::string::String::from("<no-dentry>"),
        };
            let mut k = 0;
            for &b in path.as_bytes() {
                if k >= 96 {
                    break;
                }
                EOF_SPIN[i].path[k].store(b, Relaxed);
                k += 1;
            }
            EOF_SPIN[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
}


/// pa5 EAGAIN-spin census: (comm, path) of nonblocking-fd reads that
/// returned EAGAIN, once per comm — the fast-path twin of
/// note_read_eof (pipe/sockets never reach the generic EOF census).
pub fn note_read_eagain(file: &crate::fs::types::File) {
    use core::sync::atomic::Ordering::Relaxed;
    let comm: [u8; 16] = match crate::sched::hotpath::current() {
        // SAFETY: current task; comm is a fixed-size field.
        Some(t) => *t.comm(),
        None => return,
    };
    for i in 0..EAGAIN_SPIN.len() {
        let mut empty = true;
        for j in 0..16 {
            if EAGAIN_SPIN[i].comm[j].load(Relaxed) != 0 {
                empty = false;
                break;
            }
        }
        if empty {
            for j in 0..16 {
                EAGAIN_SPIN[i].comm[j].store(comm[j], Relaxed);
            }
            let path = match file.get_dentry() {
            Some(dentry) => crate::fs::hotpath::dentry_user_path(dentry),
            None => alloc::string::String::from("<no-dentry>"),
        };
            let mut k = 0;
            for &b in path.as_bytes() {
                if k >= 96 {
                    break;
                }
                EAGAIN_SPIN[i].path[k].store(b, Relaxed);
                k += 1;
            }
            EAGAIN_SPIN[i].n.fetch_add(1, Relaxed);
            return;
        }
    }
}

/// Syscall entry stamp. `None` when the probe is off. Stamps are in the
/// NOMINAL time domain (`read_time()`, 10 ticks/us), NOT raw RDTSC:
/// under KVM each vCPU carries its own TSC offset (the sc100/sc114
/// lesson — offsets differ, rates match), so a raw delta across a CPU
/// migration is garbage (pa1 first attempt: nanosleep "avg 1.5 s");
/// `read_time()` applies the per-CPU correction and the monotonic
/// ratchet, at ~2 atomic loads of overhead.
#[inline]
pub fn enter() -> Option<u64> {
    if on() {
        Some(crate::timer::read_time())
    } else {
        None
    }
}

/// Flood-nr per-comm attribution: which process drives the 10^5/s calls.
/// Only the known flood nrs are tracked (16 comm slots each), so the
/// hot path pays one array scan only for those nrs.
pub const FLOOD_NRS: [usize; 6] = [0 /*read*/, 228 /*clock_gettime*/, 47 /*recvmsg*/, 45 /*recvfrom*/, 20 /*writev*/, 202 /*futex*/];


const EMPTY_CC: CommCount = CommCount {
    comm: [const { AtomicU8::new(0) }; 16],
    n: AtomicU64::new(0),
};

pub static FLOOD: [[CommCount; 16]; 6] = [const { [const { EMPTY_CC }; 16] }; 6];

#[inline]
fn flood_account(nr: usize) {
    let f = match FLOOD_NRS.iter().position(|&x| x == nr) {
        Some(i) => i,
        None => return,
    };
    let comm: &[u8; 16] = match crate::sched::hotpath::current() {
        Some(t) => t.comm(),
        None => return,
    };
    let tab = &FLOOD[f];
    for i in 0..16 {
        let mut same = true;
        let mut empty = true;
        for j in 0..16 {
            let c = tab[i].comm[j].load(Ordering::Relaxed);
            if c != 0 {
                empty = false;
            }
            if c != comm[j] {
                same = false;
            }
        }
        if same {
            tab[i].n.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if empty {
            // Claim the slot (comm first, count follows).
            for j in 0..16 {
                tab[i].comm[j].store(comm[j], Ordering::Relaxed);
            }
            tab[i].n.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
}

/// Syscall exit accounting (wall cycles include blocking by design).
/// `dt` in nominal ticks (100 ns each).
#[inline]
pub fn exit(nr: usize, t0: Option<u64>) {
    let t0 = match (t0, on()) {
        (Some(t), true) => t,
        _ => return,
    };
    if nr >= NR_SLOTS {
        return;
    }
    flood_account(nr);
    let dt = crate::timer::read_time().saturating_sub(t0);
    let e = &TABLE[nr];
    e.count.fetch_add(1, Ordering::Relaxed);
    e.cyc.fetch_add(dt, Ordering::Relaxed);
    // Monotonic-ish max: CAS grow; a lost race keeps the older max.
    let mut m = e.max.load(Ordering::Relaxed);
    while dt > m {
        match e
            .max
            .compare_exchange_weak(m, dt, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(cur) => m = cur,
        }
    }
    if dt > SLOW_CYC {
        let i = SLOW_IDX.fetch_add(1, Ordering::Relaxed) % SLOW_RING;
        SLOW[i].nr.store(nr as u64, Ordering::Relaxed);
        SLOW[i].cyc.store(dt, Ordering::Relaxed);
        if let Some(t) = crate::sched::hotpath::current() {
            let c = *t.comm();
            for j in 0..16 {
                SLOW[i].comm[j].store(c[j], Ordering::Relaxed);
            }
        }
    }
}

/// Tick-mode accounting hook — call once per timer tick that landed in
/// a context with a user address space (`user_mode` per the trap
/// frame), plus the idle variant for ticks without one.
#[inline]
pub fn tick_user() {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_USER[c].fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub fn tick_kern(rip: u64) {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_KERN[c].fetch_add(1, Ordering::Relaxed);
    }
    rip_sample(rip);
}

#[inline]
pub fn tick_idle() {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_IDLE[c].fetch_add(1, Ordering::Relaxed);
    }
}

/// Kernel-text page sample (mirrors sc105 STIME_HIST, comm-free so the
/// whole boot crowd shares one table).
#[inline]
fn rip_sample(rip: u64) {
    let (lo, hi) = crate::sched::hotpath::kernel_text_window();
    if rip < lo || rip >= hi {
        return;
    }
    let page = rip & !0xFFFu64;
    // Fast path: find an existing slot.
    let mut min_i = 0usize;
    let mut min_c = u64::MAX;
    for i in 0..RIP_SLOTS {
        let p = RIP_HIST[i].page.load(Ordering::Relaxed);
        if p == page {
            RIP_HIST[i].count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if p == 0 {
            // Claim empty slot.
            if RIP_HIST[i]
                .page
                .compare_exchange(0, page, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                RIP_HIST[i].count.store(1, Ordering::Relaxed);
                return;
            }
            continue;
        }
        let c = RIP_HIST[i].count.load(Ordering::Relaxed);
        if c < min_c {
            min_c = c;
            min_i = i;
        }
    }
    // Steal the least-sampled slot.
    if RIP_HIST[min_i]
        .count
        .compare_exchange_weak(min_c, 1, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        RIP_HIST[min_i].page.store(page, Ordering::Relaxed);
    }
}

/// Periodic dump from the timer softirq. Cheap when off.
pub fn syscost_tick(now_jiffies: u64) {
    if !on() {
        return;
    }
    if now_jiffies.saturating_sub(LAST_DUMP.load(Ordering::Relaxed))
        < SYSCOST_DUMP_SECS * crate::config::KERNEL_HZ as u64
    {
        return;
    }
    LAST_DUMP.store(now_jiffies, Ordering::Relaxed);
    // The report body lives in the debug crate (crate-flip rung
    // debug); the row fires only on the dump cadence.
    crate::api::debug::syscost_dump();
}


// ════════════════ filerd ══════════════════════════════
// rux:filerd (retired debug=filerd) — cached-file read cost anatomy probe (bw_file_rd lane).
//
// The lmbench `bw_file_rd 1m io_only` row ran 50x slower than the
// reference kernel (669 MB/s vs 33.6 GB/s, KVM, strong profile) while
// `bw_mmap_rd` sat at parity and the PageCacheOps counters proved the
// benchmark's reads were 100% page-cache hits with zero disk I/O.
// That pinned the gap to pure CPU cost somewhere between `sys_read`
// and the cached-page copyout, but not WHERE — this probe decomposes
// the warm read(2) path into phases and reports cycle totals per phase
// through /proc/filerd:
//
// - `outer`  — the ext4 fast-path call as seen from the syscall layer
//              (fd lookup excluded: brackets the ext4_file_read_user_vfs
//              call inside read_file_core)
// - `lock`   — f_pos io_lock acquisition + position read
// - `pin`    — page_cache::get_range_into (claim + per-inode lock +
//              BTreeMap walk + per-page pin)
// - `copy`   — the per-page dst.put copyout loop (copy_to_user)
// - `unpin`  — page_cache::put_range (claim + lock + per-page unpin)
// - `epilog` — set_pos + atime stamp + inotify notify
// - `miss`   — how many read calls took the batch-fill path at all
//
// Cycles are raw RDTSC ticks; `tsc_hz` is self-measured at read time by
// bracketing a ~10 ms busy window between the calibrated 10 MHz
// timekeeper and RDTSC. All hooks are behind a single `rux:filerd (retired debug=filerd)`
// cmdline switch (one relaxed atomic load per read call when enabled,
// zero instrumentation cost when off). OFF by default like every DEBUG
// probe.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Fast-path gate (one relaxed load). Published from the debug cmdline
/// switch by `set_enabled`; read on every instrumented read call.

/// Counter set. fetch_add on distinct cache lines; Relaxed is enough —
/// the probe is diagnostic, not a sequencer.
macro_rules! filerd_counters {
    ($($name:ident => $doc:literal),* $(,)?) => {
        $(
            #[doc = $doc]
            pub static $name: AtomicU64 = AtomicU64::new(0);
        )*
    };
}

filerd_counters! {
    OUTER_N => "read(2) fast-path calls bracketed",
    OUTER_CYCLES => "cycles inside the ext4 read fast path (outer bracket)",
    LOCK_CYCLES => "cycles acquiring the f_pos io_lock + reading pos",
    PIN_CYCLES => "cycles inside page_cache::get_range_into",
    COPY_CYCLES => "cycles in the per-page copyout loop (dst.put)",
    UNPIN_CYCLES => "cycles inside page_cache::put_range",
    EPILOG_CYCLES => "cycles in set_pos + atime + inotify notify",
    MISS_N => "read calls that took the batch-fill (cache miss) path",
}

/// Is the probe armed? One relaxed load on each instrumented call.
#[inline]
pub fn filerd_on() -> bool {
    crate::trace::EVENT_FILERD.armed()
}

/// Raw cycle timestamp (RDTSC on x86_64; arch counter elsewhere).
#[inline]
pub fn now() -> u64 {
    crate::arch::cpu::read_counter()
}

/// Charge `since` cycles into `ctr`.
#[inline]
pub fn charge(ctr: &AtomicU64, since: u64) {
    let d = now().wrapping_sub(since);
    ctr.fetch_add(d, Relaxed);
}

/// One read call took the miss path.
#[inline]
pub fn note_miss() {
    MISS_N.fetch_add(1, Relaxed);
}
/// Render /proc/filerd (moved to the debug crate): same-named
/// wrapper for the procfs registration site; dispatches the hook row.
pub fn generate() -> alloc::vec::Vec<u8> {
    crate::api::debug::filerd_generate()
}


// ════════════════ pstore ══════════════════════════════
// pstore-lite — the last panic's evidence, kept in reserved RAM and
// readable through /sys/fs/pstore after the next boot.
//
// The reference kernel's pstore/ramoops arrangement: a RAM region the
// allocator never touches survives the reboot (a guest reset does not
// clear RAM), the crashing kernel writes its last words into it, and
// the next boot exposes the record as
// `/sys/fs/pstore/<type>-<backend>-<id>` (mode 0444 — the reference's
// pstore_mkfile in fs/pstore/inode.c). Rux implements the console
// flavor of that with one reserved region:
//
// - **Reserve.** At boot, `reserve(avoid, floor)` carves the 128 KiB
//   that ends at `avoid`'s page boundary — the DTB's physical base on
//   this platform. The DTB placement is the anchor that makes the
//   scheme work: QEMU loads the FDT at a fixed, machine-config-derived
//   address (the 2 MiB boundary below RAM end on `virt`), so the carve
//   lands at the SAME physical address every boot of the same machine,
//   and it sits strictly below the DTB — the next boot's firmware
//   rewrites the DTB in place, which must never touch the record. (A
//   naive top-of-RAM carve would land exactly ON the DTB at the
//   standard `-m` sizes: 1 GiB RAM ends at 0xc0000000 and the FDT sits
//   at 0xbfe00000.) `memblock_reserve` then keeps the buddy allocator
//   out (only memblock-free pages are seeded into the zones). The
//   carve is refused when it would reach below `floor` (the kernel's
//   static regions) — a memory map too small to carve from gets no
//   pstore, not a pstore over the kernel. Known cost: a boot with
//   `-initrd` places the initrd just below the FDT, so QEMU reloads it
//   over the record area every boot — the record survives only on the
//   no-initrd standard flows (the rootfs-on-disk images).
//
// - **Capture.** The panic handler arms a small staging sink FIRST, so
//   every byte the panic report prints (header, CSRs, registers, the
//   symbolized stack) is copied as it goes to the UART. At the end of
//   the report `panic_capture()` writes the region: a fixed header
//   (magic, version, length, jiffies), the staged panic report, then
//   the newest printk records (a panic-context, lock-free tail walk —
//   a stopped CPU may hold the ring lock, so the copy must not wait).
//   The magic word is written LAST: a crash inside the capture leaves
//   no half-record for the next boot to trust.
//
// - **Re-expose.** After the linear map is up, `early_scan()` checks
//   the magic. A valid record is remembered and the magic is cleared
//   immediately (the CONTENT stays — only the marker goes; without the
//   clear, a clean next boot would re-expose the same stale record).
//   sysfs then publishes it as `console-ramoops-0` when a record was
//   found — the reference names files `<type>-<backend>-<id>` and the
//   single console zone carries id 0.
//
// The region is one record deep: the next panic overwrites it, the
// same last-panic contract the reference's console backend has.

/// The reserved region's size. 128 KiB: the panic report (≤ the staging
/// sink) plus ~110 KiB of console tail — several full screens of
/// context around the crash.
pub const PSTORE_RAM_SIZE: usize = 128 * 1024;

/// The staging sink's size (the panic report itself — header, CSRs,
/// registers, stack — is captured here as it prints).
const SINK_SIZE: usize = 16 * 1024;

/// The record magic: "RUXPST02".
const MAGIC: u64 = u64::from_le_bytes(*b"RUXPST02");


const VERSION: u32 = 2;

/// Console flavor — the panic report (`console-ramoops-0`).
pub const KIND_CONSOLE: u32 = 0;
/// Dmesg flavor — the oops report (`dmesg-ramoops-0`).
pub const KIND_DMESG: u32 = 1;

/// The region's physical base (0 until `reserve()` ran).
static REGION_BASE: AtomicUsize = AtomicUsize::new(0);

/// A record from the previous boot was found by `early_scan()`.
static HAS_RECORD: AtomicBool = AtomicBool::new(false);

/// The found record's kind (valid only while HAS_RECORD).
static RECORD_KIND: AtomicU32 = AtomicU32::new(KIND_CONSOLE);

/// The staging sink: the panic report's bytes, ring-buffered (the write
/// index wraps; the newest SINK_SIZE bytes survive).
static SINK: [AtomicU8Cell; SINK_SIZE] = [const { AtomicU8Cell::new() }; SINK_SIZE];
static SINK_AT: AtomicUsize = AtomicUsize::new(0);
/// The sink is armed (between panic-handler entry and the region write).
static SINK_ARMED: AtomicBool = AtomicBool::new(false);


impl AtomicU8Cell {
    const fn new() -> Self {
        Self(core::sync::atomic::AtomicU8::new(0))
    }
}

/// Compute and remember the region: the `PSTORE_RAM_SIZE` bytes that end
/// at `avoid`'s page boundary (the DTB's physical base — see the module
/// doc for why the carve anchors there instead of the top of RAM).
/// Returns None when the carve would reach below `floor` (the end of
/// the kernel's static regions) or below the start of RAM — a memory map
/// too small to carve from gets no pstore rather than a pstore over the
/// kernel.
pub fn reserve(avoid: usize, floor: usize) -> Option<(usize, usize)> {
    const PAGE: usize = 4096;
    let avoid_page = avoid & !(PAGE - 1);
    let base = avoid_page.saturating_sub(PSTORE_RAM_SIZE);
    if avoid_page <= PSTORE_RAM_SIZE || base <= floor {
        return None;
    }
    REGION_BASE.store(base, Ordering::Release);
    Some((base, PSTORE_RAM_SIZE))
}

/// Remember the region at an explicit base (the x86_64 anchor: the
/// bytes just above the kernel image's physical end — same address
/// every boot of the same build, unlike anything the firmware places;
/// see the plan doc's Tranche-5 design note). Returns None when `base`
/// is not page-aligned — the caller's anchor arithmetic, not a memory
/// map, decides the address here.
pub fn reserve_at(base: usize) -> Option<(usize, usize)> {
    if base % 4096 != 0 {
        return None;
    }
    REGION_BASE.store(base, Ordering::Release);
    Some((base, PSTORE_RAM_SIZE))
}

/// The region's virtual address through the linear map (None before
/// `reserve()` or when the linear map is not up yet).
fn region_virt() -> Option<*mut u8> {
    let base = REGION_BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let va = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(base as u64));
    Some(va.bits() as *mut u8)
}

/// After the linear map is up: look for the previous boot's record.
///
/// A valid magic means the last boot panicked or oopsed (those paths
/// are the only writers). Remember it, clear the magic — the content
/// stays readable until the next capture overwrites the region.
pub fn early_scan() {
    let Some(virt) = region_virt() else {
        return;
    };
    // SAFETY: the region is reserved RAM, mapped by the linear map; the
    // header reads are plain loads of memory no writer touches (the
    // previous boot is gone).
    let hdr = unsafe { &*(virt as *const RecordHeader) };
    if hdr.magic != MAGIC || hdr.version != VERSION {
        return;
    }
    let length = (hdr.length as usize).min(PSTORE_RAM_SIZE - size_of::<RecordHeader>());
    let kind = if hdr.kind == KIND_DMESG {
        KIND_DMESG
    } else {
        KIND_CONSOLE
    };
    HAS_RECORD.store(true, Ordering::Release);
    RECORD_KIND.store(kind, Ordering::Release);
    // SAFETY: same region; this boot now owns it. Clear only the magic.
    unsafe {
        core::ptr::write_volatile(virt as *mut u64, 0);
    }
    crate::pr_info!(
        "pstore: previous boot's {} record found ({} bytes) — /sys/fs/pstore/{}, jiffies {}",
        if kind == KIND_DMESG { "dmesg" } else { "console" },
        length,
        record_name(),
        hdr.jiffies
    );
}

/// The file name the found record publishes as — the reference's
/// `<type>-<backend>-<id>`: the console flavor is console-ramoops-0,
/// the dmesg flavor (an oops) is dmesg-ramoops-0.
pub fn record_name() -> &'static str {
    if RECORD_KIND.load(Ordering::Acquire) == KIND_DMESG {
        "dmesg-ramoops-0"
    } else {
        "console-ramoops-0"
    }
}

/// Did this boot find a record? (sysfs asks before publishing the file.)
pub fn has_record() -> bool {
    HAS_RECORD.load(Ordering::Acquire)
}

/// The record's bytes for the sysfs file (empty when there is none).
pub fn read_record() -> alloc::vec::Vec<u8> {
    if !has_record() {
        return alloc::vec::Vec::new();
    }
    let Some(virt) = region_virt() else {
        return alloc::vec::Vec::new();
    };
    // SAFETY: reserved, mapped, owned by this boot since early_scan
    // cleared the magic; no writer exists until the next panic (and a
    // next panic would overwrite with a full new header first).
    let hdr = unsafe { &*(virt as *const RecordHeader) };
    let length = (hdr.length as usize).min(PSTORE_RAM_SIZE - size_of::<RecordHeader>());
    let bytes = unsafe {
        core::slice::from_raw_parts(virt.add(size_of::<RecordHeader>()), length)
    };
    bytes.to_vec()
}

// ============================================================================
// The panic sink and the capture
// ============================================================================

/// Arm the staging sink (the panic handler's FIRST step, so the whole
/// report is captured as it prints).
pub fn arm_sink() {
    SINK_ARMED.store(true, Ordering::Release);
}

/// One byte of panic output (ConsoleWriter pushes every byte here).
/// Lock-free by construction: plain atomic stores into a static ring.
pub fn sink(byte: u8) {
    if !SINK_ARMED.load(Ordering::Relaxed) {
        return;
    }
    let at = SINK_AT.fetch_add(1, Ordering::Relaxed) % SINK_SIZE;
    SINK[at].0.store(byte, Ordering::Relaxed);
}

/// Write the panic record into the region. Called at the very end of
/// the panic report (after the stack dump, before the `panic=` policy),
/// so the staged sink holds the whole report.
///
/// Allocation-free by construction: the head is formatted into a stack
/// buffer, the sink is a static array, and the console tail is
/// formatted straight into the region by `printk::ring_tail_into` —
/// the panic being captured may well be the heap's own failure, so the
/// capture path must not trust the heap with anything.
pub fn panic_capture(info: &core::panic::PanicInfo<'_>) {
    use core::fmt::Write as _;
    let mut hw = HeadWriter { buf: [0; 512], len: 0 };
    let _ = core::write!(hw, "\nPanic: {}\n", info.message());
    if let Some(loc) = info.location() {
        let _ = core::write!(hw, "Location: {}:{}\n", loc.file(), loc.line());
    }
    write_record(KIND_CONSOLE, &hw);
}

/// Write the oops record into the region (the dmesg flavor). Called
/// from `debug::oops` after the report printed and the policy decided
/// the oops is recoverable — the record then survives a reboot the
/// task death would otherwise hide. Same allocation-free construction
/// as the panic capture.
pub fn oops_capture(count: u32) {
    use core::fmt::Write as _;
    let mut hw = HeadWriter { buf: [0; 512], len: 0 };
    let _ = core::write!(hw, "\nOops: non-fatal kernel fault [#{}], task killed\n", count);
    write_record(KIND_DMESG, &hw);
}

/// The record writer shared by both captures: head bytes, then the
/// staged sink (the report as printed), then the printk tail.
fn write_record(kind: u32, hw: &HeadWriter) {
    let Some(virt) = region_virt() else {
        return;
    };
    // Disarm the sink first: nothing after this point belongs in the
    // record.
    SINK_ARMED.store(false, Ordering::Release);

    // SAFETY: reserved, mapped RAM owned by this dying boot; nothing
    // else touches it (the other CPUs are already stopped or on their
    // way; a racing capture on another CPU at worst interleaves bytes —
    // the record is a forensic artifact, not a protocol).
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


impl core::fmt::Write for HeadWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}


// ════════════════ oops ════════════════════════════════
// The non-fatal Oops path (Tranche 5's remaining bullet).
//
// The reference kernel's contract: a kernel fault that has no
// exception-table fixup prints an `Oops` report — the faulting
// instruction, the registers, the stack — taints the kernel
// (`TAINT_DIE`, the `D` letter), and then POLICY decides what dies:
//
// - an oops in interrupt context, or with `panic_on_oops` set, is a
//   full panic (Linux `oops_end`: `panic("Fatal exception in
//   interrupt")` / `panic("Fatal exception")`);
// - the idle task cannot die (`panic("Attempted to kill the idle
//   task!")`);
// - anything else faults inside SOME task's system call — the task is
//   killed with SIGSEGV (`make_task_dead(SIGSEGV)`) and the machine
//   keeps running.
//
// Until now every one of those faults took the whole machine down (a
// KERNPANIC halt or `panic!`). The trade the reference makes and this
// module adopts: the faulting task may hold locks when it dies, and
// whatever it held stays held (the usual limp-wedge); in exchange the
// other CPUs, the console, and every instrument keep working, so the
// oops is diagnosable from a live system instead of a corpse. Hunts
// that prefer the old loud halt boot `oops=panic`.
//
// Report shape: the first line and the die banner are the reference
// kernel's, verbatim from Linux 5.15 (the Ubuntu 22.04 guest
// generation) — `arch/riscv/mm/fault.c` `die_kernel_fault` /
// `arch/x86/mm/fault.c` `show_fault_oops` for the head line,
// `arch/{riscv,x86}/kernel/traps.c` `die()` for the banner, and
// `show_regs_print_info` for the CPU/PID/Comm/Tainted context line.
// The register rows follow the arch `__show_regs` layout from the
// same source. A byte-exact transcript needs a live reference oops,
// which this host cannot provoke — every line names its source in a
// comment instead.
//
// Capture: the report stages into the pstore sink exactly like the
// panic report (`arm_sink` before the first byte), and the record is
// written into the reserved region as the reference's `dmesg` flavor
// — `/sys/fs/pstore/dmesg-ramoops-0` after the reboot (the panic
// record stays `console-ramoops-0`; a later panic overwrites either).

/// The oops counter — the `[#N]` in the die banner (Linux's
/// `die_counter`, per boot).
static OOPS_COUNT: AtomicU32 = AtomicU32::new(0);

/// Re-entrancy latch: a fault INSIDE this report (a wild frame pointer
/// in the stack walk, a bad comm pointer) must not recurse — the
/// second entry parks that CPU with one marker line, the same
/// discipline as the trap printers' `KP_DUMP_ENTERED`.
static OOPS_ENTERED: AtomicBool = AtomicBool::new(false);

/// `panic_on_oops` — seeded from the `oops=panic` kernel parameter
/// (Linux panic.c's `oops_setup`) and writable at runtime through
/// `/proc/sys/kernel/panic_on_oops`. Default off, like the reference.
static PANIC_ON_OOPS: AtomicBool = AtomicBool::new(false);

/// Read the `oops=` kernel parameter once at boot (debug init). Only
/// `oops=panic` is defined by the reference; any other value is
/// ignored, as upstream's `oops_setup` does.
pub fn oops_init_from_cmdline() {
    if let Some(value) = crate::cmdline::get_param("oops") {
        if value.trim().eq_ignore_ascii_case("panic") {
            PANIC_ON_OOPS.store(true, Ordering::Release);
        }
    }
}

/// The live `panic_on_oops` value (the sysctl's read side).
pub fn panic_on_oops() -> bool {
    PANIC_ON_OOPS.load(Ordering::Acquire)
}

/// Set `panic_on_oops` (the sysctl's write side).
pub fn set_panic_on_oops(on: bool) {
    PANIC_ON_OOPS.store(on, Ordering::Release);
}

/// Print the Linux-shaped Oops report and apply the policy.
/// Never returns.
///
/// `regs_dump` renders the FAULTING trap frame — the register rows in
/// the arch `__show_regs` layout — not a `save_regs()` artifact (the
/// trap printers' SR-PROBE lesson: the panic register dump shows the
/// printer's own frame, which is exactly what an oops must not do).
#[cold]
pub fn oops(kind: Kind, regs_dump: &dyn Fn(&mut ConsoleWriter)) -> ! {
    // Latch first: recursion inside the report parks here instead of
    // nesting forever.
    if OOPS_ENTERED.swap(true, Ordering::Relaxed) {
        let mut put = |b: u8| unsafe { crate::console::putchar_no_lock(b) };
        for &b in b"trap: oops re-entered during report; halting\n" {
            put(b);
        }
        // R11 (the parked-CPU interrupt strand): park with interrupts
        // DISABLED — an IF=1 park wakes on the next timer tick and runs
        // scheduler_tick on top of this broken report context, which can
        // acquire the runqueue lock and wedge inside it (fr7's stranded
        // GRQ holder). The reference shape is panic_smp_self_stop: a
        // dead CPU never takes interrupts again.
        crate::arch::cpu::disable_irq();
        loop {
            crate::arch::cpu::wfi();
        }
    }

    // The die banner's counter, and the D taint, are added before the
    // report prints (Linux die() adds TAINT_DIE before show_regs), so
    // the report's own "Tainted:" line carries the D.
    let n = OOPS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    self::add_taint(super::types::TaintFlags::DIE);

    // Stage every report byte for the pstore record, like the panic
    // handler does FIRST (one relaxed load per byte on every other
    // boot).
    self::arm_sink();

    {
        let mut w = ConsoleWriter::new();

        // ---- head line: the reference's, per arch and kind ----
        match kind {
            Kind::PageFault { addr } => {
                let what = if addr < 4096 {
                    "NULL pointer dereference"
                } else {
                    "paging request"
                };
                #[cfg(feature = "riscv64")]
                let _ = write!(
                    w,
                    "Unable to handle kernel {} at virtual address {:x}\n",
                    what, addr
                );
                #[cfg(feature = "x86_64")]
                let _ = write!(w, "BUG: unable to handle kernel {} at {:x}\n", what, addr);
            }
            Kind::IllegalInstruction => {
                // riscv do_trap_error's banner string ("Oops - illegal
                // instruction"); x86 reports illegal instructions through
                // the same die("Oops") path as faults, and prints the kind
                // from the trap nr line below.
                #[cfg(feature = "riscv64")]
                let _ = w.write_str("Oops - illegal instruction\n");
                #[cfg(feature = "x86_64")]
                let _ = w.write_str("BUG: kernel illegal instruction\n");
            }
            Kind::UnknownException { cause } => {
                #[cfg(feature = "riscv64")]
                let _ = write!(w, "Oops - unhandled exception, scause={:#x}\n", cause);
                #[cfg(feature = "x86_64")]
                let _ = write!(w, "BUG: unhandled kernel exception, vector={:#x}\n", cause);
            }
        }

        // ---- die banner: die()'s "%s [#%d]" (riscv) / __die's
        // "%s: %04lx [#%d] PREEMPT SMP" (x86). The x86 error-code word
        // has no Rux counterpart on the non-#PF kinds; 0000 prints.
        #[cfg(feature = "riscv64")]
        let _ = write!(w, "Oops [#{}]\n", n);
        #[cfg(feature = "x86_64")]
        let _ = write!(w, "Oops: 0000 [#{}] PREEMPT SMP\n", n);

        // ---- context line: show_regs_print_info ----
        let cpu = crate::arch::cpu_id();
        let (pid, comm): (u64, &[u8]) = match crate::sched::hotpath::current() {
            Some(t) => {
                // SAFETY: current() returned a live task pointer; pid()
                // and comm() are plain reads.
                (t.pid() as u64, t.comm() as &[u8])
            }
            None => (0, b"swapper".as_slice()),
        };
        let mut taint_buf = [0u8; 16];
        self::taint_string(&mut taint_buf);
        let tainted = core::str::from_utf8(&taint_buf).unwrap_or("?");
        let _ = write!(w, "CPU: {} PID: {} Comm: {} Tainted: {}\n", cpu, pid, oops_comm_str(comm), tainted);

        // ---- the faulting frame (arch __show_regs rows) ----
        regs_dump(&mut w);
    }

    // ---- Call Trace (the shared symbolized walker; prints its own
    // header) ----
    self::dump_stack();

    // ---- footer (the reference's dump_stack end marker) ----
    {
        let mut w = ConsoleWriter::new();
        let _ = w.write_str("---[ end trace 0000000000 ]---\n");
    }

    // ---- policy: Linux oops_end / make_task_dead ----
    // gw2/fm5 (oh-launch round 6): EVERY arm of this policy either parks
    // the CPU (the panic!s) or runs the dying task's exit (do_exit) —
    // and the fault that brought us here may have struck INSIDE a GRQ
    // section whose lock word would then strand (the permanent wedge;
    // fm5: the first dumper parked via panic! holding the runqueue and
    // 487 DEADLOCK rows piled). Release it NOW, before any arm: the
    // interrupted section is abandoned by every arm below, later GRQ
    // use on the dying path acquires fresh, and the terminal switch's
    // own release becomes a no-op (the tracker is already clear).
    // grq-lock-1st: the GRQ crime scene BEFORE the release — the site
    // stamp and the holder's own stack name the operation the fault
    // struck. IPI-free by construction (the holder has IRQs off).
    crate::sched::hotpath::grq_dump_holder("oops");
    crate::sched::hotpath::terminal_release_grq_if_held();
    if crate::interrupt::preempt::in_interrupt() {
        panic!("Fatal exception in interrupt");
    }
    if panic_on_oops() {
        panic!("Fatal exception");
    }
    match crate::sched::hotpath::current() {
        // No current task (early boot) or the idle task: there is no
        // task to kill — the machine dies (make_task_dead's guards).
        None => panic!("Attempted to kill the idle task!"),
        Some(t) => {
            // SAFETY: current() returned a live task pointer.
            let pid = t.pid();
            if pid == 0 {
                panic!("Attempted to kill the idle task!");
            }
            // Recoverable: the faulting task dies with SIGSEGV and the
            // machine keeps running. The oops record goes to the pstore
            // region here (a policy-escalated oops does not: the panic
            // handler writes its own console record right after, and
            // the staged sink bytes — the oops report — ride inside it).
            self::oops_capture(n);
            crate::process::hotpath::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
        }
    }
}

/// Render a task comm as printable text (the reference prints comm with
/// "%.20s" — printable bytes, truncated; a comm with embedded control
/// bytes prints masked rather than corrupting the line).
fn oops_comm_str(comm: &[u8]) -> &str {
    // comm fields are NUL-padded; stop at the first NUL like a C string.
    let end = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
    let bytes = &comm[..end.min(20)];
    match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => "?(invalid utf8)",
    }
}


// ════════════════ schb ════════════════════════════════
// Soft-wedge heartbeat (`rux:schb (retired debug=schb)`, sc-series quiet-quiet hunt, session R).
//
// The quiet-quiet boot (quietcon + loglevel=3) has a second failure shape
// the classic-race probes cannot see: the pid flow stalls in the
// 900-1400 zone from ~min 10-12 with NO rwlock freeze word and NO kernel
// evidence (sc196/sc197; sc195 passes it 1-in-3). The evidence gap is
// structural: `printk_bytes` drops every record above CONSOLE_LOGLEVEL
// BEFORE the ring, so at loglevel=3 all pr_warn! census output
// (SACMP, binder-stall, stkfutex, RWFRZ) never existed — muted is the
// wrong model; it was never recorded.
//
// This module buys visibility back with a budget far below the
// loglevel-4 watermark (the sc198 lesson: probe traffic modulates the
// console race):
//
// - ONE ~90-byte heartbeat line every `BEAT_SECS` (10s), emitted from
//   this kthread's own context (never a racer thread) straight to the
//   UART via `ConsoleWriter` — no printk ring write, no persistent-log
//   disk I/O, nothing on any binder/futex critical path.
// - `SC-PROC` lines name each new binder process as the heartbeat
//   drains a ring that `binder_open` fills — the port-IO again lands
//   in schbd, not the opener.
// - When the pid flow plateaus for `PLATEAU_BEATS` beats (60s) past
//   early boot, ONE census dump fires per episode: every binder
//   read-wait thread regardless of age (stall_report only prints
//   waiters older than 120s), the scheduler view of every runnable /
//   D-state task, the kernel stall machinery with the console loglevel
//   briefly lifted to 4 (binder-stall + freeze_scan + WAKE-RUN-LEAK
//   then print), and the tail of the printk ring — quietcon stashes
//   the userspace /dev/console stream there, so the records around
//   the wedge are the userspace last-known-good boundary. The dump
//   fires after the wedge has settled, so its burn cannot decide the
//   race it reports on.

/// Heartbeat period (seconds). One line per beat is ~2 orders of
/// magnitude below the loglevel-4 probe stream that carried sc198's
/// freeze in at ~min 9.
const BEAT_SECS: u64 = 10;

/// Consecutive zero-flow beats that would arm a census on the freeze
/// shape alone — kept as documentation of the original sc197 shape;
/// the live trigger is the generalized STALL_BEATS below.
const PLATEAU_BEATS: u32 = 6;
#[allow(dead_code)]
const _PLATEAU_DOC: u32 = PLATEAU_BEATS;

/// sc200 (11th leg): the wedge also comes as a CRAWL, not a freeze —
/// the pid frontier advances 1-9 pids per beat forever (retry-loop
/// churn: DB_TaskExecutor threads), so flow==0 never happens. A beat
/// counts as stalled when ANY of: flow == 0, flow <= CRAWL_FLOW_MAX,
/// or bw15 >= WAITER15_FLOOR (the binder world that parked). Census
/// after STALL_BEATS consecutive stalled beats (120s).
const CRAWL_FLOW_MAX: u32 = 5;
const WAITER15_FLOOR: usize = 150;
const STALL_BEATS: u32 = 12;

/// Minimum boot age (seconds) before a census may fire — early-boot
/// lulls before the service storm are not the wedge.
const MIN_BOOT_SECS: u64 = 240;

/// Minimum live tasks before a census may fire (past early init).
const MIN_TASKS: usize = 50;

/// Leg-17: deterministic census timing (`censusat=<secs>` boot param).
/// The 80%-heap trigger inside the beat cannot fire on a boot parked at
/// the render_service lottery (heap stalls ~300MB of the ~1.1G window),
/// and tonight 3-of-3 rolls parked — the type-attribution mission was
/// hostage to a userspace race. This param fires the SAME one-shot
/// `oom_anat_dump()` at a fixed boot age instead, so even a parked boot
/// banks the census + peek histograms. u64::MAX = disabled (default);
/// the DONE guard inside `oom_anat_dump` dedupes against the 80% trigger
/// (whichever fires first wins).
static CENSUS_AT_SECS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Ring records dumped at the census (quietcon's userspace console
/// stream lives in the printk ring; its tail around the wedge is the
/// userspace last-known-good).
const RING_TAIL: usize = 192;

/// Depth of the binder-proc open ring (one slot per binder_open; an OH
/// boot opens on the order of a hundred).
pub const PROC_RING_LEN: usize = 128;

static SCHB_RUNNING: AtomicBool = AtomicBool::new(false);

/// binder_open trace ring: (seq, pid, jiffies). Filled by `note_proc_open`
/// from the opener's context (spinlock push only — no I/O there); drained
/// and printed by schbd so every byte of UART cost stays off racer paths.
pub static PROC_RING: crate::sync::types::Spinlock<[(u64, u32, u64); PROC_RING_LEN]> =
    crate::sync::types::Spinlock::new([(0, 0, 0); PROC_RING_LEN]);
pub static PROC_SEQ: AtomicU64 = AtomicU64::new(0);

/// Record a binder-proc creation (called from `binder_open`). No-op
/// unless `rux:schb (retired debug=schb)`. Bounded cost: one switch load + one spinlock store.
pub fn note_proc_open(pid: u32) {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    let seq = PROC_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let j = crate::timer::get_jiffies();
    let mut ring = PROC_RING.lock();
    ring[(seq as usize - 1) % PROC_RING_LEN] = (seq, pid, j);
}

// ===== Leg-15 pace counters (launcher round decomposition) =====
//
// sc210 measured ~11 s per launcher getOsAccountLocalId round = ~3000 COW
// faults + 40 forks, with the zone pinned at free~100 pages (wmark low
// 2451, in_direct_reclaim=1) — every COW fault's page alloc rides the
// serialized direct-reclaim slowpath. These cumulative counters decompose
// the round cost from the heartbeat alone: avg ms/fault = 1000/KERNEL_HZ *
// d(cowJ)/d(cow), reclaim share = d(drctJ)/beat-jiffies, regime =
// zfree/heapK trajectory. All sites are fault/alloc hot paths: ONE
// relaxed fetch_add per event, no locks held at any call site, no I/O —
// the sc198 lesson (probe traffic modulates the race) is about prints;
// the only UART cost stays here in schbd's beat.

static PACE_COW: AtomicU64 = AtomicU64::new(0);
static PACE_COW_J: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT_J: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT_MAXJ: AtomicU64 = AtomicU64::new(0);
static PACE_SLOW: AtomicU64 = AtomicU64::new(0);

/// Record one COW fault resolution attempt. `j0` is the jiffies value
/// captured at handle_cow_fault entry (callers pass it so the exit-side
/// cost stays one read + adds). No-op unless `rux:schb (retired debug=schb)`.
pub fn note_cow(j0: u64) {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    PACE_COW.fetch_add(1, Ordering::Relaxed);
    let d = crate::timer::get_jiffies().saturating_sub(j0);
    PACE_COW_J.fetch_add(d, Ordering::Relaxed);
}

/// Record one direct-reclaim round (try_to_free_pages), same shape.
pub fn note_drct(j0: u64) {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    PACE_DRCT.fetch_add(1, Ordering::Relaxed);
    let d = crate::timer::get_jiffies().saturating_sub(j0);
    PACE_DRCT_J.fetch_add(d, Ordering::Relaxed);
    PACE_DRCT_MAXJ.fetch_max(d, Ordering::Relaxed);
}

/// Record one page-alloc slowpath entry (zone fastpath failed).
pub fn note_slow() {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    PACE_SLOW.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the pace counters for the beat line (schbd context only).
pub fn pace_snapshot() -> (u64, u64, u64, u64, u64, u64) {
    (
        PACE_COW.load(Ordering::Relaxed),
        PACE_COW_J.load(Ordering::Relaxed),
        PACE_DRCT.load(Ordering::Relaxed),
        PACE_DRCT_J.load(Ordering::Relaxed),
        PACE_DRCT_MAXJ.load(Ordering::Relaxed),
        PACE_SLOW.load(Ordering::Relaxed),
    )
}

/// One-shot allocator anatomy at the first fake-OOM floor (leg-16).
/// Called from the oom killer's "no killable processes" path: raw UART,
/// allocation-free, counters only — safe in any context, once per boot
/// regardless of debug switches (NET_PAGES is always live; the kstack
/// high-water needs rux:schb (retired debug=schb) to have armed its probe).
pub fn oom_anat_dump() {
    use crate::debug::hotpath::{taskdump_dec, taskdump_raw_line};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let (kn, khw) = crate::process::hotpath::kstack_anat();
    taskdump_raw_line(b"OOM-ANAT kstack live=");
    taskdump_dec(kn);
    taskdump_raw_line(b" hw=");
    taskdump_dec(khw);
    taskdump_raw_line(b" taskSZ=");
    taskdump_dec(core::mem::size_of::<crate::process::types::Task>() as u64);
    // Type sizes for the per-class census mapping below (bytes, the
    // small-alloc class is the next power of two >= size).
    taskdump_raw_line(b"\nOOM-ANAT sizes: vma=");
    taskdump_dec(core::mem::size_of::<crate::mm::types::Vma>() as u64);
    taskdump_raw_line(b" file=");
    taskdump_dec(core::mem::size_of::<crate::fs::types::File>() as u64);
    taskdump_raw_line(b" dentry=");
    taskdump_dec(core::mem::size_of::<crate::fs::types::Dentry>() as u64);
    taskdump_raw_line(b" inode=");
    taskdump_dec(core::mem::size_of::<crate::fs::types::Inode>() as u64);
    taskdump_raw_line(b" mm=");
    taskdump_dec(core::mem::size_of::<crate::mm::types::MmStruct>() as u64);
    taskdump_raw_line(b" ptregs=");
    taskdump_dec(core::mem::size_of::<crate::arch::pt_regs::PtRegs>() as u64);
    taskdump_raw_line(b" fdte=");
    taskdump_dec(
        core::mem::size_of::<[Option<alloc::sync::Arc<crate::fs::types::File>>; 1024]>() as u64
            + 128,
    );
    taskdump_raw_line(b"\nOOM-ANAT buddy live pages by order:\n");
    // Leg-17 leak test: live user mms = creations - drops. If live_mm
    // >> live procs at the census moment, every spawn strands its
    // AddressSpace (and its VMA tree + vma_files pins ride along).
    {
        let mn = self::MM_NEWS.load(Ordering::Relaxed) as u64;
        let md = self::MM_DROPS.load(Ordering::Relaxed) as u64;
        taskdump_raw_line(b"OOM-ANAT mm: new=");
        taskdump_dec(mn);
        taskdump_raw_line(b" drop=");
        taskdump_dec(md);
        taskdump_raw_line(b" live=");
        taskdump_dec(mn.saturating_sub(md));
        taskdump_raw_line(b"\n");
    }
    let np = &crate::mm::hotpath::NET_PAGES;
    let mut o = 0usize;
    while o < np.len() {
        let v = np[o].load(Ordering::Relaxed);
        if v != 0 {
            taskdump_raw_line(b" o");
            taskdump_dec(o as u64);
            taskdump_raw_line(b"=");
            taskdump_dec(v as u64);
            taskdump_raw_line(b"p\n");
        }
        o += 1;
    }
    crate::mm::hotpath::census_line();
    // Leg-16: type identification for the two heaviest classes at the
    // floor (c256 ~520K live, c1024 ~84K live) — dump object headers from
    // the active slab pages; vtables/magic are symbolisable offline.
    crate::mm::hotpath::census_peek(5, 8);
    crate::mm::hotpath::census_peek(7, 8);
    // Leg-17: structural buckets (the v2 histogram alone cannot name a
    // pointer-first heterogeneous population).
    crate::mm::hotpath::census_peek3(5);
    crate::mm::hotpath::census_peek3(7);
}

/// Start the schbd kthread. Gated on the `rux:schb (retired debug=schb)` switch; called from
/// the same late-boot slot as khungtaskd (secondaries online, timer
/// wakeups routable).
pub fn schb_init() {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    // Leg-17: `censusat=<secs>` — see CENSUS_AT_SECS above. Parsed here
    // (init runs once, late boot, cmdline is stable by then).
    if let Some(v) = crate::cmdline::get_param("censusat") {
        if let Ok(secs) = v.trim().parse::<u64>() {
            CENSUS_AT_SECS.store(secs, Ordering::Release);
            crate::pr_info!("debug: schbd census timer armed at {}s", secs);
        }
    }
    if SCHB_RUNNING.load(Ordering::Acquire) {
        return;
    }
    let started =
        crate::api::process::kthread_run(schbd_fn, core::ptr::null_mut(), "schbd").is_some();
    if started {
        SCHB_RUNNING.store(true, Ordering::Release);
        crate::pr_err!(
            "debug: schbd soft-wedge heartbeat armed ({}s beat, census at {}s stalled)",
            BEAT_SECS,
            STALL_BEATS as u64 * BEAT_SECS
        );
    } else {
        crate::pr_err!("debug: schbd failed to start");
    }
}

/// Trim a comm byte array to a printable &str.
fn schb_comm_str(c: &[u8; 16]) -> &str {
    let end = c.iter().position(|&b| b == 0).unwrap_or(c.len());
    core::str::from_utf8(&c[..end]).unwrap_or("?")
}

/// schbd main loop: sleep / beat / detect / census.
extern "C" fn schbd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::types::TaskState;

    let hz = crate::config::KERNEL_HZ as u64;
    if hz == 0 {
        return 0;
    }
    let beat_jiffies = (BEAT_SECS * hz).max(1);
    let boot_jiffies = crate::timer::get_jiffies();

    let mut last_pidmax: u32 = 0;
    let mut plateau: u32 = 0;
    let mut dumped = false; // one census per plateau episode
    let mut drained_seq: u64 = 0;

    loop {
        if crate::api::process::kthread_should_stop() {
            break;
        }

        // --- Periodic sleep (khungtaskd discipline: arm a one-shot timer
        // as the only waker, mark INTERRUPTIBLE, re-check, then sleep). ---
        let my_pid = match crate::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => break,
        };
        let target = crate::timer::get_jiffies() + beat_jiffies;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        if timer_id != 0 {
            // Re-derived running-task borrows (never held across
            // schedule() below).
            crate::sched::hotpath::current().map(|t| t.set_state(TaskState::new(TaskState::INTERRUPTIBLE)));
            if crate::timer::get_jiffies() >= target {
                if let Some(t) = crate::sched::hotpath::current() {
                    t.set_state(TaskState::new(TaskState::RUNNING));
                    // A racing wake may have enqueued us mid-exit — take
                    // ourselves back off (NEW-C2 discipline).
                    crate::sched::hotpath::dequeue_task(t);
                }
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::cpu::restore_irq(true);
                crate::sched::hotpath::schedule();
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full: stay runnable, yield one round, retry.
            crate::sched::hotpath::schedule();
        }

        // --- Beat: one task-table walk + one binder count walk. ---
        let mut max_pid: u32 = 0;
        let mut n_tasks: usize = 0;
        let mut n_procs: usize = 0; // leg-16: tgid leaders (processes vs threads)
        let mut n_run: usize = 0; // state bits clear: runnable or running
        let mut n_mark: usize = 0; // state clear + on_cpu: executing OR orphaned mark
        let mut n_s: usize = 0; // INTERRUPTIBLE
        let mut n_d: usize = 0; // UNINTERRUPTIBLE
        let mut young_pid: u32 = 0;
        let mut young_comm = [0u8; 16];
        crate::sched::hotpath::for_each_task(|task| {
            // The walk hands &Task under the bucket lock; plain-field
            // reads only (same contract as khungtaskd's check_tasks).
            {
                let t = task;
                let pid = t.pid();
                n_tasks += 1;
                if t.tgid() == pid {
                    n_procs += 1;
                }
                if pid > max_pid {
                    max_pid = pid;
                }
                if pid >= young_pid {
                    young_pid = pid;
                    young_comm = *t.comm();
                }
                let b = t.state().bits();
                if b & 0x37 == 0 {
                    if t.on_cpu() {
                        n_mark += 1;
                    } else {
                        n_run += 1;
                    }
                } else if b & 0x02 != 0 {
                    n_d += 1;
                } else if b & 0x01 != 0 {
                    n_s += 1;
                }
            }
        });
        let (bprocs, bwait, bwait15) = crate::api::ipc::schb_beat();
        let flow = max_pid.saturating_sub(last_pidmax);
        let t_secs = crate::timer::get_jiffies().saturating_sub(boot_jiffies) / hz;

        {
            // Leg-15 pace snapshot (all reads relaxed, schbd context):
            // cumulative COW attempts + jiffies spent in handle_cow_fault,
            // direct-reclaim rounds + jiffies + max round, slowpath
            // entries, zone free pages, kernel heap used KB.
            let (cow, cowj, drct, drctj, drctmax, slow) = pace_snapshot();
            let zfree = crate::mm::hotpath::first_online_node()
                .map(|n| n.free_pages() as u64)
                .unwrap_or(0);
            let heapk = crate::mm::hotpath::buddy_stats().used_bytes / 1024;
            // Leg-16: fire the allocator anatomy dump when the heap is
            // ~80% full — comfortably before the floor/OOM would, so a
            // roll that never quite reaches the fake-OOM still banks the
            // census (one-shot; the OOM path reuses the same guard).
            {
                let st = crate::mm::hotpath::buddy_stats();
                if st.used_bytes * 5 >= st.heap_size * 4 {
                    oom_anat_dump();
                }
            }
            // Leg-17: fixed-time census (see CENSUS_AT_SECS). Whichever
            // trigger wins first, the DONE guard makes it one-shot.
            if t_secs >= CENSUS_AT_SECS.load(Ordering::Acquire) {
                oom_anat_dump();
            }
            let mut w = ConsoleWriter::new();
            let _ = writeln!(
                w,
                "SC-HB t={}s pidmax={} flow={} young={} tasks={} R={} M={} S={} D={} bproc={} bw={} bw15={} pace: cow={} cowJ={} drct={} drctJ={} drctMaxJ={} slow={} zfree={} heapK={}",
                t_secs, max_pid, flow, schb_comm_str(&young_comm), n_tasks, n_run, n_mark, n_s, n_d,
                bprocs, bwait, bwait15, cow, cowj, drct, drctj, drctmax, slow, zfree, heapk
            );
            // Ring row per beat (debug-migrate round 5): the greppable
            // heartbeat outside the serial stream.
            crate::trace::trace_schb(0, max_pid, flow as u64, bwait15 as u64);
        }

        // Leg-16 anatomy line: decomposes heapK at the floor. procs =
        // tgid leaders; kN/kK/kHW = live kernel stacks / their bytes /
        // boot-max high-water (probe armed by rux:schb (retired debug=schb)); smallK vs
        // smallPgK = live small-object estimate vs slab pages actually
        // held (the delta is packing slack); oN = buddy per-order live
        // pages (o5 = the 128KB kernel-stack class on this config).
        {
            let (kn, khw) = crate::process::hotpath::kstack_anat();
            // kb/ks = packed-slab blocks/live slots (x86_64 only; 0/0
            // reads as "no slab" on arches that keep plain order-5
            // stacks — see process/task.rs).
            let (kb, ks, _fb) = crate::process::hotpath::kstack_slab_stats();
            let (small_k, small_pg_k) = crate::mm::hotpath::live_stats_kib();
            let np = &crate::mm::hotpath::NET_PAGES;
            let mut w = ConsoleWriter::new();
            let _ = writeln!(
                w,
                "SC-ANAT t={}s procs={} kN={} kK={} kHW={} kb={} ks={} smallK={} smallPgK={} o0={} o1={} o2={} o3={} o4={} o5={} o6={} o7={} o8={}",
                t_secs, n_procs, kn, kn * (crate::config::KERNEL_STACK_SIZE as u64) / 1024, khw,
                kb, ks,
                small_k, small_pg_k,
                np[0].load(Ordering::Relaxed), np[1].load(Ordering::Relaxed),
                np[2].load(Ordering::Relaxed), np[3].load(Ordering::Relaxed),
                np[4].load(Ordering::Relaxed), np[5].load(Ordering::Relaxed),
                np[6].load(Ordering::Relaxed), np[7].load(Ordering::Relaxed),
                np[8].load(Ordering::Relaxed)
            );
        }

        // Drain the proc-open ring (SC-PROC lines) after the beat line.
        {
            let cur = PROC_SEQ.load(Ordering::Relaxed);
            if cur > drained_seq {
                let ring = PROC_RING.lock();
                let mut w = ConsoleWriter::new();
                let mut seq = drained_seq;
                while seq < cur {
                    let (s, pid, j) = ring[(seq as usize) % PROC_RING_LEN];
                    // A wrapped/overwritten slot shows a stale seq: skip
                    // silently (bounded ring; beats drain faster than 128
                    // opens in practice).
                    if s == seq + 1 {
                        let _ = writeln!(w, "SC-PROC seq={} pid={} @{}j", s, pid, j);
                    }
                    seq += 1;
                }
                drop(ring);
                drained_seq = cur;
            }
        }

        // Re-arm on REAL recovery only: a beat with healthy flow (>= 20)
        // proves the frontier moves and any prior census's heal worked, so
        // a later re-freeze gets a fresh census. bw15 alone must NOT gate
        // the re-arm - sc202 recovered to flow 27-54 with bw15 still ~300
        // (the old parked cohort never drains), and bw15-gating kept the
        // plateau counter alive forever, permanently disarming the census.
        // sc206: the else-if had the same defect through branch ORDER -
        // after the first census the parked cohort keeps bw15 >=
        // WAITER15_FLOOR on every beat, so the first arm of the OR always
        // fired and the flow >= 20 re-arm below was dead code: sc206's
        // t=537 flow=80 burst never re-armed the census and a 410 s
        // qualifying plateau (t=547-957, bw15 223-244) got no census.
        // Test the recovery arm FIRST: a flow >= 20 beat is a real
        // frontier move regardless of the parked waiter count.
        if flow >= 20 {
            plateau = 0;
            dumped = false;
        } else if flow <= CRAWL_FLOW_MAX || bwait15 >= WAITER15_FLOOR {
            plateau = plateau.saturating_add(1);
        }
        last_pidmax = max_pid;

        if plateau >= STALL_BEATS && !dumped && n_tasks >= MIN_TASKS && t_secs >= MIN_BOOT_SECS {
            dumped = true;
            wedge_census(
                t_secs,
                max_pid,
                plateau as u64 * BEAT_SECS,
            );
        }
    }

    SCHB_RUNNING.store(false, Ordering::Release);
    0
}

/// The one-shot wedge census (fires after the wedge has settled, so its
/// output burn cannot decide the race it reports on).
fn wedge_census(t_secs: u64, pidmax: u32, plateau_secs: u64) {
    {
        let mut w = ConsoleWriter::new();
        let _ = writeln!(
            w,
            "SC-WEDGE t={}s pidmax={} plateau={}s census begin",
            t_secs, pidmax, plateau_secs
        );
        crate::trace::trace_schb(1, pidmax, plateau_secs, 0);
    }

    // (1) Every binder read-wait thread regardless of age. stall_report
    // only prints waiters older than 120s; at plateau+60s the wedge's
    // waiters are younger than that.
    {
        let mut s = alloc::string::String::new();
        crate::api::ipc::schb_waiter_census(&mut s);
        let mut w = ConsoleWriter::new();
        let _ = write!(w, "{}", s);
    }

    // (2) Scheduler view of every runnable (not on_cpu) and D-state task.
    // Collect raw fields under the pid-hash walk; re-find each task for
    // the runqueue-linked diagnostic afterwards (never take a GRQ
    // diagnostic lock under a pid-hash bucket lock).
    struct Snap {
        pid: u32,
        comm: [u8; 16],
        bits: u32,
        on_cpu: bool,
        claim: i32,
        pk: u32,
        ti_cpu: i32,
    }
    let mut snaps: alloc::vec::Vec<Snap> = alloc::vec::Vec::new();
    crate::sched::hotpath::for_each_task(|task| {
        // Plain-field reads under the bucket lock (khungtaskd contract).
        {
            let t = task;
            let b = t.state().bits();
            if (b & 0x37 == 0 && !t.on_cpu()) || (b & 0x02 != 0) {
                snaps.push(Snap {
                    pid: t.pid(),
                    comm: *t.comm(),
                    bits: b,
                    on_cpu: t.on_cpu(),
                    claim: t.running_on_cpu.load(Ordering::Acquire),
                    pk: t.pick_state(),
                    ti_cpu: t.ti_cpu(),
                });
            }
        }
    });
    {
        let mut s = alloc::string::String::new();
        for sn in &snaps {
            // Diagnostic re-find via TaskBorrow (short read; the task may
            // have exited between walk and now — lookup returns None).
            let linked = crate::process::types::TaskBorrow::lookup(sn.pid);
            let linked_str = if linked.is_none() {
                "gone"
            } else {
                match crate::sched::hotpath::grq_diag_cfs_linked(linked.unwrap().as_ptr()) {
                    Some(true) => "1",
                    Some(false) => "0",
                    None => "n/a",
                }
            };
            let _ = writeln!(
                s,
                "SC-WEDGE-{} pid={} comm={} state={:#x} oncpu={} claim={} pk={} ti_cpu={} linked={}",
                if sn.bits & 0x02 != 0 { "D" } else { "R" },
                sn.pid,
                schb_comm_str(&sn.comm),
                sn.bits,
                sn.on_cpu,
                sn.claim,
                sn.pk,
                sn.ti_cpu,
                linked_str
            );
        }
        let mut w = ConsoleWriter::new();
        let _ = write!(w, "{}", s);
    }

    // (3) Kernel stall machinery at warn level: lift the console loglevel
    // to 4 so binder-stall / freeze_scan / WAKE-RUN-LEAK print (at
    // loglevel=3 their records are dropped before the ring — see the
    // module doc), run one stall_report pass, restore verbatim. The pass
    // also re-kicks parked readers (idempotent heal khungtaskd already
    // runs every 60s); if that unwedges the system the next heartbeats
    // show the flow resuming — itself a diagnostic.
    {
        let mut w = ConsoleWriter::new();
        let _ = writeln!(w, "SC-WEDGE stall_report + freeze_scan (loglevel lifted to 4):");
    }
    let saved = crate::printk::get_console_loglevel();
    if saved < 4 {
        crate::printk::set_console_loglevel(4);
    }
    crate::api::ipc::stall_report();
    // rux:sleepsnap (retired debug=sleepsnap) names the syscall every S-state task is parked in —
    // the wedge census's blind spot is a server parked OUTSIDE binder
    // (samgr in sc201: not runnable, not binder-waiting, clients owed
    // 200s+). sleep_snapshot is self-gated on its own debug switch; boots
    // that arm rux:schb (retired debug=schb),sleepsnap get its census inside this lifted
    // window (its khungtaskd-driven prints at loglevel 3 are dropped
    // pre-ring, so the 60s cadence costs no UART burn).
    crate::api::ipc::sleep_snapshot();
    if saved < 4 {
        crate::printk::set_console_loglevel(saved);
    }

    // (4) Userspace tail: quietcon diverts the /dev/console stream into
    // the printk ring; the newest records around the wedge are the
    // userspace last-known-good boundary.
    {
        let tail = crate::printk::ring_tail(RING_TAIL);
        let mut w = ConsoleWriter::new();
        let _ = writeln!(w, "SC-WEDGE printk ring tail ({} records, oldest first):", RING_TAIL);
        let _ = write!(w, "{}", tail);
        let _ = writeln!(w, "SC-WEDGE census end");
    }
}


// ════════════════ fsw ═════════════════════════════════
// ext4/bio cost-anatomy counters (perf-audit line, cmdline `fsw=1`).
//
// Leg-18 F4 diagnosis: the ext4 metadata family measures 100-300x Linux
// per-op (open 271 us, fsync 90 ms). This probe counts the VOLUMES under
// those latencies — block-cache hits/misses, synchronous writes,
// evictions, journal events, directory scans, allocator round trips,
// big-lock acquisitions — so the cost centers can be ranked by
// (counter delta) x (per-event cost implied by the syscall table),
// instead of guessing from code reading.
//
// Discipline: same contract as syscost section (hotpath.rs) — own cmdline key (`fsw=1`),
// lazy-armed after cmdline init, one dump every FSW_DUMP_SECS from the
// timer softirq, zero traffic when off, Relaxed atomics only (the counts
// are statistics, not invariants).
//
// Dump format (stable, grep-able; two compact lines, emitted at :30 past
// each minute to avoid the console-emit race with syscost's dump — pa28):
// ```text
// FSW1 t=123s br=<bread> rd=<dev reads> wr=<dev writes> sd=<sync_dirty>/<skip> ev=<evict>/<dirty> sa=<sync_all> pb=<pub flush>.<records> ps=<scoped>.<own records> pc=<constraint flushes> ph=<high-water drains> pb2=<batch drains> pm=<queue max> lvl=<console_loglevel> es=<console emits skipped>
// FSW2 t=123s ri=<read_inode> wi=<write_inode> lk=<dir lookups>.<blocks> ls=<list_dir> ni=<new_inode> ar=<alloc runs> fr=<free runs> j=<jstart>.<jstop>.<jlazy>.<jcommit_fast>.<jrevoke> bl=<biglock>.<contended> w=<walks>.<hits>.<misses>
// ```
//
// `lvl`/`es` exist so the two pa28 probe-bug classes are separable from
// the serial alone (leg 18 paid two boot iterations to tell them apart):
// a vanished round with `es` climbing = emit-lock loss (this probe dumps
// from the timer softirq, whose console-emit acquire budget is 100µs —
// printing right behind syscost's ~70-line dump lost that race 11/12
// rounds); a vanished round with `es` flat and `lvl` < 4 = the records
// were dropped before the ring by the printk level filter (pa28-a: the
// first build dumped via pr_info, dead at the pa-boot default loglevel
// 4 — dump must stay at pr_warn or louder). Both fields are read once
// per dump; no hot-path cost.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Dump cadence in seconds (same as syscost's 60 s).
const FSW_DUMP_SECS: u64 = 60;

static FSW_LAST_DUMP: AtomicU64 = AtomicU64::new(0);

// === bio layer ============================================================
pub static BREAD: AtomicU64 = AtomicU64::new(0);
/// Actual device reads (block-cache misses that went to disk).
pub static DEV_READ: AtomicU64 = AtomicU64::new(0);
/// Actual device writes from BufferHead::sync() (every synced 4K block:
/// inline write-through, eviction writeback, journal write-through, sync_all).
pub static DEV_WRITE: AtomicU64 = AtomicU64::new(0);
/// sync_dirty_buffer calls that performed (or attempted) a sync.
pub static SYNC_DIRTY: AtomicU64 = AtomicU64::new(0);
/// sync_dirty_buffer calls suppressed by an active journal handle.
pub static SYNC_DIRTY_SKIP: AtomicU64 = AtomicU64::new(0);
/// LRU evictions completed.
pub static EVICT: AtomicU64 = AtomicU64::new(0);
/// Evictions whose victim was dirty (paid a synchronous write).
pub static EVICT_DIRTY: AtomicU64 = AtomicU64::new(0);
/// sync_buffers() whole-cache drains.
pub static SYNC_ALL: AtomicU64 = AtomicU64::new(0);
/// flush_publications() drains that had work (records processed).
pub static PUB_FLUSH: AtomicU64 = AtomicU64::new(0);
pub static PUB_RECORDS: AtomicU64 = AtomicU64::new(0);
/// Scoped drains (flush_publications_scoped): the calling task's OWN
/// records published (leg-30A per-cookie fix; the world-drain latency
/// family — the 43-min hiview fdatasync class — moves off PUB_FLUSH onto
/// these).
pub static PUB_SCOPED: AtomicU64 = AtomicU64::new(0);
pub static PUB_SCOPED_RECORDS: AtomicU64 = AtomicU64::new(0);
/// Per-block constraint flushes (flush_publication_constraints) before an
/// out-of-band write of an order-constrained block (commit write-through,
/// inline sync, eviction).
pub static PUB_CSTR: AtomicU64 = AtomicU64::new(0);
/// High-water producer-paid drains (publication_defer past PUB_HIGH_WATER).
pub static PUB_HIWAT: AtomicU64 = AtomicU64::new(0);
/// Leg-55: mkdirs counted at the scoped-flush point (the lazy-variant
/// follow-up reads this against the phase mix).
pub static PUB_LAZY: AtomicU64 = AtomicU64::new(0);
/// Op-boundary batched drains (publication_batch_drain past PUB_BATCH) —
/// the amortized creator-paid drain that keeps the queue short now that
/// per-mkdir scoped drains are gone.
pub static PUB_BATCH_DRAINS: AtomicU64 = AtomicU64::new(0);
/// Maximum pending-publication queue length observed.
pub static PUB_QMAX: AtomicU64 = AtomicU64::new(0);

// === ext4 =================================================================
pub static READ_INODE: AtomicU64 = AtomicU64::new(0);
pub static WRITE_INODE: AtomicU64 = AtomicU64::new(0);
/// ext4 directory lookups (linear scans over the directory's blocks).
pub static DIR_LOOKUP: AtomicU64 = AtomicU64::new(0);
/// Directory blocks scanned by those lookups.
pub static DIR_BLOCKS: AtomicU64 = AtomicU64::new(0);
/// list_dir calls (getdents64 re-materializes the whole directory each call).
pub static LIST_DIR: AtomicU64 = AtomicU64::new(0);
pub static NEW_INODE: AtomicU64 = AtomicU64::new(0);
/// Allocator runs (single block = run of 1): one bitmap read + one bitmap
/// sync write + one group-desc sync write + one SUPERBLOCK sync write each.
pub static ALLOC_RUN: AtomicU64 = AtomicU64::new(0);
pub static FREE_RUN: AtomicU64 = AtomicU64::new(0);

// === jbd2 =================================================================
pub static J_START: AtomicU64 = AtomicU64::new(0);
pub static J_STOP: AtomicU64 = AtomicU64::new(0);
/// Stops that left the transaction open for batching (lazy commit).
pub static J_LAZY: AtomicU64 = AtomicU64::new(0);
/// Write-through fast-path commits (one sync write per registered buffer).
pub static J_COMMIT_FAST: AtomicU64 = AtomicU64::new(0);
pub static J_COMMIT_FAST_BUFS: AtomicU64 = AtomicU64::new(0);
/// Slow-path commits (descriptor + data + revoke + commit blocks).
pub static J_COMMIT_SLOW: AtomicU64 = AtomicU64::new(0);
pub static J_REVOKE: AtomicU64 = AtomicU64::new(0);

// === serialization =======================================================
/// EXT4_BIG_LOCK acquisitions.
pub static BIGLOCK: AtomicU64 = AtomicU64::new(0);
/// Acquisitions that fell past the adaptive spin (queued/spun for the lock).
pub static BIGLOCK_CONTEND: AtomicU64 = AtomicU64::new(0);

// === op-cost anatomy (perf-fs lane 2026-10-31) ============================
/// Whole-bracket ext4_mkdir duration (ENTRY_MUTEX guard to journal stop),
/// microseconds: count / cumulative / max. The residual p6_mkdrmdir floor
/// (~2.5ms/pair under load with zero steady-state device I/O) needs an
/// in-op cost attribution; this bracket timer plus the registration-scan
/// counter below bracket the two suspects (CPU in the op vs jbd2 dedup
/// scans that grow with the running transaction's unique-block set).
pub static MKDIR_US_N: AtomicU64 = AtomicU64::new(0);
pub static MKDIR_US_SUM: AtomicU64 = AtomicU64::new(0);
pub static MKDIR_US_MAX: AtomicU64 = AtomicU64::new(0);
/// Same bracket timer for ext4_rmdir / ext4_unlink (us).
pub static RMDIR_US_SUM: AtomicU64 = AtomicU64::new(0);
pub static RMDIR_US_MAX: AtomicU64 = AtomicU64::new(0);
pub static UNLINK_US_SUM: AtomicU64 = AtomicU64::new(0);
pub static UNLINK_US_MAX: AtomicU64 = AtomicU64::new(0);
/// jbd2_journal_get_write_access dedup-scan iterations (the linear
/// already-tracked walk over t_dirty_buffers): grows O(set) per metadata
/// registration between commits.
pub static J_DEDUP_ITERS: AtomicU64 = AtomicU64::new(0);

// === VFS walk ============================================================
/// path_lookup calls.
pub static WALK: AtomicU64 = AtomicU64::new(0);
/// Walk components resolved from the dentry tree.
pub static WALK_HIT: AtomicU64 = AtomicU64::new(0);
/// Walk components that needed a filesystem lookup (dcache miss).
pub static WALK_MISS: AtomicU64 = AtomicU64::new(0);

// === path-walk cost anatomy (perf-path lane 2026-11-01) ==================
// Sub-microsecond phases: NS sums (converted at dump), one count each
// where the divisor is not implied by the counters above. The brackets
// live in sys_fstatat / sys_openat / path_lookup; the question they
// answer: of the measured stat/open latency, how much is (a) the syscall
// prelude (user copies, resolve_user_path, shortcut gates), (b) the walk
// prologue (root fetch, normalize, component split), (c) the per-
// component loop, and inside it (d) get_inode+permission, (e) the dentry
// child-map lookup, (f) the per-hit icache refresh, (g) mount+symlink
// follow, versus (h) getattr+copyout / File+fdtable for the tail.
pub static STAT_N: AtomicU64 = AtomicU64::new(0);
pub static STAT_PRE_NS: AtomicU64 = AtomicU64::new(0);
pub static STAT_TAIL_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_N: AtomicU64 = AtomicU64::new(0);
pub static OPEN_PRE_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_TAIL_NS: AtomicU64 = AtomicU64::new(0);
pub static WALK_PRE_NS: AtomicU64 = AtomicU64::new(0);
pub static WALK_LOOP_NS: AtomicU64 = AtomicU64::new(0);
pub static WALK_COMP_N: AtomicU64 = AtomicU64::new(0);
/// Loop arm: current.get_inode() + dir/x permission checks.
pub static WALK_A_NS: AtomicU64 = AtomicU64::new(0);
/// Loop arm: lookup_child (dentry child-map lock + compare).
pub static WALK_B_NS: AtomicU64 = AtomicU64::new(0);
/// Loop hit arm: icache refresh (global ICACHE lock path).
pub static WALK_C_NS: AtomicU64 = AtomicU64::new(0);
/// Loop arm: get_mount + follow_symlink check.
pub static WALK_D_NS: AtomicU64 = AtomicU64::new(0);
/// Loop miss arm: fs lookup + iget + dentry add (excludes A/B/D).
pub static WALK_MISSCOST_NS: AtomicU64 = AtomicU64::new(0);
/// Final dentry inode re-fetch + trailing-slash checks (walk tail).
pub static WALK_TAIL_NS: AtomicU64 = AtomicU64::new(0);
/// Walk misses whose fs lookup returned ENOENT (existence probes —
/// each one paid the full ext4 directory scan; the negative-memo's
/// target class).
pub static WALK_MISS_ENOENT: AtomicU64 = AtomicU64::new(0);
/// Negative-memo hits (ENOENT served from the bounded per-parent memo).
pub static NEGHIT: AtomicU64 = AtomicU64::new(0);
/// Stale negative memos discarded (parent mutated since insert).
pub static NEGSTALE: AtomicU64 = AtomicU64::new(0);

// === open() wall-vs-exec anatomy (21st successor) =========================
// syscost's open row is WALL time per syscall — under the OH boot's load
// it conflates kernel CPU with deschedule + spin-wait. These brackets
// split the two: OPEN_WALL_MAX names the worst wall per open,
// OPEN_EXEC_NS/OPEN_EXEC_MAX measure the sched-exec share (actual CPU
// consumed inside the syscall, from the CFS entity's exec accounting).
// exec ~= wall  -> real kernel cost (fix the fs path);
// exec << wall  -> waiting (scheduling / lock-holder descheduled).
pub static OPEN_WALL_MAX_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_EXEC_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_EXEC_MAX_NS: AtomicU64 = AtomicU64::new(0);

/// Max-update helper: keep the running maximum of `val` in `dst`.
#[inline]
fn max_update(dst: &AtomicU64, val: u64) {
    let mut cur = dst.load(Ordering::Relaxed);
    while val > cur {
        match dst.compare_exchange_weak(cur, val, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(c) => cur = c,
        }
    }
}

/// Entry stamp for the open wall/exec brackets. Returns (wall_t0,
/// exec_armed): the exec side charges the task's CFS accounting up to
/// the entry instant (the returned pre-open window is discarded); the
/// exit call then returns exactly the on-CPU time consumed inside the
/// syscall. (0, false) when the probe is off or no task context exists.
#[inline]
pub fn open_exec_stamp() -> (u64, bool) {
    if !fsw_on() {
        return (0, false);
    }
    let w0 = crate::timer::monotonic_ns_since_boot();
    match crate::sched::hotpath::current() {
        None => (w0, false),
        Some(t) => {
            t.sched_entity()
                .update_exec_runtime(crate::sched::hotpath::sched_clock());
            (w0, true)
        }
    }
}

/// Close the open wall/exec brackets opened by open_exec_stamp().
#[inline]
pub fn open_close(w0: u64, exec_armed: bool) {
    if w0 == 0 {
        return;
    }
    let wall = crate::timer::monotonic_ns_since_boot().saturating_sub(w0);
    max_update(&OPEN_WALL_MAX_NS, wall);
    if exec_armed {
        if let Some(t) = crate::sched::hotpath::current() {
            let d = t
                .sched_entity()
                .update_exec_runtime(crate::sched::hotpath::sched_clock());
            OPEN_EXEC_NS.fetch_add(d, Ordering::Relaxed);
            max_update(&OPEN_EXEC_MAX_NS, d);
        }
    }
}

pub fn note_walk_miss_enoent() {
    WALK_MISS_ENOENT.fetch_add(1, Ordering::Relaxed);
}

pub fn note_neghit() {
    NEGHIT.fetch_add(1, Ordering::Relaxed);
}

pub fn note_negstale() {
    NEGSTALE.fetch_add(1, Ordering::Relaxed);
}

// (21st successor) cmdline `negmemo=1`: bounded negative-lookup memo in
// the path walk (ext4 ENOENT results cached as negative dentry children,
// invalidated by per-parent mut_gen, capped per parent). Gated separately
// from the counters so a boot can measure the miss class with the fix
// off, then A/B with it on.
static NEG_MEMO_ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn negmemo_on() -> bool {
    match NEG_MEMO_ARMED8.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("negmemo").as_deref() == Some("1"),
            };
            NEG_MEMO_ARMED8.store(if v { 2 } else { 1 }, Ordering::Relaxed);
            crate::pr_warn!(
                "negmemo: {} (cmdline negmemo param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("negmemo")
            );
            v
        }
    }
}

/// Bracket timestamp (0 when the probe is off — skips the clock read).
#[inline]
pub fn pc_now() -> u64 {
    if fsw_on() {
        crate::timer::monotonic_ns_since_boot()
    } else {
        0
    }
}

/// Close a bracket opened with pc_now() (no-op when t0 == 0, i.e. off).
#[inline]
pub fn pc_add(dst: &AtomicU64, t0: u64) {
    if t0 != 0 {
        dst.fetch_add(
            crate::timer::monotonic_ns_since_boot().saturating_sub(t0),
            Ordering::Relaxed,
        );
    }
}

/// Probe armed? Lazy verdict after cmdline init (syscost's pa15 lesson:
/// never cache a verdict before the cmdline exists). Tri-state AtomicU8
/// exactly like syscost (0 = unresolved, 1 = off, 2 = on) plus a one-shot
/// resolution print so a dead probe is diagnosable from the serial.
static ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn fsw_on() -> bool {
    match ARMED8.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            // Do NOT cache a verdict before the cmdline exists (pa15).
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("fsw").as_deref() == Some("1"),
            };
            ARMED8.store(if v { 2 } else { 1 }, Ordering::Relaxed);
            crate::pr_warn!(
                "fsw: {} (cmdline fsw param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("fsw")
            );
            v
        }
    }
}

macro_rules! bump {
    ($c:expr) => {
        if self::fsw_on() {
            $c.fetch_add(1, Ordering::Relaxed);
        }
    };
    ($c:expr, $n:expr) => {
        if self::fsw_on() {
            $c.fetch_add($n as u64, Ordering::Relaxed);
        }
    };
}

// bio layer notes
#[inline]
pub fn note_bread() {
    bump!(BREAD);
}
#[inline]
pub fn note_dev_read() {
    bump!(DEV_READ);
}
#[inline]
pub fn note_dev_write() {
    bump!(DEV_WRITE);
}
#[inline]
pub fn note_sync_dirty() {
    bump!(SYNC_DIRTY);
}
#[inline]
pub fn note_sync_dirty_skip() {
    bump!(SYNC_DIRTY_SKIP);
}
#[inline]
pub fn note_evict(dirty: bool) {
    if self::fsw_on() {
        EVICT.fetch_add(1, Ordering::Relaxed);
        if dirty {
            EVICT_DIRTY.fetch_add(1, Ordering::Relaxed);
        }
    }
}
#[inline]
pub fn note_sync_all() {
    bump!(SYNC_ALL);
}
#[inline]
pub fn note_pub_flush(records: usize) {
    if self::fsw_on() {
        PUB_FLUSH.fetch_add(1, Ordering::Relaxed);
        PUB_RECORDS.fetch_add(records as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_pub_scoped(records: usize) {
    if self::fsw_on() {
        PUB_SCOPED.fetch_add(1, Ordering::Relaxed);
        PUB_SCOPED_RECORDS.fetch_add(records as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_pub_cstr() {
    bump!(PUB_CSTR);
}
#[inline]
pub fn note_pub_hiwat() {
    bump!(PUB_HIWAT);
}
#[inline]
pub fn note_pub_batch() {
    bump!(PUB_BATCH_DRAINS);
}
/// Record one whole-bracket ext4_mkdir duration (microseconds).
#[inline]
pub fn note_mkdir_us(delta_us: u64) {
    if self::fsw_on() {
        MKDIR_US_N.fetch_add(1, Ordering::Relaxed);
        MKDIR_US_SUM.fetch_add(delta_us, Ordering::Relaxed);
        let mut cur = MKDIR_US_MAX.load(Ordering::Relaxed);
        while delta_us > cur {
            match MKDIR_US_MAX.compare_exchange_weak(
                cur,
                delta_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}
/// jbd2 registration dedup-scan iterations (see J_DEDUP_ITERS).
#[inline]
pub fn note_j_dedup(iters: usize) {
    bump!(J_DEDUP_ITERS, iters);
}
/// Freed-block dirty-buffer drops (bio::drop_freed_block) — the
/// discard-on-free path that keeps dead data out of publication drains.
pub static DROP_BLOCK: AtomicU64 = AtomicU64::new(0);
#[inline]
pub fn note_drop_block() {
    bump!(DROP_BLOCK);
}
/// Record one whole-bracket ext4_rmdir duration (microseconds). Count
/// rides MKDIR-class counters' shape: RMDIR count = FREE_RUN-free path is
/// not 1:1, so a dedicated count is not kept — use MKDIR_US_N for mkdir
/// only; rmdir/unlink report sum/max.
#[inline]
pub fn note_rmdir_us(delta_us: u64) {
    if self::fsw_on() {
        RMDIR_US_SUM.fetch_add(delta_us, Ordering::Relaxed);
        let mut cur = RMDIR_US_MAX.load(Ordering::Relaxed);
        while delta_us > cur {
            match RMDIR_US_MAX.compare_exchange_weak(
                cur,
                delta_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}
/// Record one whole-bracket ext4_unlink duration (microseconds).
#[inline]
pub fn note_unlink_us(delta_us: u64) {
    if self::fsw_on() {
        UNLINK_US_SUM.fetch_add(delta_us, Ordering::Relaxed);
        let mut cur = UNLINK_US_MAX.load(Ordering::Relaxed);
        while delta_us > cur {
            match UNLINK_US_MAX.compare_exchange_weak(
                cur,
                delta_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}
/// ext4_rmdir_inner per-phase nanosecond sums (monotonic clock):
/// find+is_dir_empty / delete_entry / inode writes / free_inode /
/// free_inode_blocks. Dumped as FSW3.
pub static RMD_PHASE1_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE2_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE3_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE4_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE5_NS: AtomicU64 = AtomicU64::new(0);
/// ext4_rmdir_inner phase-N max (microseconds), same order.
pub static RMD_PHASE_MAX_US: [AtomicU64; 5] = [
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];
#[inline]
pub fn note_rmdir_phases(p1: u64, p2: u64, p3: u64, p4: u64, p5: u64) {
    if !self::fsw_on() {
        return;
    }
    RMD_PHASE1_NS.fetch_add(p1, Ordering::Relaxed);
    RMD_PHASE2_NS.fetch_add(p2, Ordering::Relaxed);
    RMD_PHASE3_NS.fetch_add(p3, Ordering::Relaxed);
    RMD_PHASE4_NS.fetch_add(p4, Ordering::Relaxed);
    RMD_PHASE5_NS.fetch_add(p5, Ordering::Relaxed);
    let ps = [p1, p2, p3, p4, p5];
    for (i, &v) in ps.iter().enumerate() {
        let us = v / 1000;
        let mut cur = RMD_PHASE_MAX_US[i].load(Ordering::Relaxed);
        while us > cur {
            match RMD_PHASE_MAX_US[i].compare_exchange_weak(
                cur, us, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => cur = x,
            }
        }
    }
}
#[inline]
pub fn note_pub_lazy() {
    bump!(PUB_LAZY);
}
#[inline]
pub fn note_pub_qmax(len: usize) {
    if self::fsw_on() {
        // Relaxed max (statistics only).
        let mut cur = PUB_QMAX.load(Ordering::Relaxed);
        let l = len as u64;
        while l > cur {
            match PUB_QMAX.compare_exchange_weak(
                cur,
                l,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}

// ext4 layer notes
#[inline]
pub fn note_read_inode() {
    bump!(READ_INODE);
}
#[inline]
pub fn note_write_inode() {
    bump!(WRITE_INODE);
}
#[inline]
pub fn note_dir_lookup(blocks: usize) {
    if self::fsw_on() {
        DIR_LOOKUP.fetch_add(1, Ordering::Relaxed);
        DIR_BLOCKS.fetch_add(blocks as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_list_dir() {
    bump!(LIST_DIR);
}
#[inline]
pub fn note_new_inode() {
    bump!(NEW_INODE);
}
#[inline]
pub fn note_alloc_run() {
    bump!(ALLOC_RUN);
}
#[inline]
pub fn note_free_run() {
    bump!(FREE_RUN);
}

// jbd2 notes
#[inline]
pub fn note_j_start() {
    bump!(J_START);
}
#[inline]
pub fn note_j_stop() {
    bump!(J_STOP);
}
#[inline]
pub fn note_j_lazy() {
    bump!(J_LAZY);
}
#[inline]
pub fn note_j_commit_fast(bufs: usize) {
    if self::fsw_on() {
        J_COMMIT_FAST.fetch_add(1, Ordering::Relaxed);
        J_COMMIT_FAST_BUFS.fetch_add(bufs as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_j_commit_slow() {
    bump!(J_COMMIT_SLOW);
}
#[inline]
pub fn note_j_revoke() {
    bump!(J_REVOKE);
}

// serialization notes
#[inline]
pub fn note_biglock() {
    bump!(BIGLOCK);
}
#[inline]
pub fn note_biglock_contend() {
    bump!(BIGLOCK_CONTEND);
}

// VFS walk notes
#[inline]
pub fn note_walk() {
    bump!(WALK);
}
#[inline]
pub fn note_walk_hit() {
    bump!(WALK_HIT);
}
#[inline]
pub fn note_walk_miss() {
    bump!(WALK_MISS);
}

/// Periodic dump from the timer softirq. Cadence is 60 s OFFSET BY half an
/// interval from syscost's dumps (pa28 lesson: my four lines immediately
/// after syscost's ~70-line dump lost the console-emit race every round
/// except once in 12 — one line survived at t=420; the counters themselves
/// were fine). Dumping at :30 past each minute keeps the emission window
/// free. Output is TWO compact lines (RECORD_TEXT_SIZE caps a record at
/// 256 bytes — keep each line short enough not to truncate).
pub fn fsw_tick(now_jiffies: u64) {
    if !fsw_on() {
        return;
    }
    let last = FSW_LAST_DUMP.load(Ordering::Relaxed);
    if last == 0 {
        // First call: park the gate half an interval ahead.
        FSW_LAST_DUMP.store(now_jiffies + FSW_DUMP_SECS * crate::config::KERNEL_HZ as u64 / 2, Ordering::Relaxed);
        return;
    }
    if now_jiffies.saturating_sub(last) < FSW_DUMP_SECS * crate::config::KERNEL_HZ as u64 {
        return;
    }
    FSW_LAST_DUMP.store(now_jiffies, Ordering::Relaxed);
    // The report body lives in the debug crate (crate-flip rung
    // debug); the row fires only on the dump cadence.
    crate::api::debug::fsw_dump();
}

/// The counter snapshot (moved to the debug crate): same-named
/// wrapper so the shutdown cascade's historical call site is unchanged;
/// dispatches the hook row.
pub fn fsw_dump() {
    crate::api::debug::fsw_dump();
}


// ════════════════ unixrd ══════════════════════════════
// rux:unixrd (retired debug=unixrd) — AF_UNIX hot-path per-op phase anatomy probe (39th lane).
//
// The hilogd storm measures recvmsg 28-58 us / recvfrom 9 us / writev
// 22 us per-op (syscost per-nr tables) against the reference kernel's
// ~2-5 us on the same flood — a 90k/s socket-op storm that saturates
// both CPUs and starves the cooperative FFRT userspace into the wedge
// cascade (38th close, u2v6). The syscost per-nr averages bracket the
// WHOLE syscall; they cannot say WHERE inside the call the time goes.
// This probe decomposes the three unix hot entries into phases and
// reports cycle totals per phase on the syscost dump cadence:
//
// recvmsg AF_UNIX branch (RM_*):
// - `outer` — the whole AF_UNIX branch (fd->socket resolve to return)
// - `iov`   — the shared iovec snapshot loop (all families; the storm
//             is unix-dominated so the attribution holds)
// - `alloc` — the kernel bounce buffer `vec![0u8; total_buf_len]`
//             (all families; `allocb` = summed byte request)
// - `recv`  — unix_recv_ctl: queue lock, drain copy, wake/notify
// - `cout`  — the scatter copy_to_user back to the iovecs
// - `cmsg`  — deliver_unix_cmsgs (SCM_RIGHTS install + SCM_CREDENTIALS
//             emission; `cred` = calls that emitted a cred cmsg)
// - `addr`  — put_sockaddr_un source-address write-back
//
// recvfrom AF_UNIX branch (RF_*): the hilogd 2-byte length peek rides
// this entry (`peek` count). Phases: outer/alloc/recv/cout.
//
// writev gathered-dgram path (WV_*): libhilog sends every record as
// writev(header, tag, content) on its connected DGRAM — one gather
// allocation + copy_from_user per iovec, then the owned send (queue
// push + cred stamp + wake). Phases: outer/gather/send.
//
// All sums are raw monotonic-ns; per-op averages = sum / n. Hooks sit
// behind one `unixrd=1` cmdline param (cctx tri-state pattern): one
// relaxed load per call when on, zero cost when off. OFF by default.
//
// Dumped from syscost's periodic dump (45s-mod-60 cadence) and the
// shutdown cascade — the wedge-era machine never reaches reboot(2)
// cleanly under the boot driver, so the periodic line is the evidence
// channel.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

static UNIXRD_ENABLED: AtomicBool = AtomicBool::new(false);

// === recvmsg AF_UNIX =======================================================

pub static RM_N: AtomicU64 = AtomicU64::new(0);
/// AF_UNIX recvmsg that returned data.
pub static RM_OK_N: AtomicU64 = AtomicU64::new(0);
/// AF_UNIX recvmsg that returned EAGAIN (empty queue).
pub static RM_EA_N: AtomicU64 = AtomicU64::new(0);
/// AF_UNIX recvmsg that returned another error.
pub static RM_ERR_N: AtomicU64 = AtomicU64::new(0);
/// MSG_PEEK variant calls.
pub static RM_PEEK_N: AtomicU64 = AtomicU64::new(0);
/// Calls whose cmsg chain carried SCM_CREDENTIALS (receiver SO_PASSCRED).
pub static RM_CRED_N: AtomicU64 = AtomicU64::new(0);

pub static RM_OUTER_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_IOV_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_ALLOC_NS: AtomicU64 = AtomicU64::new(0);
/// Byte request behind RM_ALLOC (sum of total_buf_len).
pub static RM_ALLOC_B: AtomicU64 = AtomicU64::new(0);
pub static RM_RECV_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_COUT_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_CMSG_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_ADDR_NS: AtomicU64 = AtomicU64::new(0);

// === recvfrom AF_UNIX ======================================================

pub static RF_N: AtomicU64 = AtomicU64::new(0);
pub static RF_OK_N: AtomicU64 = AtomicU64::new(0);
pub static RF_EA_N: AtomicU64 = AtomicU64::new(0);
pub static RF_ERR_N: AtomicU64 = AtomicU64::new(0);
pub static RF_PEEK_N: AtomicU64 = AtomicU64::new(0);

pub static RF_OUTER_NS: AtomicU64 = AtomicU64::new(0);
pub static RF_ALLOC_NS: AtomicU64 = AtomicU64::new(0);
/// Byte request behind RF_ALLOC.
pub static RF_ALLOC_B: AtomicU64 = AtomicU64::new(0);
pub static RF_RECV_NS: AtomicU64 = AtomicU64::new(0);
pub static RF_COUT_NS: AtomicU64 = AtomicU64::new(0);

// === writev gathered-dgram =================================================

pub static WV_N: AtomicU64 = AtomicU64::new(0);
pub static WV_OK_N: AtomicU64 = AtomicU64::new(0);
/// Sender-side EAGAIN (target queue full / nonblock).
pub static WV_EA_N: AtomicU64 = AtomicU64::new(0);
pub static WV_ERR_N: AtomicU64 = AtomicU64::new(0);

pub static WV_OUTER_NS: AtomicU64 = AtomicU64::new(0);
pub static WV_GATHER_NS: AtomicU64 = AtomicU64::new(0);
/// Gathered byte total behind WV_GATHER.
pub static WV_GATHER_B: AtomicU64 = AtomicU64::new(0);
pub static WV_SEND_NS: AtomicU64 = AtomicU64::new(0);

/// Bracket timestamp (0 when the probe is off — skips the clock read).
/// Calls on() so the FIRST call after cmdline init resolves the tri-state
/// (one relaxed load per call afterwards).
#[inline]
pub fn unixrd_now() -> u64 {
    if unixrd_on() {
        crate::timer::monotonic_ns_since_boot()
    } else {
        0
    }
}

/// Close a bracket opened with now() (no-op when t0 == 0, i.e. off).
#[inline]
pub fn unixrd_add(dst: &AtomicU64, t0: u64) {
    if t0 != 0 {
        dst.fetch_add(
            crate::timer::monotonic_ns_since_boot().saturating_sub(t0),
            Relaxed,
        );
    }
}

/// Bump a count (no-op when off — resolves the tri-state like now()).
#[inline]
pub fn unixrd_bump(c: &AtomicU64) {
    if unixrd_on() {
        c.fetch_add(1, Relaxed);
    }
}

/// Byte-sum helper (no-op when off).
#[inline]
pub fn addb(dst: &AtomicU64, bytes: u64) {
    if unixrd_on() {
        dst.fetch_add(bytes, Relaxed);
    }
}

/// Probe armed? Tri-state exactly like cctx (0 = unresolved, 1 = off,
/// 2 = on); never cache a verdict before the cmdline exists.
static UNIXRD_ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn unixrd_on() -> bool {
    match UNIXRD_ARMED8.load(Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("unixrd").as_deref() == Some("1"),
            };
            UNIXRD_ARMED8.store(if v { 2 } else { 1 }, Relaxed);
            UNIXRD_ENABLED.store(v, Relaxed);
            crate::pr_warn!(
                "unixrd: {} (cmdline unixrd param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("unixrd")
            );
            v
        }
    }
}

fn v(c: &AtomicU64) -> u64 {
    c.load(Relaxed)
}

/// Per-op average in ns (0 when n == 0).
fn avg(sum: &AtomicU64, n: &AtomicU64) -> u64 {
    let n = v(n);
    if n == 0 {
        0
    } else {
        v(sum) / n
    }
}

/// Print the phase snapshot (syscost periodic dump + shutdown cascade).
pub fn unixrd_dump() {
    if !unixrd_on() {
        return;
    }
    let secs = crate::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    // recvmsg AF_UNIX phases. iov/alloc sum over ALL recvmsg families
    // (shared code above the family dispatch) — the storm boot is
    // unix-dominated, so sum/N vs the AF_UNIX count reads as an upper
    // bound; the informative number is the RATIO of phases.
    crate::pr_warn!(
        "UNIXRD-RM t={}s n={} ok={} ea={} err={} peek={} cred={} outer={}ns iov={}ns alloc={}ns recv={}ns cout={}ns cmsg={}ns addr={}ns allocb={}B",
        secs,
        v(&RM_N),
        v(&RM_OK_N),
        v(&RM_EA_N),
        v(&RM_ERR_N),
        v(&RM_PEEK_N),
        v(&RM_CRED_N),
        avg(&RM_OUTER_NS, &RM_N),
        avg(&RM_IOV_NS, &RM_N),
        avg(&RM_ALLOC_NS, &RM_N),
        avg(&RM_RECV_NS, &RM_N),
        avg(&RM_COUT_NS, &RM_N),
        avg(&RM_CMSG_NS, &RM_N),
        avg(&RM_ADDR_NS, &RM_N),
        if v(&RM_N) == 0 { 0 } else { v(&RM_ALLOC_B) / v(&RM_N) },
    );
    crate::pr_warn!(
        "UNIXRD-RF t={}s n={} ok={} ea={} err={} peek={} outer={}ns alloc={}ns recv={}ns cout={}ns allocb={}B",
        secs,
        v(&RF_N),
        v(&RF_OK_N),
        v(&RF_EA_N),
        v(&RF_ERR_N),
        v(&RF_PEEK_N),
        avg(&RF_OUTER_NS, &RF_N),
        avg(&RF_ALLOC_NS, &RF_N),
        avg(&RF_RECV_NS, &RF_N),
        avg(&RF_COUT_NS, &RF_N),
        if v(&RF_N) == 0 { 0 } else { v(&RF_ALLOC_B) / v(&RF_N) },
    );
    crate::pr_warn!(
        "UNIXRD-WV t={}s n={} ok={} ea={} err={} outer={}ns gather={}ns send={}ns gatherb={}B",
        secs,
        v(&WV_N),
        v(&WV_OK_N),
        v(&WV_EA_N),
        v(&WV_ERR_N),
        avg(&WV_OUTER_NS, &WV_N),
        avg(&WV_GATHER_NS, &WV_N),
        avg(&WV_SEND_NS, &WV_N),
        if v(&WV_N) == 0 { 0 } else { v(&WV_GATHER_B) / v(&WV_N) },
    );
}


// ════════════════ hilogcensus ═════════════════════════
// rux:hilogcensus (retired debug=hilogcensus) — per-caller census of the OpenHarmony log fabric
// (hilog-flood lane).
//
// The OH boot floods the hilog unix-dgram fabric at 10^4-10^6 msg/s
// (hilogd.server recvmsg matches the writers' writev ok count 1:1 —
// every message is one writev(header, tag, content) datagram from
// libhilog). syscost's 16-slot first-seen comm table cannot name the
// producers (slots fill early, the big writers print past the printk
// line truncation), and the in-guest `hilog` CLI returns nothing on
// this image. This census answers "WHO emits, WHAT tag, at what rate"
// from the kernel side:
//
// - `unix_send_pre/post` + the periodic dump — MOVED to the debug
//   service crate (kernel/debug/src/hilogcensus.rs; root-cold lane
//   2026-11-24): syscall's writev path calls them there directly
//   (sibling service dep, zero indirection), and the dump fires
//   through the DebugHooks `hilog_dump` row below.
// - `kmsg_write(buf)` — stays HERE: called by name from printk.rs's
//   kmsg_file_write and the ext4 journal seam (both framework files),
//   so rule 1/2 pins it. The kmsg table stays with it and the
//   crate-side dump reads it through the pub surface below (the
//   syscost/fws split-file pattern: markers framework-side, report
//   halves crate-side).
//
// Arming (obs-hygiene migration): the unified-framework tracepoint
// `rux:hilog_census` IS the switch — arm it via `trace_event=` on the
// cmdline or `set_event` at runtime; one relaxed load per hook when
// armed, zero when off. NOT `hilogtap`, which arms the existing
// per-record content tap in net/unix.rs — that scanner costs ~seconds
// of CPU/s at storm rates. The bespoke cmdline token `hilogcensus=1`
// survives as a compatibility ALIAS that arms the same events at boot
// (existing hunt rigs keep working; the frozen debug= switch table is
// untouched — this module was never in it). Dumped on the syscost
// periodic cadence (and the shutdown cascade): top senders, one
// bounded line each. Aggregate-as-event throughout — a per-message
// tracepoint at 10^6/s would cost more than it observes.
//
// Table honesty notes: unix slots are DIRECT-MAPPED by
// `sender_pid & (SLOTS-1)`; a colliding/reused pid resets the slot
// (EVICT counts the resets). Rate = n delta between dumps / dump
// interval, computed host-side from the printed cumulative counts.

// Kernel layer policy (docs/architecture/kernel-layers.md): this module
// is data-plane Safe Rust EXCEPT the read-only `current task`
// comm/pid snapshot in kmsg_write (the syscost flood_account pattern —
// the same single documented unsafe shape, no other unsafe here).

/// The census gate: the unified-framework event switch (live — arming
/// through tracefs `set_event` at runtime takes effect immediately,
/// which the old cached tri-state could not do).
#[inline]
pub fn hilogcensus_on() -> bool {
    crate::trace::EVENT_HILOG_CENSUS.armed()
}

/// Compatibility arm (boot, before trace_event= is applied): the old
/// bespoke cmdline token `hilogcensus=1` now just flips the unified
/// events' enable switches — same running state as
/// `trace_event=rux:hilog_census,rux:hilog_kmsg`.
pub fn compat_arm() {
    if crate::cmdline::get_cmdline().is_none() {
        return;
    }
    if crate::cmdline::get_param("hilogcensus").as_deref() == Some("1") {
        crate::trace::EVENT_HILOG_CENSUS.enabled.store(true, Relaxed);
        crate::trace::EVENT_HILOG_KMSG.enabled.store(true, Relaxed);
        crate::pr_warn!(
            "hilogcensus: compat token hilogcensus=1 armed (rux:hilog_census + rux:hilog_kmsg)"
        );
    }
}


impl KmsgSender {
    const fn new() -> Self {
        KmsgSender {
            pid: AtomicU32::new(u32::MAX),
            n: AtomicU64::new(0),
            comm: [const { AtomicU8::new(0) }; COMM],
            head: [const { AtomicU8::new(0) }; 32],
            head_len: AtomicU32::new(0),
        }
    }
}

pub const KMSG_SLOTS: usize = 256;
pub static KMSG: [KmsgSender; KMSG_SLOTS] = [const { KmsgSender::new() }; KMSG_SLOTS];

/// Hook: one userspace /dev/kmsg write (printk.rs kmsg_file_write).
pub fn kmsg_write(buf: &[u8]) {
    if !hilogcensus_on() {
        return;
    }
    let (spid, scomm): (u32, [u8; COMM]) = match crate::sched::hotpath::current() {
        // sched::current() returns a typed &mut Task reference.
        Some(t) => (t.pid() as u32, *t.comm()),
        None => return,
    };
    let slot = &KMSG[(spid as usize) & (KMSG_SLOTS - 1)];
    if slot.pid.load(Relaxed) != spid {
        slot.pid.store(spid, Relaxed);
        slot.n.store(0, Relaxed);
        for j in 0..COMM {
            slot.comm[j].store(scomm[j], Relaxed);
        }
    }
    slot.n.fetch_add(1, Relaxed);
    let hl = buf.len().min(32);
    for j in 0..hl {
        slot.head[j].store(buf[j], Relaxed);
    }
    slot.head_len.store(hl as u32, Relaxed);
}

/// Periodic dump (syscost cadence + shutdown cascade) — the body lives
/// in the debug crate; this wrapper dispatches the DebugHooks
/// `hilog_dump` row (process's reboot cascade and any framework
/// site keep the historical name).
pub fn hilogcensus_dump() {
    crate::api::debug::hilog_dump();
}


// ════════════════ spinwatch ═══════════════════════════
// rux:spinwatch (retired debug=spinwatch) — rapid zero-event epoll-return census (hilog-flood lane).
//
// The OH boot's log flood is the visible face of service loops that
// should PARK but cycle: FFRT sleepers (os_ds_service's EpollManager
// logs "in WaitTimeout, epoll_wait timeout" at 4.5k/s), mmi's dispatch
// loop, systemui's loop handler. When a task's epoll_wait returns ZERO
// events far faster than its asked timeout could expire, some fd in
// its set is mis-signalled, or its timeout arithmetic is off. This
// watch names the culprit: sys_epoll_wait's zero-event return path
// (timeout expiry AND the 10 ms re-check slice) calls `record()`;
// over the threshold (>80 zero-event returns per 200 ms) the caller
// dumps the task's whole epoll interest set (fd, wanted mask, CURRENT
// poll verdict, ops pointer for nm symbolization) once per 5 s.
//
// The walk itself lives in syscall/misc.rs (EpollFile/EpollEntry are
// private to that module); this file owns arming, per-task counting
// and the report rate limit.
//
// Arming (obs-hygiene migration): the unified-framework tracepoint
// `rux:spinwatch` IS the switch — arm it via `trace_event=` on the
// cmdline or `set_event` at runtime; one relaxed load per zero-event
// return when armed, zero when off. Each report also records into the
// framework (`rux:spinwatch` names the spinner; the per-fd interest
// rows ride `rux:spinwatch_fd`). The bespoke cmdline token `spinwatch=1`
// survives as a compatibility ALIAS that arms the same events at boot
// (existing hunt rigs keep working; the frozen debug= switch table is
// untouched — this module was never in it).

/// The watch gate: the unified-framework event switch (live — runtime
/// `set_event` arming takes effect immediately, which the old cached
/// tri-state could not do).
#[inline]
pub fn spinwatch_on() -> bool {
    crate::trace::EVENT_SPINWATCH.armed()
}
/// Compatibility arm (moved to the debug crate): same-named wrapper
/// for the boot arming site; dispatches the hook row.
pub fn spinwatch_compat_arm() {
    crate::api::debug::spinwatch_compat_arm();
}

/// The window count of the LAST report (set by `record` right before it
/// returns true; read by the report emitter for the trace record).
pub fn last_report_n() -> u32 {
    LAST_REPORT_N.load(Relaxed)
}

static LAST_REPORT_N: AtomicU32 = AtomicU32::new(0);


impl Watch {
    const fn new() -> Self {
        Watch {
            pid: AtomicU32::new(u32::MAX),
            n: AtomicU32::new(0),
            win: AtomicU64::new(0),
            last_rep: AtomicU64::new(0),
        }
    }
}

const SPINWATCH_SLOTS: usize = 512;
static WATCH: [Watch; SPINWATCH_SLOTS] = [const { Watch::new() }; SPINWATCH_SLOTS];
static ANY_REPORTED: AtomicBool = AtomicBool::new(false);

/// Threshold: zero-event returns per 200 ms window before the interest
/// set is dumped. A healthy waiter with a 10 ms slice re-check does at
/// most ~20; the FFRT spin face does 100+.
const N_THRESH: u32 = 80;
const WIN_JIFFIES: u64 = 20; // 200 ms at 100 Hz
const REP_GAP_JIFFIES: u64 = 500; // 5 s per task

/// Hook: one zero-event epoll_wait return (timeout expiry or empty
/// re-check slice). Returns true when the caller should dump its
/// interest set NOW (threshold crossed, per-task rate limit honored).
#[inline]
pub fn record() -> bool {
    if !spinwatch_on() {
        return false;
    }
    let pid = crate::process::hotpath::current_pid();
    let w = &WATCH[(pid as usize) & (SPINWATCH_SLOTS - 1)];
    if w.pid.load(Relaxed) != pid {
        w.pid.store(pid, Relaxed);
        w.n.store(0, Relaxed);
        w.win.store(0, Relaxed);
        w.last_rep.store(0, Relaxed);
    }
    let now = crate::timer::get_jiffies();
    let win = w.win.load(Relaxed);
    if win == 0 || now.saturating_sub(win) >= WIN_JIFFIES {
        w.win.store(now, Relaxed);
        w.n.store(1, Relaxed);
        return false;
    }
    let n = w.n.fetch_add(1, Relaxed) + 1;
    if n >= N_THRESH {
        let last = w.last_rep.load(Relaxed);
        if now.saturating_sub(last) >= REP_GAP_JIFFIES {
            w.last_rep.store(now, Relaxed);
            // Reset the window so the count needs to re-climb before the
            // next report (steady spinners re-report every 5 s).
            w.n.store(0, Relaxed);
            w.win.store(now, Relaxed);
            LAST_REPORT_N.store(n, Relaxed);
            if !ANY_REPORTED.swap(true, Relaxed) {
                crate::pr_warn!(
                    "SPINWATCH first report (threshold {} per {}ms)",
                    N_THRESH,
                    WIN_JIFFIES * 10
                );
            }
            return true;
        }
    }
    false
}


// ════════════════ leakorigin ══════════════════════════
// Leak-origin probe (rux:leakhunt (retired debug=leakhunt)) — names the acquire site of a leaked
// PREEMPT_OFFSET (and, since s14, of a stranded HARDIRQ_OFFSET — the
// pc=0x10000/0x10001 incumbent face of the fleet's four-faced wedge
// family; `hsite=` in every report names the trap bracket that booked
// it).
//
// The sc261-263 OH boots (and the fleet's riscv monkey lanes) show tasks
// carrying a leaked preempt_count of exactly 1 across park/wake cycles
// (PARKED-LEAK; multi-second WAKE-STRANDED incumbents with pc=1/2). The
// static audits found no unpaired add/sub pair, no underflow (no double
// drop), no discipline-bypassing switch (SWITCH-COUNT-LEAK stayed at 0
// all boot) and no 0→nonzero parked transition (PARKED-WILDWRITE never
// fired) — the producer is a runtime shape the existing probes can only
// see seconds after the fact.
//
// This instrument closes the gap from the other side: it records, for
// every task, the call site of the OUTERMOST PREEMPT_OFFSET acquire
// (the 0→1 transition — the one whose loss is the leak), and trips a
// report the moment a task crosses back into USERSPACE with a nonzero
// count. A user-mode return with preempt_count != 0 is a definitive
// leak: no in-flight IRQ can be booked on the task at that point, and
// every guard the syscall path took must have been dropped. The report
// carries the recorded acquire site (file:line of the guard
// construction) — the producer is named directly, not inferred.
//
// Costs when armed (rux:leakhunt (retired debug=leakhunt)): one side-table store per outermost
// guard acquire and one preempt_count load per user-mode trap return.
// OFF by default (the switch mirrors into ENABLED); the disabled-path
// cost is one relaxed load per outermost acquire.
//
// Print discipline: direct console (the trip can fire from IRQ-laden
// contexts), one report per leak episode per task (the latch re-arms on
// the next 0→1 acquire), total budget 400 lines per boot.

/// Mirror of the rux:leakhunt (retired debug=leakhunt) runtime switch (set from switches section (hotpath.rs)).

/// Total print budget for the boot (reports + parked-site lines).
static BUDGET: AtomicU32 = AtomicU32::new(400);

const LEAKORIGIN_SLOTS: usize = 2048;
const SLOT_MASK: usize = LEAKORIGIN_SLOTS - 1;


static LEAKORIGIN_TABLE: [LoSlot; LEAKORIGIN_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: LoSlot = LoSlot {
        task: AtomicUsize::new(0),
        site: AtomicUsize::new(0),
        jif: AtomicU64::new(0),
        cnt_after: AtomicI32::new(0),
        hsite: AtomicUsize::new(0),
        hjif: AtomicU64::new(0),
        ssite: AtomicUsize::new(0),
        sjif: AtomicU64::new(0),
        scpu: AtomicU32::new(0),
        smis: AtomicU32::new(0),
        reported: AtomicBool::new(false),
        otask: AtomicUsize::new(0),
        osite: AtomicUsize::new(0),
        ojif: AtomicU64::new(0),
        ocpu: AtomicU32::new(0),
        omis: AtomicU32::new(0),
        oslot: AtomicUsize::new(0),
        urrep_jif: AtomicU64::new(0),
    };
    [ZERO; LEAKORIGIN_SLOTS]
};

#[inline]
fn leakorigin_slot_of(task: *mut crate::process::types::Task) -> &'static LoSlot {
    let a = task as usize;
    // preempt-hunt: XOR-of-shifts mixes alias — for every shift set there
    // are address strides (observed live: 0x1000000, then 0x2080000)
    // whose contributions XOR to zero, making distinct task structs share
    // slots (bogus ACQUIRE-PAIR-SPLITs, shadowed records). Fibonacci
    // multiplication: no constant stride aliases.
    &LEAKORIGIN_TABLE[(a.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 53) & SLOT_MASK]
}

/// Record a PREEMPT_OFFSET acquire (last wins). Called from
/// `preempt_count_add` when the probe is armed.
#[inline]
pub fn note_acquire(
    task: *mut crate::process::types::Task,
    loc: &'static Location,
    cnt_after: i32,
) {
    let slot = leakorigin_slot_of(task);
    slot.task.store(task as usize, Ordering::Release);
    slot.site.store(loc as *const Location as usize, Ordering::Release);
    slot.jif
        .store(crate::timer::get_jiffies(), Ordering::Release);
    slot.cnt_after.store(cnt_after, Ordering::Release);
    slot.reported.store(false, Ordering::Release);
    // Outermost base acquire (base field 0->1): record the leak-origin
    // candidate. `cnt_after & 0xFF == 1` also matches base transitions
    // made under a live softirq field (0x100 -> 0x101).
    if (cnt_after & crate::interrupt::preempt::PREEMPT_MASK) == 1 {
        slot.otask.store(task as usize, Ordering::Release);
        slot.osite.store(loc as *const Location as usize, Ordering::Release);
        slot.ojif
            .store(crate::timer::get_jiffies(), Ordering::Release);
        let cpu = crate::arch::cpu_id() as u32;
        slot.ocpu.store(cpu, Ordering::Release);
        // tp-vs-slot divergence at acquire time: the sched slot names
        // another task than tp does. In the steady state these agree;
        // they diverge only inside the pick-publish -> __switch_to
        // window (IRQs off — no acquire can run there) and in the
        // stale-tp / aborted-switch families. A set flag on a leak line
        // is strong stale-booking evidence.
        let mis = if (cpu as usize) < crate::config::MAX_CPUS {
            // Volatile: the sched slot is written by its owning CPU under
            // the GRQ; this is a racy diagnostic read.
            let slot_task = unsafe {
                core::ptr::read_volatile(
                    &crate::sched::hotpath::cpu_state(cpu as usize).current
                        as *const *mut crate::process::types::Task
                ) as usize
            };
            if slot_task != task as usize {
                slot.oslot.store(slot_task, Ordering::Release);
                // ph5/ph5b verdict (2/2 boots wedged vs ph3's clean 1200s
                // with this print's absence as the only tree delta): the
                // LIVE print from inside the acquire path is wedged-boot
                // correlated — record-only now. The data still surfaces
                // at adjudication time (oslot= on the ORIGIN lines).
                report_acquire_divergent(task, loc, cpu, slot_task);
                1
            } else {
                0
            }
        } else {
            2
        };
        slot.omis.store(mis, Ordering::Release);
        // glib-mutex lane (2026-11-14): PHANTOM-BOOKING formation trap.
        // A base 0->1 landing on a task owned by NO cpu slot and NOT
        // mid-switch-out is phantom BY CONSTRUCTION — sc265 convicted
        // the invariant ("ZERO guards are ever booked on a task current
        // on no CPU"; the WAKE-HEAL block cites it for the wake side,
        // this is the acquire side of the same invariant). Guards live
        // on the executing task: the pre-park window's task is still in
        // its CPU's current slot, the mid-switch-out shape carries the
        // SWITCHING_OUT marker, and a picked task is slot-named from
        // its pick publish — all skipped by pick_owner_live, mirroring
        // the heal's adjudication. Covers both faces: the SLEEPING
        // parker booked post-freeze, and the RUNNING-but-unowned
        // freshly-woken/enqueued task booked inside the wake path
        // before the heal point (the state-only face missed bookings
        // that land after the wake flips state RUNNING). Dead targets
        // are skipped (teardown). This is the WAKE-HEAL producer caught
        // at the moment of booking instead of at the wake, naming the
        // acquire site directly. pid 0 (idle/static tasks) is skipped:
        // the AP-bringup window books printk/rcu guards on idle tasks
        // before their slots publish (boot25: 19 early lines, all
        // pid=0, budget gone before the runtime window). Budgeted
        // direct console (R34): the acquire path may run with IRQs on
        // and locks held.
        if !unsafe { (*task).state().is_dead() }
            && unsafe { (*task).pid() } != 0
            && !crate::sched::hotpath::pick_owner_live(task)
        {
            static PB_N: AtomicU32 = AtomicU32::new(120);
            let b = PB_N.load(Ordering::Acquire);
            if b != 0
                && PB_N
                    .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                use crate::console::uart_putc_raw as putchar;
                fn phex(v: usize) {
                    use crate::console::uart_putc_raw as putchar;
                    for sh in (0..16).rev() {
                        let n = ((v >> (sh * 4)) & 0xF) as u8;
                        putchar(if n < 10 {
                            b'0' + n
                        } else {
                            b'a' + n - 10
                        });
                    }
                }
                fn pdec(mut v: u64) {
                    use crate::console::uart_putc_raw as putchar;
                    let mut buf = [0u8; 12];
                    let mut n = 0;
                    if v == 0 {
                        putchar(b'0');
                        return;
                    }
                    while v > 0 && n < buf.len() {
                        buf[n] = b'0' + (v % 10) as u8;
                        v /= 10;
                        n += 1;
                    }
                    for i in (0..n).rev() {
                        putchar(buf[i]);
                    }
                }
                for &c in b"\nPHANTOM-BOOKING task=0x" {
                    putchar(c);
                }
                phex(task as usize);
                for &c in b" pid=" {
                    putchar(c);
                }
                pdec(unsafe { (*task).pid() } as u64);
                for &c in b" cpu=" {
                    putchar(c);
                }
                pdec(cpu as u64);
                for &c in b" state=0x" {
                    putchar(c);
                }
                phex(unsafe { (*task).state().bits() } as usize);
                // SAFETY: stored from a &'static Location above.
                let site = loc;
                for &c in b" site=" {
                    putchar(c);
                }
                for &c in site.file().as_bytes() {
                    putchar(c);
                }
                putchar(b':');
                pdec(site.line() as u64);
                for &c in b" mis=" {
                    putchar(c);
                }
                pdec(mis as u64);
                putchar(b'\n');
            }
        }
    }
}

/// Live report of a divergent outermost acquire (preempt-hunt). Record-
/// only by default since 14d04456: ph5/ph5b — the only boots carrying
/// this print — both wedged (UNIX_TABLE both-CPU deadlock at 18s/90s)
/// while ph3, without it, completed 1200s cleanly. CONFOUND NOTE (Face C
/// re-trial, 2026-11-02): the ph-session branch was built AFTER 192bfdce
/// (perf-sched merge 11-01 01:27) — every ph boot, wedged or clean, ran
/// with the x86_64 IRQ-exit kpreempt hook armed by default; the wedge-
/// fix lane's bisect (28b1afca) proved hook-ON causes exactly the
/// both-CPU DEADLOCK family. The print was never isolated from that
/// cause. Re-armed via the `rux:acqdiv (retired debug=acqdiv)` runtime switch for the A/B: the
/// print rides only on boots that ask for it, on a tree where kpreempt
/// defaults OFF.

#[allow(unsafe_code)]
fn report_acquire_divergent(
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

/// Record a HARDIRQ_OFFSET acquire (last wins; s14 extension — the
/// stranded-0x10000 face). `loc` is the trap dispatch site when
/// `irq_enter` is #[track_caller]. Kept separate from the PREEMPT
/// shadow so a 0x10001 leak names BOTH producers independently.
#[inline]
pub fn note_hardirq_acquire(
    task: *mut crate::process::types::Task,
    loc: &'static Location,
    cnt_after: i32,
) {
    let slot = leakorigin_slot_of(task);
    slot.task.store(task as usize, Ordering::Release);
    slot.hsite.store(loc as *const Location as usize, Ordering::Release);
    slot.hjif
        .store(crate::timer::get_jiffies(), Ordering::Release);
    slot.cnt_after.store(cnt_after, Ordering::Release);
    slot.reported.store(false, Ordering::Release);
}

/// Record a SOFTIRQ_OFFSET acquire (last wins; phantom-bits extension —
/// the stranded-0x100 monopolist face). `loc` is softirq.rs's
/// `__do_softirq` entry or seqlock.rs's `write` with #[track_caller]
/// propagation, so a stranded 0x100 names its booker directly. Also
/// stamps tp-vs-sched-slot divergence at booking time: a divergent
/// booking is the switch-window family (the release then lands on the
/// task tp names at ITS time, splitting the pair).
#[inline]
pub fn note_softirq_acquire(
    task: *mut crate::process::types::Task,
    loc: &'static Location,
    cnt_after: i32,
) {
    let slot = leakorigin_slot_of(task);
    slot.task.store(task as usize, Ordering::Release);
    slot.ssite.store(loc as *const Location as usize, Ordering::Release);
    slot.sjif
        .store(crate::timer::get_jiffies(), Ordering::Release);
    let cpu = crate::arch::cpu_id() as u32;
    slot.scpu.store(cpu, Ordering::Release);
    let mis = if (cpu as usize) < crate::config::MAX_CPUS {
        // Volatile racy diagnostic read (same discipline as note_acquire).
        let slot_task = unsafe {
            core::ptr::read_volatile(
                &crate::sched::hotpath::cpu_state(cpu as usize).current
                    as *const *mut crate::process::types::Task
            ) as usize
        };
        if slot_task != task as usize { 1 } else { 0 }
    } else {
        2
    };
    slot.smis.store(mis, Ordering::Release);
}

/// Softirq field drained to zero: the acquire paired — drop the shadow.
#[inline]
pub fn note_softirq_release(task: *mut crate::process::types::Task) {
    let slot = leakorigin_slot_of(task);
    slot.ssite.store(0, Ordering::Release);
}

/// Hardirq orphan twin (phantom-bits): print the recorded hsite for a
/// task observed carrying a hardirq bit at tick ENTRY (stranded from an
/// earlier bracket — the tick's own booking is not in that snapshot).
pub fn report_hardirq_origin(tag: &[u8], task: *mut crate::process::types::Task) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    let slot = leakorigin_slot_of(task);
    let site = slot.hsite.load(Ordering::Acquire) as *const Location;
    if site.is_null() {
        return;
    }
    static HBUDGET: AtomicU32 = AtomicU32::new(120);
    let b = HBUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if HBUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::uart_putc_raw as putchar;
    fn putd(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    for &c in b"\n" {
        putchar(c);
    }
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    putd(unsafe { (*task).pid() } as u64);
    // SAFETY: stored from a &'static Location (see note_hardirq_acquire).
    let loc = crate::debug::r#unsafe::location_ref(site);
    for &c in b" hsite=" {
        putchar(c);
    }
    for &c in loc.file().as_bytes() {
        putchar(c);
    }
    putchar(b':');
    putd(loc.line() as u64);
    for &c in b" hage_jif=" {
        putchar(c);
    }
    putd(
        crate::timer::get_jiffies()
            .wrapping_sub(slot.hjif.load(Ordering::Acquire)),
    );
    putchar(b'\n');
}

/// Print the SOFTIRQ acquire record for `task` (the phantom-bits tap):
/// names the site that booked a still-set 0x100. Called from the
/// tick-side orphan scan (preempt.rs) and the heal reports. Budgeted
/// separately so orphan lines cannot be starved.
pub fn report_softirq_origin(tag: &[u8], task: *mut crate::process::types::Task) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    let slot = leakorigin_slot_of(task);
    let site = slot.ssite.load(Ordering::Acquire) as *const Location;
    if site.is_null() {
        return; // armed late / wild write (no booking record)
    }
    static SBUDGET: AtomicU32 = AtomicU32::new(240);
    let b = SBUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if SBUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::uart_putc_raw as putchar;
    fn putd(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    for &c in b"\n" {
        putchar(c);
    }
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    putd(unsafe { (*task).pid() } as u64);
    for &c in b" pc=0x" {
        putchar(c);
    }
    let pc = unsafe { (*task).preempt_count() } as u32 as u64;
    for sh in (0..8).rev() {
        let n = ((pc >> (sh * 4)) & 0xF) as u8;
        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
    }
    // SAFETY: stored from a &'static Location (see note_softirq_acquire).
    let loc = crate::debug::r#unsafe::location_ref(site);
    for &c in b" ssite=" {
        putchar(c);
    }
    for &c in loc.file().as_bytes() {
        putchar(c);
    }
    putchar(b':');
    putd(loc.line() as u64);
    for &c in b" scpu=" {
        putchar(c);
    }
    putd(slot.scpu.load(Ordering::Acquire) as u64);
    if slot.smis.load(Ordering::Acquire) != 0 {
        for &c in b" smis=1" {
            putchar(c);
        }
    }
    for &c in b" sage_jif=" {
        putchar(c);
    }
    putd(
        crate::timer::get_jiffies()
            .wrapping_sub(slot.sjif.load(Ordering::Acquire)),
    );
    putchar(b'\n');
}

/// User-mode return tripwire. Call at the trap-handler tail when the
/// frame returns to userspace: preempt_count MUST be 0 there. Direct
/// console, latched per episode, budgeted.
///
/// preempt-hunt rate gate: a task carrying a stuck +1 re-acquires inner
/// guards constantly (every lock it touches re-arms the episode latch),
/// which flooded the shared budget within seconds (374 lines for one
/// carrier in the first 20s of ph1). Space reports 50 jiffies apart per
/// task and give the face its own budget.
pub fn check_user_return() {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    let Some(task) = crate::sched::hotpath::current() else {
        return;
    };
    // SAFETY: current() pins a live task; scalar field reads.
    let pc = unsafe { (*task).preempt_count() };
    if pc == 0 {
        return;
    }
    let slot = leakorigin_slot_of(task);
    if slot.task.load(Ordering::Acquire) != task as *mut _ as usize {
        return;
    }
    if slot.reported.swap(true, Ordering::AcqRel) {
        return;
    }
    let now = crate::timer::get_jiffies();
    let last = slot.urrep_jif.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 50 {
        return;
    }
    slot.urrep_jif.store(now, Ordering::Release);
    emit(b"\nLEAK-ORIGIN user-return", task, pc);
}

/// Append the recorded site for a PARKED-LEAK victim (called from the
/// sched.rs parkwalk printer, already rate-gated there).
///
/// v3: the walk classifies "parked" via the SCHED PER_CPU slots; a task
/// can be current on NO sched slot yet still EXECUTING (tp names it) —
/// the R55 stale-slot family would then read as a phantom parked-leak
/// (a legit live guard on a running task). The report now scans the
/// ARCH tp slots: `tpcpu=N` (plus the arch-slot count) means the victim
/// is actually running on N and the parkwalk's parked classification is
/// the artifact; no tpcpu = genuinely parked, and the +1 is real.
pub fn report_parked_site(task: *mut crate::process::types::Task) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    use crate::console::uart_putc_raw as putchar;
    fn pd(mut v: u64) {
        use crate::console::uart_putc_raw as putchar;
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            buf[0] = b'0';
            n = 1;
        } else {
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    // Scan the arch tp slots for the victim.
    let mut tpcpu: i32 = -1;
    let mut tppc: i32 = 0;
    for c in 0..crate::config::MAX_CPUS {
        let tp = crate::arch::cpu::get_thread_id_of(c) as *mut crate::process::types::Task;
        if tp == task {
            tpcpu = c as i32;
            // SAFETY: diagnostic field read.
            tppc = unsafe { (*tp).preempt_count() };
            break;
        }
    }
    const A: &[u8] = b"\nLEAK-ORIGIN parked-adjacent tp=";
    for &c in A {
        putchar(c);
    }
    pd(tpcpu as u64);
    if tpcpu >= 0 {
        const B: &[u8] = b" tpc_pc=";
        for &c in B {
            putchar(c);
        }
        pd(tppc as u64);
    }
    emit(b" LEAK-ORIGIN parked", task, unsafe { (*task).preempt_count() });
}

/// Append the recorded site for the tick-side PREEMPT-LEAK incumbent
/// (called from the scheduler_tick tripwire's print block).
pub fn report_incumbent_site(task: *mut crate::process::types::Task) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    emit(b"\nLEAK-ORIGIN running", task, unsafe { (*task).preempt_count() });
}

/// Print the OUTERMOST-acquire record for `task` (the preempt-hunt tap):
/// names the site whose 1->0 release never ran. Called from the
/// WAKE-HEAL zeroing (sched.rs, GRQ held — direct console only) and the
/// underflow clamp report: the two points where a nonzero base count is
/// adjudicated. Budgeted separately from BUDGET (the origin line is the
/// hunt's primary output; it must not be starved by parked-adjacent
/// noise).
pub fn report_outermost(tag: &[u8], task: *mut crate::process::types::Task) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    let slot = leakorigin_slot_of(task);
    if slot.otask.load(Ordering::Acquire) != task as usize {
        return; // no outermost record for this task (armed late / healed)
    }
    static OBUDGET: AtomicU32 = AtomicU32::new(200);
    let b = OBUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if OBUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::uart_putc_raw as putchar;
    fn putd(mut v: u64) {
        use crate::console::uart_putc_raw as putchar;
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    for &c in b"\n" {
        putchar(c);
    }
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    putd(unsafe { (*task).pid() } as u64);
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
        putd(loc.line() as u64);
    } else {
        for &c in b" osite=?" {
            putchar(c);
        }
    }
    for &c in b" ocpu=" {
        putchar(c);
    }
    putd(slot.ocpu.load(Ordering::Acquire) as u64);
    let mis = slot.omis.load(Ordering::Acquire);
    if mis != 0 {
        for &c in b" omis=" {
            putchar(c);
        }
        putd(mis as u64);
        let osl = slot.oslot.load(Ordering::Acquire);
        if osl != 0 {
            for &c in b" oslot=0x" {
                putchar(c);
            }
            for sh in (0..16).rev() {
                let n = ((osl >> (sh * 4)) & 0xF) as u8;
                putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
            }
        }
    }
    for &c in b" oage_jif=" {
        putchar(c);
    }
    putd(
        crate::timer::get_jiffies()
            .wrapping_sub(slot.ojif.load(Ordering::Acquire)),
    );
    putchar(b'\n');
}

/// One report line: tag, pid, pc, site=<file>:<line> cnt_after=<n>
/// age=<jiffies> for the recorded acquire.
fn emit(tag: &[u8], task: *mut crate::process::types::Task, pc: i32) {
    use crate::console::uart_putc_raw as putchar;
    let b = BUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    BUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .ok();
    fn put_i64(mut v: i64) {
        use crate::console::uart_putc_raw as putchar;
        if v < 0 {
            putchar(b'-');
            v = v.wrapping_neg();
        }
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            buf[0] = b'0';
            n = 1;
        } else {
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    let slot = leakorigin_slot_of(task);
    let now = crate::timer::get_jiffies();
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    put_i64(unsafe { (*task).pid() } as i64);
    for &c in b" pc=" {
        putchar(c);
    }
    put_i64(pc as i64);
    let site = slot.site.load(Ordering::Acquire) as *const Location;
    if slot.task.load(Ordering::Acquire) == task as usize && !site.is_null() {
        // SAFETY: stored from a &'static Location (see site_of).
        let loc = crate::debug::r#unsafe::location_ref(site);
        for &c in b" site=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        put_i64(loc.line() as i64);
    } else {
        for &c in b" site=?" {
            putchar(c);
        }
    }
    // s14: the HARDIRQ acquire shadow — names which trap bracket
    // booked the stranded 0x10000 bit on this task, and when.
    let hsite = slot.hsite.load(Ordering::Acquire) as *const Location;
    if slot.task.load(Ordering::Acquire) == task as usize && !hsite.is_null() {
        // SAFETY: stored from a &'static Location (see note_hardirq_acquire).
        let loc = crate::debug::r#unsafe::location_ref(hsite);
        for &c in b" hsite=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        put_i64(loc.line() as i64);
        for &c in b" hage_jif=" {
            putchar(c);
        }
        put_i64(now.wrapping_sub(slot.hjif.load(Ordering::Acquire)) as i64);
    }
    // phantom-bits: the SOFTIRQ acquire shadow — names which +0x100
    // booker (__do_softirq bracket vs SeqLock::write) owns a stranded
    // softirq bit on this task, and when.
    let ssite = slot.ssite.load(Ordering::Acquire) as *const Location;
    if slot.task.load(Ordering::Acquire) == task as usize && !ssite.is_null() {
        // SAFETY: stored from a &'static Location (see note_softirq_acquire).
        let loc = crate::debug::r#unsafe::location_ref(ssite);
        for &c in b" ssite=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        put_i64(loc.line() as i64);
        for &c in b" sage_jif=" {
            putchar(c);
        }
        put_i64(now.wrapping_sub(slot.sjif.load(Ordering::Acquire)) as i64);
    }
    for &c in b" cnt_after=" {
        putchar(c);
    }
    put_i64(slot.cnt_after.load(Ordering::Acquire) as i64);
    for &c in b" age_jif=" {
        putchar(c);
    }
    put_i64(now.wrapping_sub(slot.jif.load(Ordering::Acquire)) as i64);
    // Outermost-acquire tap: when the reported task is the one holding
    // the unreturned 0->1, its site is the leak's origin (stable across
    // inner 1->2->1 pairs; see LoSlot.otask).
    if slot.otask.load(Ordering::Acquire) == task as usize {
        let osite = slot.osite.load(Ordering::Acquire) as *const Location;
        if !osite.is_null() {
            // SAFETY: stored from a &'static Location (see note_acquire).
            let loc = crate::debug::r#unsafe::location_ref(osite);
            for &c in b" osite=" {
                putchar(c);
            }
            for &c in loc.file().as_bytes() {
                putchar(c);
            }
            putchar(b':');
            put_i64(loc.line() as i64);
        }
        for &c in b" ocpu=" {
            putchar(c);
        }
        put_i64(slot.ocpu.load(Ordering::Acquire) as i64);
        if slot.omis.load(Ordering::Acquire) != 0 {
            for &c in b" omis=" {
                putchar(c);
            }
            put_i64(slot.omis.load(Ordering::Acquire) as i64);
            let osl = slot.oslot.load(Ordering::Acquire);
            if osl != 0 {
                for &c in b" oslot=0x" {
                    putchar(c);
                }
                for sh in (0..16).rev() {
                    let n = ((osl >> (sh * 4)) & 0xF) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
            }
        }
    }
    putchar(b'\n');
}


// ════════════════ pathstash ═══════════════════════════
// Per-task last-open path stash for TXN-HANG (rux:txnhunt (retired debug=txnhunt)).
//
// Leg-28 item 2: sc303's TXN-HANG showed storage_daemon parked
// UNINTERRUPTIBLE in open() for 5.5s→26s under the code-37 handler,
// but the entry-frame report only prints a0/a1 — the raw user
// pathname pointer, useless from the serial log. The audit runs in
// timer context against ANOTHER task's address space, so it cannot
// copy the string at print time; instead sys_openat stashes the
// resolved full path here at entry (in the task's own context, after
// resolve_user_path succeeded — the D-state stall happens later,
// inside the open work itself, so the stash holds exactly the path
// the stuck open is working on).
//
// Contract: zero cost when rux:txnhunt (retired debug=txnhunt) is off (one switch check);
// no locks — 8 slots indexed by tid % 8, tid word acts as the
// publication flag; diagnostics only, torn reads can mix bytes of
// two paths of tasks colliding on a slot, which is cosmetic (bounded
// print, utf8-checked).

const N: usize = 8;


static mut PATHSTASH_SLOTS: [Slot; N] = [const {
    Slot {
        tid: AtomicU32::new(0),
        op: AtomicU32::new(0),
        len: AtomicU32::new(0),
        buf: [0; CAP],
    }
}; N];

/// Stash op tags (leg-29A): the syscall family that recorded the path.
pub const OP_OPEN: u32 = 1;
pub const OP_WRITE: u32 = 2;
pub const OP_SYNC: u32 = 3;
/// leg-36B: truncate family. sc333B's foundation-main wedge sat in
/// ftruncate(fd, len) 80+ s while holding a server txn; the serial only
/// showed the raw fd. Both truncate entries stash their resolved path
/// so TXN-HANG can name the file.
pub const OP_TRUNC: u32 = 4;

/// Record the current task's last resolved open path. Called from
/// sys_openat after resolution; arm-gated.
pub fn stash_open(path: &str) {
    stash_file(OP_OPEN, path);
}

/// Record the current task's last file path with the syscall-family
/// tag (open / regular-file write / fsync-fdatasync). Each family
/// re-stashes at ITS entry, so a thread parked inside one of them
/// always shows its own file. Arm-gated on TxnHang (leg-29A: sc310's
/// hiview handler sat in fdatasync 43 min with only a raw fd known).
pub fn stash_file(op: u32, path: &str) {
    if !crate::trace::EVENT_TXNHANG.armed() {
        return;
    }
    // current_pid() is the task (thread) id — the same id TXN-HANG
    // reports as the server tid.
    let tid = crate::process::hotpath::current_pid();
    if tid == 0 {
        return;
    }
    let bytes = path.as_bytes();
    let n = bytes.len().min(CAP);
    // SAFETY: diagnostic write into a static slot; this task is the
    // only writer for its tid slot, publication ordered via tid/len.
    let s = unsafe { &mut *core::ptr::addr_of_mut!(PATHSTASH_SLOTS[(tid as usize) % N]) };
    s.tid.store(0, Ordering::Release);
    s.buf[..n].copy_from_slice(&bytes[..n]);
    s.op.store(op, Ordering::Release);
    s.len.store(n as u32, Ordering::Release);
    s.tid.store(tid, Ordering::Release);
}

/// Lazy-path variant for call sites that would have to CONSTRUCT the
/// path per call: `stash_file(OP_WRITE, &file.path())` evaluates
/// `File::path()` (dentry walk + String formatting) on every write(2)
/// BEFORE stash_file's switch check discards it — a probe cost paid on
/// the serial-clean family with the switch off (p5c decomposition,
/// leg 44). The switch check runs BEFORE `f`; with TxnHang off this is
/// one atomic load and no allocation.
pub fn stash_file_lazy(op: u32, f: impl FnOnce() -> alloc::string::String) {
    if !crate::trace::EVENT_TXNHANG.armed() {
        return;
    }
    let path = f();
    stash_file(op, &path);
}

/// Look up the last (op, path) stashed by `tid`. Returns None when the
/// slot is empty, owned by another tid, or the path was never written.
pub fn lookup_file(tid: u32) -> Option<(u32, &'static str)> {
    let i = (tid as usize) % N;
    // SAFETY: diagnostic read of the static slot table; the 'static
    // borrow is derived from the static itself and bytes are
    // utf8-checked below.
    let s: &'static Slot = unsafe { &*core::ptr::addr_of!(PATHSTASH_SLOTS).cast::<Slot>().add(i) };
    if s.tid.load(Ordering::Acquire) != tid {
        return None;
    }
    let n = s.len.load(Ordering::Acquire) as usize;
    if n == 0 || n > CAP {
        return None;
    }
    let op = s.op.load(Ordering::Acquire);
    let p = core::str::from_utf8(&s.buf[..n]).ok()?;
    Some((op, p))
}

/// Backward-compatible open-only lookup (leg-28 print).
pub fn lookup_open(tid: u32) -> Option<&'static str> {
    lookup_file(tid).map(|(_, p)| p)
}


// ════════════════ slowtask ════════════════════════════
// Slow-kernel-entry audit (leg-33B, `rux:slowtask (retired debug=slowtask)`).
//
// sc329B adjudication gap: the TXN-HANG lines name the SERVER thread of
// a hung transaction and its LAST kernel entry (sys/rip), but nothing
// attributes tasks that stall inside NON-binder syscalls — and the
// foundation reader-exhaustion cascade had its leaves exactly there
// (hiview OS_IPC_1_680 in pwrite64 for 30+ s, powermgr in an
// uninterruptible open, foundation tid 808 in ftruncate). Every layer
// above those leaves (futex lock waits, BINDER_WRITE_READ reply waits,
// TXN-HANG growth, watchdog SIGABRT) is downstream.
//
// Contract: called from the 1 Hz timer-softirq heartbeat slot (same
// discipline as txn_hang_audit — the heartbeat compare_exchange admits
// exactly one CPU). Every 4th call (~4 s), walk all tasks once and
// snapshot (pid, syscall-nr, entry-epc, state) of every SLEEPING task
// that has entered the kernel. A tuple that persists across two rounds
// (>= 8 s on the same kernel entry) is STUCK; fs-family syscalls print
// a full SLOWTASK line with the parked kernel-stack chain; lock-family
// waits (futex/epoll/poll) collapse into a one-line summary unless one
// of them is the single oldest entry of the round. Stack words come
// from the same diagnostic windows as TXN-HANG-STK (sleep_snapshot
// contract, sc261); symbolize offline with addr2line.
//
// All state lives in this module's static table (single-auditor,
// no locks needed); zero cost when the switch is off.

const TABLE_N: usize = 192;


// Round table under a spinlock: the heartbeat CAS admits ONE auditor,
// so the lock is uncontended in the healthy case — it exists so the
// slot array's exclusivity is type-checked, not review-enforced.
static SLOWTASK_SLOTS: crate::sync::types::Spinlock<[SlowtaskSlot; TABLE_N]> =
    crate::sync::types::Spinlock::new([const {
        SlowtaskSlot {
            pid: 0,
            sys: 0,
            epc: 0,
            state: 0,
            first_j: 0,
            last_j: 0,
            printed_j: 0,
        }
    }; TABLE_N]);

/// Round counter (incremented per audit fire; a fire is every ~4 s).
static ROUND: AtomicU32 = AtomicU32::new(0);
/// Whether any SLOTS cell was ever written (skip the clear sweep until
/// then).
static TABLE_USED: AtomicBool = AtomicBool::new(false);
/// Total SLOWTASK lines emitted this boot (hard budget).
static LINES: AtomicU64 = AtomicU64::new(0);

/// fs-family syscall numbers (arch tables differ).
#[cfg(feature = "x86_64")]
const FS_FAMILY: &[u64] = &[
    0, 1, 2, 3, // read write open close
    4, 5, 6, // stat fstat lstat
    16, // ioctl (BINDER_WRITE_READ sits here — binder reads matter too)
    17, 18, 19, 20, // pread64 pwrite64 readv writev
    76, 77, // truncate ftruncate
    74, 75, // fsync fdatasync
    32, 33, // dup dup2 (fd table walks)
    87, 88, // unlink rename (x86_64 numbers)
    83, 84, 85, 86, // mkdir rmdir creat link
    257, // openat
    322, // sync_file_range
];
#[cfg(any(feature = "riscv64", feature = "aarch64"))]
const FS_FAMILY: &[u64] = &[
    63, 64, // read write
    43, 57, // close dup3 (generic)
    49, 50, // (spare)
    29, 30, // ioctl (generic 29)
    67, 68, // pread64 pwrite64 (generic 67/68)
    65, 66, // readv writev (generic 65/66)
    45, 46, // truncate ftruncate (generic)
    82, 83, // fsync fdatasync (generic)
    34, 35, // mkdirat unlinkat (generic)
    38, 39, // renameat renameat2 (generic)
    56, // openat (generic)
    62, // lseek
];

fn fs_family(sys: u64) -> bool {
    FS_FAMILY.contains(&sys)
}

/// Audit entry point (timer heartbeat, ~1 Hz caller; fires every 4th).
pub fn slowtask_tick() {
    if !crate::trace::EVENT_SLOWTASK.armed() {
        return;
    }
    let round = ROUND.fetch_add(1, Ordering::Relaxed);
    if round % 4 != 0 {
        return;
    }
    let now = crate::timer::get_jiffies() as u64;
    // SAFETY: single-auditor (heartbeat CAS admits one CPU; the table
    // is only touched from this path).
    unsafe { audit(now) };
}

/// Collect this round's stuck entries (>= 8 s old), print them.
///
/// Called only from `tick` (single auditor, heartbeat CAS).
fn audit(now: u64) {
    // Age out stale slots (task exited / syscall changed): anything not
    // refreshed this round dies.
    if TABLE_USED.load(Ordering::Relaxed) {
        for s in SLOWTASK_SLOTS.lock_irqsave().iter_mut() {
            if s.pid != 0 && now.saturating_sub(s.last_j) > 20 * 100 {
                s.pid = 0;
            }
        }
    }

    // Round walk: match or insert.
    struct Note {
        pid: u32,
        sys: u64,
        epc: u64,
        state: u32,
        a0: u64,
        a1: u64,
        age_j: u64,
    }
    let mut notes: alloc::vec::Vec<Note> = alloc::vec::Vec::new();

    // lb325 wedge: this audit runs at the timer-softirq heartbeat (IF=0,
    // non-preemptible). The BLOCKING task walk deadlocks when the snapshot
    // reaches a bucket whose lock an interrupted task-context writer still
    // holds (self-deadlock by interrupt). Try-walk: buckets held at
    // snapshot time are skipped; a tuple missed one round simply
    // re-baselines next round (the >=8 s persistence filter is unaffected).
    let skipped = crate::sched::hotpath::for_each_task_try(|task| {
        // The walk hands &Task under the bucket lock — plain reads.
        let tk = task;
        let st_bits = tk.state().bits();
        // Lost-park all-task census (rux:lostpark (retired debug=lostpark)): one unfiltered row
        // per task every audit round while wedged-hunting — answers
        // "which bookkeeping class does the vanished task rest in"
        // (runqueue, slots, wakeable state, pid-hash presence).
        if crate::trace::EVENT_LOSTPARK.armed() && tk.pid() != 0
            && tk.pid() != u32::MAX
        {
            static LPALL_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if LPALL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                < 24
            {
                let claim = tk.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
                crate::pr_warn!(
                    "LP-CENSUS pid={} state={:#x} pc={:#x} oncpu={} pk={} cl={} onrq={} j={}",
                    tk.pid(),
                    st_bits,
                    tk.preempt_count() as u32 as u64,
                    tk.on_cpu() as u64,
                    tk.pick_state() as u64,
                    claim as i64,
                    tk.sched_entity().is_on_rq() as u64,
                    now
                );
                // The parked chain names the exact park site (the
                // unregistered-park signature hunt).
                print_parked_chain(tk.pid(), 0);
            }
        }
        let sleeping = st_bits & 0x3 != 0; // INTERRUPTIBLE | UNINTERRUPTIBLE
        if !sleeping {
            // Lost-park census (fix-lostpark, rux:lostpark (retired debug=lostpark)): a RUNNING
            // task owned by no CPU slot is the R46 phantom family — on
            // x86_64 the idle-loop harvester is riscv64-gated, so nothing
            // else ever names or heals it. Report it with the same
            // aging/chain discipline so the wedge capture names the
            // vanished poster.
            if crate::trace::EVENT_LOSTPARK.armed() && tk.pid() != 0
                && tk.pid() != u32::MAX
            {
                let owned = (0..crate::config::MAX_CPUS)
                    .any(|c| crate::sched::hotpath::cpu_state(c).current as usize == task as *const _ as usize);
                if !owned {
                    notes.push(Note {
                        pid: tk.pid(),
                        sys: 0xFFFF_FFFF_0000_0001, // marker: non-sleeping, slotless
                        epc: 0,
                        state: st_bits,
                        a0: tk.preempt_count() as u64,
                        a1: (tk.on_cpu() as u64) | ((owned as u64) << 8),
                        age_j: 0,
                    });
                    print_parked_chain(tk.pid(), 0);
                }
            }
            return;
        }
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        let _ = top;
        // Entry-frame read through the typed shared view.
        let Some(r) = tk.pt_regs_ref() else { return };
        #[cfg(feature = "x86_64")]
        let (sys, epc, a0, a1) = (r.orig_rax as u64, r.rip as u64, r.rdi, r.rsi);
        #[cfg(feature = "riscv64")]
        let (sys, epc, a0, a1) = (r.a7 as u64, r.epc as u64, r.a0 as u64, r.a1 as u64);
        #[cfg(feature = "aarch64")]
        let (sys, epc, a0, a1) = (r.syscallno as u64, r.pc as u64, r.regs[0] as u64, r.regs[1] as u64);
        if sys == 0 {
            // Not a syscall entry (interrupt frame) — skip, except for
            // the lost-park census (rux:lostpark (retired debug=lostpark)): a USER task parked
            // under an interrupt frame never re-enters its wait loop's
            // recheck and no wake path names it — the vanished-task
            // signature of the ping-pong wedge. Name it once per audit
            // round with its parked chain.
            if crate::trace::EVENT_LOSTPARK.armed() && tk.pid() != 0
                && tk.pid() != u32::MAX
            {
                notes.push(Note {
                    pid: tk.pid(),
                    sys: 0xFFFF_FFFF_0000_0002, // marker: sleeping, IRQ frame
                    epc: 0,
                    state: st_bits,
                    a0: tk.preempt_count() as u64,
                    a1: tk.on_cpu() as u64,
                    age_j: 0,
                });
                print_parked_chain(tk.pid(), 0);
            }
            return;
        }
        let pid = tk.pid();
        // Single-auditor table (heartbeat CAS admits one CPU); the
        // spinlock makes the exclusivity type-checked. The lock is
        // dropped before any printing.
        let (i, matched, first_j) = {
            let mut table = SLOWTASK_SLOTS.lock_irqsave();
            let mut idx: Option<usize> = None;
            for (i, s) in table.iter().enumerate() {
                if s.pid == pid {
                    idx = Some(i);
                    break;
                }
            }
            match idx {
                Some(i) => {
                    let slot = &mut table[i];
                    let matched = slot.sys == sys && slot.epc == epc;
                    if matched {
                        slot.last_j = now;
                        slot.state = st_bits;
                    } else {
                        // Entry changed since last round — restart aging.
                        slot.sys = sys;
                        slot.epc = epc;
                        slot.state = st_bits;
                        slot.first_j = now;
                        slot.last_j = now;
                    }
                    (i, matched, slot.first_j)
                }
                None => {
                    let Some(i) = table.iter().position(|s| s.pid == 0) else {
                        return;
                    };
                    let s = &mut table[i];
                    s.pid = pid;
                    s.sys = sys;
                    s.epc = epc;
                    s.state = st_bits;
                    s.first_j = now;
                    s.last_j = now;
                    s.printed_j = 0;
                    TABLE_USED.store(true, Ordering::Relaxed);
                    (i, true, s.first_j)
                }
            }
        };
        let age_j = now.saturating_sub(first_j);
        // Two-round persistence (>= 8 s) and a 32 s reprint cadence.
        let printed_j = SLOWTASK_SLOTS.lock_irqsave()[i].printed_j;
        if age_j >= 8 * 100 && now.saturating_sub(printed_j) >= 32 * 100 {
            notes.push(Note {
                pid,
                sys,
                epc,
                state: st_bits,
                a0,
                a1,
                age_j,
            });
            SLOWTASK_SLOTS.lock_irqsave()[i].printed_j = now;
        }
    });
    if skipped > 0 {
        // Partial snapshot (a bucket was mid-write): never fatal, but the
        // round's persistence filter saw fewer tasks — say so once.
        static SKIP_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        if SKIP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            crate::pr_warn!(
                "SLOWTASK: round snapshot partial — {} pid-hash bucket(s) held, skipped",
                skipped
            );
        }
    }

    if notes.is_empty() {
        return;
    }
    // fs-family first, then by age.
    notes.sort_by(|a, b| {
        let fa = fs_family(a.sys);
        let fb = fs_family(b.sys);
        fb.cmp(&fa).then(b.age_j.cmp(&a.age_j))
    });
    // Identity of the round's single oldest entry (a lock-family wait
    // earns one full line only when it is THE oldest — its stack usually
    // names the lock holder's wait site).
    let oldest_pid = notes[0].pid;
    let oldest_sys = notes[0].sys;

    let mut lock_lines = 0usize;
    for n in notes.iter().take(20) {
        let fs = fs_family(n.sys);
        // Lock-family waits: only the single oldest entry gets a full
        // line; the rest are counted for the summary. Lost-park census
        // markers (high sentinel word) always print — they are one-shot
        // wedge captures, not recurring lock waits.
        let is_lp_marker = n.sys >> 32 == 0xFFFF_FFFF;
        if !fs && !(n.pid == oldest_pid && n.sys == oldest_sys) && !is_lp_marker {
            lock_lines += 1;
            continue;
        }
        if LINES.load(Ordering::Relaxed) > 400 {
            break;
        }
        LINES.fetch_add(1, Ordering::Relaxed);
        // comm into a fixed buffer (no String allocation in the audit
        // path; pr_warn! takes the &str).
        let mut comm_buf = [0u8; 16];
        let comm_len;
        {
            if let Some(t) = crate::process::types::TaskBorrow::lookup(n.pid) {
                let c = t.comm();
                let n = c.len().min(comm_buf.len());
                comm_buf[..n].copy_from_slice(&c[..n]);
                comm_len = comm_buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
            } else {
                comm_len = 0;
            }
        }
        let comm = core::str::from_utf8(&comm_buf[..comm_len]).unwrap_or("?");
        crate::trace::trace_slowtask(0, n.pid, n.sys, n.epc);
        crate::pr_warn!(
            "SLOWTASK pid={} comm='{}' state={:#x} sys={} epc={:#x} a0={:#x} a1={:#x} age={}s",
            n.pid,
            comm,
            n.state,
            n.sys,
            n.epc,
            n.a0,
            n.a1,
            n.age_j / 100
        );
        // Parked-chain words (sp-up window, deepened leg-33B): 12 words
        // per line, two lines, kernel-text addresses only.
        print_parked_chain(n.pid, n.epc);
    }
    if lock_lines > 0 {
        crate::trace::trace_slowtask(1, 0, lock_lines as u64, now);
        crate::pr_warn!("SLOWTASK-SUMMARY lockfamily={} round_j={}", lock_lines, now);
    }
}

/// Print up to 24 parked-chain kernel-text words from the task's saved
/// sp upward (the wait chain; symbolize offline).
fn print_parked_chain(pid: u32, epc: u64) {
    // TaskBorrow's short-read contract (diagnostic, no yield below).
    let tk = match crate::process::types::TaskBorrow::lookup(pid) {
        Some(t) => t,
        None => return,
    };
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    extern "C" {
        static __stext: u8;
        static __etext: u8;
    }
    #[cfg(not(feature = "x86_64"))]
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    #[cfg(feature = "x86_64")]
    let (lo, hi) = (
        core::ptr::addr_of!(__stext) as u64,
        core::ptr::addr_of!(__etext) as u64,
    );
    #[cfg(not(feature = "x86_64"))]
    let (lo, hi) = (
        core::ptr::addr_of!(_stext) as u64,
        core::ptr::addr_of!(_etext) as u64,
    );
    let is_text = |v: u64| v >= lo && v < hi && (v & 1) == 0 && v != epc;
    let base = top.wrapping_sub(crate::config::KERNEL_STACK_SIZE) as u64;
    let sp = tk.thread().sp as u64;
    if sp < base || sp + 8 > top as u64 || (sp & 7) != 0 {
        return;
    }
    // Upward walk of the task's own stack through the bounds-checked
    // window (INV-WINDOW-*).
    let win = crate::safety::frame::KernelStackWindow::new(
        top - crate::config::KERNEL_STACK_SIZE,
        top,
    );
    let mut words: [u64; 24] = [0; 24];
    let mut n = 0usize;
    for k in 0..640usize {
        let v = match win.and_then(|w| w.read_u64((sp as usize) + k * 8)) {
            Some(v) => v,
            None => break,
        };
        if is_text(v) {
            words[n] = v;
            n += 1;
            if n == words.len() {
                break;
            }
        }
    }
    if n == 0 {
        return;
    }
    let mut line = alloc::string::String::new();
    use core::fmt::Write as _;
    let _ = write!(line, "SLOWTASK-STK pid={}", pid);
    for (i, w) in words.iter().take(n).enumerate() {
        if i == 12 {
            crate::pr_warn!("{}", line);
            line.clear();
            let _ = write!(line, "SLOWTASK-STK2 pid={}", pid);
        }
        let _ = write!(line, " {w:#x}");
    }
    crate::pr_warn!("{}", line);
}


// ════════════════ forkprof ════════════════════════════
// fork/exec/exit phase profiler — attribution probe for the
// process-lifecycle perf lane (`feature/perf-fork`).
//
// Armed by the kernel cmdline `forkprof=1` (own key, syscost-style;
// no switches section (hotpath.rs) bit is consumed). Costs nothing when off: every
// hook is one relaxed load of the cached armed word, and the phase
// adds are plain fetch_adds into static arrays.
//
// What it measures and why: lmbench lat_proc,fork sits ~20x over the
// reference column while a small-mm fork+exit accounts for nearly all
// of it (forkbench fb_mm). The phase timers decompose the two bodies:
//
// - do_clone (plain-fork arm, CLONE_VM==0): task-slot alloc,
//   copy_thread, fdtable deep copy, the whole mm fork (and inside it:
//   VMA snapshot, COW page-table walk, VMA replay), enqueue.
// - do_exit (leader arm): mm drop (and inside MmStruct::drop: VMA
//   release, synchronize_rcu grace wait, page-table free), orphan
//   reparent, exit-notify defer.
//
// Dump: `report()` prints one line per phase (count, avg, max in us)
// and is called from the shutdown cascade — every probe rig ends with
// a reboot(2), so the numbers ride the serial log out.

/// Phase slots (see `report()` for the names).
pub const P_FK_TOTAL: usize = 0;
pub const P_FK_ALLOC: usize = 1;
pub const P_FK_COPYTHREAD: usize = 2;
pub const P_FK_FDTABLE: usize = 3;
pub const P_FK_MMFORK: usize = 4;
pub const P_FK_ENQUEUE: usize = 5;
pub const P_MM_SNAPSHOT: usize = 6;
pub const P_MM_COW: usize = 7;
pub const P_MM_REPLAY: usize = 8;
pub const P_EX_TOTAL: usize = 9;
pub const P_EX_MMDROP: usize = 10;
pub const P_DROP_VMAFREE: usize = 11;
pub const P_DROP_RCU: usize = 12;
pub const P_DROP_PTFREE: usize = 13;
pub const P_EX_REPARENT: usize = 14;
pub const P_EX_NOTIFY: usize = 15;
pub const P_EXEC_TOTAL: usize = 16;
/// Scalar-only slots below: `bump()`ed counts (VMAs per fork, PTEs
/// copied, leaf tables allocated) — reported raw, never `add()`ed.
pub const S_FORKS_VMASES: usize = 17;
pub const S_COW_PTES: usize = 18;
pub const S_COW_TABLES: usize = 19;
pub const S_MM_DROPS: usize = 20;
/// wait4 whole body (includes the blocking sleep — the wall the
/// process-lifecycle loop pays waiting for the child side).
pub const P_WT_BODY: usize = 21;
/// The schedule() inside do_wait's sleep loop — returns only after the
/// child ran, exited and woke us: this is the fork->exit handoff.
pub const P_WT_SCHED: usize = 22;
/// release_task — the reaping parent frees the child's Task.
pub const P_WT_REAP: usize = 23;
/// Post-context-switch deferred exit-notify processing (SIGCHLD +
/// wait_chldexit group wake) — runs in the switcher's context.
pub const P_SCHED_NOTIFY: usize = 24;
/// Task::wake_up_impl body — the waker-side cost (select_cpu + locked
/// enqueue + resched/IPI dispatch).
pub const P_WAKE_BODY: usize = 25;
/// __schedule's pick section (queue pick + heal machinery).
pub const P_PICK: usize = 26;
/// context_switch proper (save/restore + switch asm tail).
pub const P_CTXSW: usize = 27;
/// wake stamp -> first context-switch-IN delta (the wake-to-run latency
/// the process-lifecycle loop pays).
pub const P_WAKE_RUN: usize = 28;
/// fork-enqueue stamp -> the child's first context-switch-IN (the new
/// task's queue latency; separate slot from real wakes).
pub const P_CHILD_RUN: usize = 29;
pub const P_COUNT: usize = 30;

/// Per-pid wake stamps (cycle domain) for the wake->run delta. Keyed by
/// pid & mask; a stamp is consumed (zeroed) by the first switch-in.
const WAKE_STAMP_PIDS: usize = 1024;
const ZERO_U64: AtomicU64 = AtomicU64::new(0);
static WAKE_STAMPS: [AtomicU64; WAKE_STAMP_PIDS] = [ZERO_U64; WAKE_STAMP_PIDS];
static CHILD_STAMPS: [AtomicU64; WAKE_STAMP_PIDS] = [ZERO_U64; WAKE_STAMP_PIDS];

/// Record the wake instant for `pid` (called on the wake's success
/// path, still inside the waker's context).
#[inline]
pub fn stamp_wake(pid: u32) {
    WAKE_STAMPS[(pid as usize) & (WAKE_STAMP_PIDS - 1)].store(forkprof_now(), Ordering::Relaxed);
}

/// Record the enqueue instant of a freshly forked child (consumed at the
/// child's first switch-in — the new-task queue latency).
#[inline]
pub fn stamp_child(pid: u32) {
    CHILD_STAMPS[(pid as usize) & (WAKE_STAMP_PIDS - 1)].store(forkprof_now(), Ordering::Relaxed);
}

/// Consume any pending stamp for `pid` and, if one was found, account
/// the stamp->switch-in delta (called at context-switch-in of `pid`).
#[inline]
pub fn consume_wake(pid: u32) {
    let idx = (pid as usize) & (WAKE_STAMP_PIDS - 1);
    let ct = CHILD_STAMPS[idx].load(Ordering::Relaxed);
    if ct != 0
        && CHILD_STAMPS[idx]
            .compare_exchange(ct, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        add(P_CHILD_RUN, ct);
        return;
    }
    let wt = WAKE_STAMPS[idx].load(Ordering::Relaxed);
    if wt == 0 {
        return;
    }
    if WAKE_STAMPS[idx]
        .compare_exchange(wt, 0, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        add(P_WAKE_RUN, wt);
    }
}

// ---- timeline trace (forkprof=2: phases PLUS the event ring) ----
//
// One ring of (site, pid, tsc) events covering the fork/exit/wait
// lifecycle; dumped raw at shutdown. 2048 dumped events is ~200
// lifecycle iterations — enough for median-pattern reading of where
// the wall goes, small enough not to drown the serial log.
pub const T_FORK_ENTRY: u8 = 0;
pub const T_FORK_DONE: u8 = 1;
pub const T_CTXSW_IN: u8 = 2;
pub const T_EXIT_ENTRY: u8 = 3;
pub const T_MMDROP_DONE: u8 = 4;
pub const T_NOTIFY_DONE: u8 = 5;
pub const T_SNDNOTIFY_DONE: u8 = 6;
pub const T_WAIT_ENTRY: u8 = 7;
pub const T_WAIT_FOUND: u8 = 8;
pub const T_REAP_DONE: u8 = 9;
pub const T_SCHED_SLEEP: u8 = 10;
pub const T_SCHED_WAKE: u8 = 11;
pub const T_NEWBORN_TAIL: u8 = 12;
pub const T_SIGCHECK: u8 = 13;
pub const T_SYSENTRY: u8 = 14;


const EV0: TEv = TEv {
    tsc: AtomicU64::new(0),
    site: AtomicU8::new(255),
    pid: AtomicU32::new(0),
};
static FORKPROF_RING: [TEv; 4096] = [EV0; 4096];
static FORKPROF_RING_CUR: AtomicU64 = AtomicU64::new(0);

/// Trace armed separately: `forkprof=2` (phases + trace); `forkprof=1`
/// is phases only.
///
/// perf-gap-hunt (2026-11-05): the syscall entry path calls this on
/// EVERY syscall (trap.rs handle_syscall), and the outlined call+frame
/// was per-syscall tax even with the trace off. Same contract as the
/// syscost irq_note gate: the wrapper skips inline unless ARMED == 2
/// (the only state where `on()` can be true), so the undetermined
/// state (0) and the settled-off state (1) both return false without
/// a call, and the armed body runs only on forkprof boots.
#[inline(always)]
pub fn trace_on() -> bool {
    if FORKPROF_ARMED.load(Ordering::Relaxed) != 2 {
        return false;
    }
    trace_on_armed()
}

/// Outlined body of [`trace_on`] (ARMED == 2 only).
#[cold]
#[inline(never)]
fn trace_on_armed() -> bool {
    forkprof_on() && crate::cmdline::get_param("forkprof").as_deref() == Some("2")
}

/// Record one timeline event (dropped silently when the trace is off).
#[inline]
pub fn trace(site: u8, pid: u32) {
    if !trace_on() {
        return;
    }
    let i = (FORKPROF_RING_CUR.fetch_add(1, Ordering::Relaxed) % 4096) as usize;
    FORKPROF_RING[i].tsc.store(forkprof_now(), Ordering::Relaxed);
    FORKPROF_RING[i].site.store(site, Ordering::Relaxed);
    FORKPROF_RING[i].pid.store(pid, Ordering::Relaxed);
}

/// Dump the ring (forkprof=2 only): one line per event, delta from the
/// previous event; last 2048 events.
pub fn report_trace() {
    if !trace_on() {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::hotpath::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0;
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let names: [&str; 16] = [
        "fork_entry",
        "fork_done",
        "ctxsw_in",
        "exit_entry",
        "mmdrop_done",
        "notify_done",
        "sndnotify_done",
        "wait_entry",
        "wait_found",
        "reap_done",
        "sched_sleep",
        "sched_wake",
        "newborn_tail",
        "sigcheck",
        "sysentry",
        "sysnr",
    ];
    let cur = FORKPROF_RING_CUR.load(Ordering::Relaxed);
    let start = cur.saturating_sub(2048);
    crate::println!("FTRACE BEGIN n={}", cur);
    let mut prev = 0u64;
    for k in start..cur {
        let i = (k % 4096) as usize;
        let t = FORKPROF_RING[i].tsc.load(Ordering::Relaxed);
        if t == 0 {
            continue;
        }
        let site = FORKPROF_RING[i].site.load(Ordering::Relaxed);
        let pid = FORKPROF_RING[i].pid.load(Ordering::Relaxed);
        let d = if prev == 0 { 0 } else { t.saturating_sub(prev) / per_us };
        prev = t;
        let nm = if (site as usize) < names.len() { names[site as usize] } else { "?" };
        crate::println!("FTRACE {} pid={} dt={}us", nm, pid, d);
    }
    crate::println!("FTRACE END");
}

const EMPTY: ForkPhaseStat = ForkPhaseStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

static FORKPROF_STATS: [ForkPhaseStat; P_COUNT] = [EMPTY; P_COUNT];

static FORKPROF_ARMED: AtomicU8 = AtomicU8::new(0);
static FORKPROF_REPORTED: AtomicBool = AtomicBool::new(false);

/// Cached arm check (self::on pattern; never cache before cmdline).
#[inline]
pub fn forkprof_on() -> bool {
    match FORKPROF_ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => {
                    match crate::cmdline::get_param("forkprof").as_deref() {
                        Some("1") | Some("2") => 2,
                        _ => 1,
                    }
                }
            };
            FORKPROF_ARMED.store(v, Ordering::Relaxed);
            v == 2
        }
    }
}

/// Cycle-counter read (raw TSC / cycle domain; the report converts).
#[inline]
pub fn forkprof_now() -> u64 {
    crate::arch::cpu::read_counter()
}

/// Account `elapsed = now() - t0` cycles against phase `p`.
#[inline]
pub fn add(p: usize, t0: u64) {
    let d = forkprof_now().saturating_sub(t0);
    let s = &FORKPROF_STATS[p];
    s.count.fetch_add(1, Ordering::Relaxed);
    s.cyc.fetch_add(d, Ordering::Relaxed);
    s.max.fetch_max(d, Ordering::Relaxed);
}

/// Scalar counter (pages copied, tables freed, ...) — reported raw.
#[inline]
pub fn forkprof_bump(p: usize, n: u64) {
    FORKPROF_STATS[p].cyc.fetch_add(n, Ordering::Relaxed);
}

#[inline]
pub fn peek(p: usize) -> (u64, u64, u64) {
    let s = &FORKPROF_STATS[p];
    (
        s.count.load(Ordering::Relaxed),
        s.cyc.load(Ordering::Relaxed),
        s.max.load(Ordering::Relaxed),
    )
}

/// Print the phase table. Called once from the shutdown cascade.
pub fn forkprof_report() {
    if !forkprof_on() || FORKPROF_REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::hotpath::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0; // cycle-domain report only on the other ports
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let names = [
        "fk_total",
        "fk_alloc",
        "fk_copythread",
        "fk_fdtable",
        "fk_mmfork",
        "fk_enqueue",
        "mm_snapshot",
        "mm_cow",
        "mm_replay",
        "ex_total",
        "ex_mmdrop",
        "drop_vmafree",
        "drop_rcu",
        "drop_ptfree",
        "ex_reparent",
        "ex_notify",
        "exec_total",
        "s_forks_vmas",
        "s_cow_ptes",
        "s_cow_tables",
        "s_mm_drops",
        "wt_body",
        "wt_sched",
        "wt_reap",
        "sched_notify",
        "wake_body",
        "pick",
        "ctxsw",
        "wake_run",
        "child_run",
    ];
    crate::println!("FORKPROF BEGIN hz={}", hz);
    for (i, name) in names.iter().enumerate() {
        let (c, cyc, max) = peek(i);
        if c == 0 && cyc == 0 {
            continue;
        }
        if c == 0 {
            // scalar-only slot (bump()ed without add())
            crate::println!(
                "FORKPROF {} scalar={}",
                name,
                cyc
            );
            continue;
        }
        let avg = cyc / c.max(1) / per_us;
        let mx = max / per_us;
        crate::println!(
            "FORKPROF {} n={} avg={}us max={}us",
            name,
            c,
            avg,
            mx
        );
    }
    crate::println!("FORKPROF END");
    report_trace();
}


// ════════════════ mmprof ══════════════════════════════
// mmap/mprotect stage profiler — attribution probe for the OH vm-op wall
// (syscost handoff: nr=9 mmap n=179k avg 72us max 722ms, nr=10 mprotect
// avg 53us, 50-70x the reference kernel's 1-5us class; distributeddata's
// 27-32s SA init rides this family).
//
// Armed by the kernel cmdline `mmprof=1` (own key, forkprof pattern; no
// switches section (hotpath.rs) bit consumed). Cost when off: one relaxed load per
// hook. Cost when on: ~6 rdtsc+fetch_add brackets per mmap/mprotect —
// noise against a 72us syscall.
//
// What it measures (the named candidate set from the perf handoff):
// - mmap: vma_write acquire wait, find_free_area search, VmaManager::add,
//   the MAP_FIXED replacement path (chunked munmap: surgery, per-page
//   walk, TLB flush tail, cond_resched yields), length buckets,
//   fixed/hint/anon placement counters, per-comm issuer attribution.
// - mprotect: vma_write acquire wait, coverage+EACCES walk, PTE rewrite
//   walk, per-page count.
// - munmap (both callers): body, surgery, walk, TLB tail, yields.
//
// Dump: periodic (timer heartbeat, every MMPROF_DUMP_SECS) + the
// shutdown cascade. Lines: MMPROF <name> n= avg=us max=us.

// ---- phase slots ----
pub const M_TOTAL: usize = 0; // sys_mmap_inner whole body
pub const M_BODY: usize = 1; // MmStruct::mmap body (x86_64)
pub const M_LOCKWAIT: usize = 2; // vma_write() acquire wait (mmap paths)
pub const M_FINDFREE: usize = 3; // find_free_area_locked search
pub const M_ADD: usize = 4; // VmaManager::add + vm accounting
pub const M_FIXEDMUN: usize = 5; // MAP_FIXED's self.munmap + install loop
pub const MU_BODY: usize = 6; // munmap body (both callers)
pub const MU_SURG: usize = 7; // per-chunk VMA surgery (release+split+re-add)
pub const MU_WALK: usize = 8; // unmap_pages body
pub const MU_TLB: usize = 9; // unmap_pages flush tail (IPI + full flush)
pub const MU_YIELD: usize = 10; // cond_resched() calls in the munmap loop
pub const MP_TOTAL: usize = 11; // sys_mprotect_inner whole body
pub const MP_LOCK: usize = 12; // mprotect vma_write acquire wait
pub const MP_COVER: usize = 13; // coverage walk + EACCES scan
pub const MP_PTE: usize = 14; // mprotect_pte_range (per-page rewrite)
pub const MP_TLB: usize = 15; // post-rewrite flush (riscv sfence arm)
pub const MMPROF_P_COUNT: usize = 16;

// ---- scalar slots (bump-counted, reported raw; separate namespace
// from the phase slots — bump() indexes SCALARS[0..P_SCALARS]) ----
pub const S_FIXED: usize = 0; // MAP_FIXED mmaps
pub const S_HINT: usize = 1; // hinted mmaps (addr != 0)
pub const S_ANON0: usize = 2; // kernel-placed mmaps (addr == 0)
pub const S_UNPAGES: usize = 3; // pages visited by unmap walks
pub const S_UNEMPTY: usize = 4; // visited pages with no PTE
pub const S_MPPAGES: usize = 5; // pages visited by mprotect walks
pub const S_VMAADD: usize = 6; // VmaManager::add calls
pub const S_MUNLOOPS: usize = 7; // munmap chunk iterations
pub const P_SCALARS: usize = 8;

// ---- length buckets for successful mmaps (count + cycles) ----
pub const B_LE64K: usize = 0;
pub const B_LE1M: usize = 1;
pub const B_LE16M: usize = 2;
pub const B_GT16M: usize = 3;
pub const B_N: usize = 4;
pub const B_NAMES: [&str; B_N] = ["len<=64K", "len<=1M", "len<=16M", "len>16M"];

// ---- per-comm issuer attribution (mmap count) ----
pub const COMM_SLOTS: usize = 8;


const MMPROF_EMPTY: PhaseStat = PhaseStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

pub static STATS: [PhaseStat; MMPROF_P_COUNT] = [MMPROF_EMPTY; MMPROF_P_COUNT];
pub static SCALARS: [AtomicU64; P_SCALARS] = [const { AtomicU64::new(0) }; P_SCALARS];
pub static BUCKETS: [PhaseStat; B_N] = [MMPROF_EMPTY; B_N];


const EMPTY_COMM: CommEnt = CommEnt {
    comm: [const { AtomicU8::new(0) }; 16],
    n: AtomicU64::new(0),
};
pub static COMMS: [CommEnt; COMM_SLOTS] = [EMPTY_COMM; COMM_SLOTS];

static MMPROF_ARMED: AtomicU8 = AtomicU8::new(0);

/// Cached arm check (self::forkprof_on pattern; never cache before cmdline).
#[inline]
pub fn mmprof_on() -> bool {
    match MMPROF_ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_param("mmprof").as_deref() {
                Some("1") => 2,
                _ => 1,
            };
            MMPROF_ARMED.store(v, Ordering::Relaxed);
            if v == 2 {
                crate::pr_warn!("MMPROF armed (cmdline mmprof=1)");
            }
            v == 2
        }
    }
}

/// Cycle-counter read (raw TSC / cycle domain; the report converts).
#[inline]
pub fn mmprof_now() -> u64 {
    crate::arch::cpu::read_counter()
}

/// Account `elapsed = now() - t0` cycles against phase `p`.
/// Out-of-range slots are dropped, never a panic (diagnostic discipline).
#[inline]
pub fn mmprof_add(p: usize, t0: u64) {
    if p >= MMPROF_P_COUNT {
        return;
    }
    let d = mmprof_now().saturating_sub(t0);
    let s = &STATS[p];
    s.count.fetch_add(1, Ordering::Relaxed);
    s.cyc.fetch_add(d, Ordering::Relaxed);
    s.max.fetch_max(d, Ordering::Relaxed);
}

/// Scalar counter (pages walked, fixed mmaps, ...).
#[inline]
pub fn mmprof_bump(p: usize, n: u64) {
    if p >= P_SCALARS {
        return;
    }
    SCALARS[p].fetch_add(n, Ordering::Relaxed);
}

/// Account a successful mmap's cycles into its length bucket.
#[inline]
pub fn bucket_len(total: usize, t0: u64) {
    let b = if total <= 64 * 1024 {
        B_LE64K
    } else if total <= 1024 * 1024 {
        B_LE1M
    } else if total <= 16 * 1024 * 1024 {
        B_LE16M
    } else {
        B_GT16M
    };
    let d = mmprof_now().saturating_sub(t0);
    BUCKETS[b].count.fetch_add(1, Ordering::Relaxed);
    BUCKETS[b].cyc.fetch_add(d, Ordering::Relaxed);
    BUCKETS[b].max.fetch_max(d, Ordering::Relaxed);
}

/// Attribute one mmap to its issuer's comm (8-slot linear table,
/// min-count eviction — the first-come table saturated with boot-time
/// one-shot processes and never named the churner; the churner's count
/// dwarfs theirs within a minute, so evicting the smallest slot
/// converges on it).
#[inline]
pub fn bump_comm() {
    let Some(t) = crate::sched::hotpath::current() else {
        return;
    };
    let comm: [u8; 16] = *t.comm();
    // fast path: present slot
    for slot in COMMS.iter() {
        let mut cur = [0u8; 16];
        for (i, b) in cur.iter_mut().enumerate() {
            *b = slot.comm[i].load(Ordering::Relaxed);
        }
        if cur == comm && slot.n.load(Ordering::Relaxed) != 0 {
            slot.n.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    // slow path: claim an empty slot, else evict the smallest count
    let mut victim = 0usize;
    let mut victim_n = u64::MAX;
    for (i, slot) in COMMS.iter().enumerate() {
        let n = slot.n.load(Ordering::Relaxed);
        if n == 0 {
            victim = i;
            victim_n = 0;
            break;
        }
        if n < victim_n {
            victim = i;
            victim_n = n;
        }
    }
    let slot = &COMMS[victim];
    if victim_n == 0 {
        for (i, b) in comm.iter().enumerate() {
            slot.comm[i].store(*b, Ordering::Relaxed);
        }
        slot.n.store(1, Ordering::Relaxed);
        return;
    }
    // evict: compare_exchange the count so a concurrent claimant cannot
    // resurrect the evicted comm under us
    if slot
        .n
        .compare_exchange(victim_n, 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        for (i, b) in comm.iter().enumerate() {
            slot.comm[i].store(*b, Ordering::Relaxed);
        }
    } else {
        slot.n.fetch_add(1, Ordering::Relaxed);
    }
}

const MMPROF_DUMP_SECS: u64 = 60;
static MMPROF_LAST_DUMP: AtomicU64 = AtomicU64::new(0);
static DUMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Timer-heartbeat hook (next to self::syscost_tick) — periodic dump.
pub fn mmprof_tick(now_jiffies: u64) {
    if !mmprof_on() {
        return;
    }
    if now_jiffies.saturating_sub(MMPROF_LAST_DUMP.load(Ordering::Relaxed))
        < MMPROF_DUMP_SECS * crate::config::KERNEL_HZ as u64
    {
        return;
    }
    MMPROF_LAST_DUMP.store(now_jiffies, Ordering::Relaxed);
    // Line emission is deferred to the dump kthread: a softirq emitter
    // carries a 100us emit-lock budget while the OH hilog flood holds the
    // console ~17ms per 256B line (emulated baud) — the lb336 t=60s dump
    // never reached the serial. Task context waits up to 2s and wins.
    ensure_dump_thread();
}

// ---- dump kthread ----
static DUMP_THREAD_ON: AtomicBool = AtomicBool::new(false);

/// Spawn the task-context dump thread (once; called from the timer
/// softirq tick and from debug::init — either way the thread starts once).
pub fn ensure_dump_thread() {
    if DUMP_THREAD_ON.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::api::process::kthread_run(
        mmprof_dump_fn,
        core::ptr::null_mut(),
        "mmprofd",
    );
}

extern "C" fn mmprof_dump_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::types::{Task, TaskState};
    loop {
        if crate::api::process::kthread_should_stop() {
            break;
        }
        // Sleep MMPROF_DUMP_SECS of jiffies (nanosleep discipline:
        // one-shot wakeup timer + INTERRUPTIBLE schedule).
        let current = match crate::sched::hotpath::current() {
            Some(t) => t,
            None => return 0, // no task context
        };
        // current() is the safe handle to the running task (we are it).
        let pid = current.pid();
        let wake_at = crate::timer::get_jiffies()
            + MMPROF_DUMP_SECS * crate::config::KERNEL_HZ as u64;
        let timer_id = crate::timer::add_timer_wakeup(wake_at, pid);
        current.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        // Re-arm IRQs so the timer tick reaches this CPU (R54).
        crate::arch::cpu::restore_irq(true);
        crate::sched::hotpath::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }
        let k = DUMP_SEQ.fetch_add(1, Ordering::Relaxed) % 3;
        report_line(k);
    }
    0
}

/// Print ONE compact stat as `name=avg/max(n)` inside a caller's line.
fn fmt_cell(out: &mut alloc::string::String, name: &str, c: u64, cyc: u64, max: u64, per_us: u64) {
    if c == 0 {
        if cyc != 0 {
            out.push_str(&alloc::format!(" {}=s{}", name, cyc));
        }
        return;
    }
    let avg = cyc / c.max(1) / per_us;
    let mx = max / per_us;
    out.push_str(&alloc::format!(" {}={}/{}(n{})", name, avg, mx, c));
}

fn stat(i: usize) -> (u64, u64, u64) {
    let s = &STATS[i];
    (
        s.count.load(Ordering::Relaxed),
        s.cyc.load(Ordering::Relaxed),
        s.max.load(Ordering::Relaxed),
    )
}

/// Emit ONE line per 60s dump, rotating through three variants.
/// Console discipline: the printk emit-lock carries a 100us budget in
/// softirq context and the OH boot floods at ~700us per UART line, so a
/// multi-line burst loses all but 1-2 lines (lb332 evidence). One
/// short line per dump survives; the full table completes every 3 min.
pub fn report_line(k: u64) {
    if !mmprof_on() {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::hotpath::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0; // cycle-domain report only on the other ports
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let secs = crate::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    match k {
        0 => {
            // A: mmap + mprotect stages
            let mut l = alloc::format!("MMPROF-A t={}s mmap:", secs);
            let (c, cy, mx) = stat(M_TOTAL);
            fmt_cell(&mut l, "tot", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_LOCKWAIT);
            fmt_cell(&mut l, "lock", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_FINDFREE);
            fmt_cell(&mut l, "find", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_ADD);
            fmt_cell(&mut l, "add", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_FIXEDMUN);
            fmt_cell(&mut l, "fixmun", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_TOTAL);
            fmt_cell(&mut l, "| mprot-tot", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_LOCK);
            fmt_cell(&mut l, "mplock", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_COVER);
            fmt_cell(&mut l, "mpcov", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_PTE);
            fmt_cell(&mut l, "mppte", c, cy, mx, per_us);
            crate::pr_warn!("{}", l);
        }
        1 => {
            // B: munmap stages + placement scalars
            let mut l = alloc::format!("MMPROF-B t={}s munmap:", secs);
            let (c, cy, mx) = stat(MU_BODY);
            fmt_cell(&mut l, "body", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_SURG);
            fmt_cell(&mut l, "surg", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_WALK);
            fmt_cell(&mut l, "walk", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_TLB);
            fmt_cell(&mut l, "tlb", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_YIELD);
            fmt_cell(&mut l, "yld", c, cy, mx, per_us);
            crate::pr_warn!(
                "{} | fixed={} hint={} anon0={} unpg={} unempty={} munloops={}",
                l,
                SCALARS[S_FIXED].load(Ordering::Relaxed),
                SCALARS[S_HINT].load(Ordering::Relaxed),
                SCALARS[S_ANON0].load(Ordering::Relaxed),
                SCALARS[S_UNPAGES].load(Ordering::Relaxed),
                SCALARS[S_UNEMPTY].load(Ordering::Relaxed),
                SCALARS[S_MUNLOOPS].load(Ordering::Relaxed)
            );
        }
        _ => {
            // C: length buckets + issuers
            let mut l = alloc::format!("MMPROF-C t={}s", secs);
            for b in 0..B_N {
                let s = &BUCKETS[b];
                fmt_cell(&mut l, B_NAMES[b], s.count.load(Ordering::Relaxed), s.cyc.load(Ordering::Relaxed), s.max.load(Ordering::Relaxed), per_us);
            }
            l.push_str(" | iss:");
            for slot in COMMS.iter() {
                let n = slot.n.load(Ordering::Relaxed);
                if n == 0 {
                    continue;
                }
                let mut comm = [0u8; 16];
                for (i, b) in comm.iter_mut().enumerate() {
                    *b = slot.comm[i].load(Ordering::Relaxed);
                }
                let end = comm.iter().position(|&c| c == 0).unwrap_or(16);
                let name = core::str::from_utf8(&comm[..end]).unwrap_or("?");
                l.push_str(&alloc::format!(" {}={}", name, n));
            }
            crate::pr_warn!("{}", l);
        }
    }
}


// ════════════════ lostpark ════════════════════════════
// Lost-park event ring (fix-lostpark lane).
//
// The SMP lost-park wedge (lmbench lat_sem / semop ping-pong at -smp 2)
// freezes a two-task IPC ping-pong with both vCPUs idle; every per-event
// print drowns in the 100k+ healthy rounds that precede the wedge, and
// the wedge itself is silent (no wake ever fires again). This module
// keeps a small lock-free ring of the LAST scheduling-relevant events
// (semop park enter/exit, refused wake tokens, semaphore value
// transitions) so the 1 Hz wakewatch heartbeat can dump the final
// window the moment it detects the frozen state (wakes delta == 0,
// global queue empty).
//
// Everything is behind the `rux:lostpark` trace event (retired
// record fast path is one Relaxed load when off.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Ring entries (keep modest: dumped on one serial line each).
pub const RING_LEN: usize = 64;

/// PUB peek surface (crate-flip rung debug): the debug report
/// half reads the ring.
pub static SEQ: AtomicU64 = AtomicU64::new(0);
static DUMPED: AtomicBool = AtomicBool::new(false);
pub static LOSTPARK_RING: [AtomicU64; RING_LEN * 5] = [const { AtomicU64::new(0) }; RING_LEN * 5];

/// Whether the ring is armed (fast record gate).
#[inline]
pub fn lostpark_armed() -> bool {
    crate::trace::EVENT_LOSTPARK.armed()
}

/// Record one event: (event, pid, a, b, jiffies). Lock-free; events may
/// interleave under SMP but each slot's seq makes the order recoverable.
pub fn lostpark_record(event: u64, pid: u64, a: u64, b: u64, jiffies: u64) {
    if !lostpark_armed() {
        return;
    }
    let s = SEQ.fetch_add(1, Ordering::Relaxed);
    let i = (s as usize % RING_LEN) * 5;
    LOSTPARK_RING[i].store(s, Ordering::Relaxed);
    LOSTPARK_RING[i + 1].store(event, Ordering::Relaxed);
    LOSTPARK_RING[i + 2].store(pid, Ordering::Relaxed);
    LOSTPARK_RING[i + 3].store(a, Ordering::Relaxed);
    LOSTPARK_RING[i + 4].store(b | (jiffies << 32), Ordering::Relaxed);
}
/// Dump the ring once (moved to the debug crate): same-named wrapper
/// for the wakewatch heartbeat's frozen-state branch; dispatches the
/// hook row.
pub fn dump_once(tag: &[u8]) {
    crate::api::debug::lostpark_dump_once(tag);
}


// ════════════════ cctx ════════════════════════════════
// Context-switch / wake / IPC-syscall cost anatomy (closegap lane,
// cmdline `cctx=1`).
//
// lmbench gap attribution probe for the four worst latency rows:
// lat_sem (12x), lat_unix (3.9x), lat_ctx (3.4-5.1x), lat_pipe (3.3x).
// Each row shares the same two bodies — the block/wake context-switch
// loop and a per-syscall body — so this probe brackets BOTH, per phase:
//
// - __schedule (prev's frame): guards (pre), GRQ lock wait, runtime
//   accounting, prev dequeue/requeue, pick_next_task, publish/unlock.
// - context_switch wrapper: FPU save, CR3 switch, FS/GS MSR swaps, the
//   switch asm itself (stamp written by the INCOMING task after
//   __switch_to returns), FPU restore.
// - Task::wake_up_impl: select_task_rq, the locked RUNNING+enqueue
//   section, the idle-claim/resched-IPI dispatch.
// - resched_cpu: the actual IPI send.
// - sys_semtimedop: prelude, each try_apply_semops, the blocking park.
// - pipe read/write, unix stream send/recv: whole-op brackets.
//
// Discipline: same contract as fsw/syscost — own cmdline key (`cctx=1`),
// lazy-armed after cmdline init, NS sums with a count each (averages at
// dump), zero traffic when off (one Relaxed load per stamp call), dump
// from the shutdown cascade (every probe rig ends in reboot(2)).
//
// Read numbers as upper bounds: each bracket pair adds ~20-30 ns of its
// own clock-read cost to its phase; the ranking and aggregate split are
// solid.


// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

// === __schedule phases (prev's frame) =====================================
/// __schedule entries past the idle fast-path guard.
pub static SCH_N: AtomicU64 = AtomicU64::new(0);
/// entry -> just before the GRQ acquire (prev-resolution + heal guards).
pub static SCH_PRE_NS: AtomicU64 = AtomicU64::new(0);
/// GRQ lock acquire time (contended spins land here).
pub static SCH_LOCKWAIT_NS: AtomicU64 = AtomicU64::new(0);
/// Runtime accounting (update_curr / DL consumption).
pub static SCH_ACCT_NS: AtomicU64 = AtomicU64::new(0);
/// prev dequeue (sleeper) / requeue (still runnable).
pub static SCH_DEQREQ_NS: AtomicU64 = AtomicU64::new(0);
/// pick_next_task (class queues + heal machinery).
pub static SCH_PICK_NS: AtomicU64 = AtomicU64::new(0);
/// current-slot publish + schedstat + unlock.
pub static SCH_PUB_NS: AtomicU64 = AtomicU64::new(0);
/// Real context switches (next != prev) — the divisor for CW_* rows.
pub static CTX_N: AtomicU64 = AtomicU64::new(0);

// === grq-10ns: exact GRQ critical-section + CFS timeline anatomy ========
/// __schedule's GRQ critical section: from lock acquired (past the
/// lock_irqsave) to unlock_irqretain — the ACTUAL lock hold (acct +
/// deq/req + pick + pub in one bracket, one clock-read pair).
pub static SCH_HOLD_NS: AtomicU64 = AtomicU64::new(0);
/// CFS timeline mutations (intrusive insert/erase in fair.rs —
/// timeline_insert + timeline_remove of every kind: pick unlink, prev
/// requeue, wake enqueue, sleeper dequeue).
pub static FAIR_TL_N: AtomicU64 = AtomicU64::new(0);
pub static FAIR_TL_NS: AtomicU64 = AtomicU64::new(0);
/// CFS pick walks entered (pick_next_cpu) and candidates consulted.
pub static FAIR_PICK_N: AtomicU64 = AtomicU64::new(0);
pub static FAIR_WALK_N: AtomicU64 = AtomicU64::new(0);

// === context_switch wrapper ===============================================
pub static CW_N: AtomicU64 = AtomicU64::new(0);
/// fpu_save_for_switch (fxsave/xsave into the outgoing task).
pub static CW_FPUSAVE_NS: AtomicU64 = AtomicU64::new(0);
/// switch_mm / CR3 write (counted separately: CW_MM_N).
pub static CW_MM_N: AtomicU64 = AtomicU64::new(0);
pub static CW_MM_NS: AtomicU64 = AtomicU64::new(0);
/// FS/GS base MSR read/write pair set.
pub static CW_MSR_NS: AtomicU64 = AtomicU64::new(0);
/// __switch_to asm proper: stamp before the call (outgoing frame), stamp
/// after it returns (incoming frame) — the true register/stack switch.
pub static CW_ASM_NS: AtomicU64 = AtomicU64::new(0);
/// restore_fpu of the incoming task.
pub static CW_FPUREST_NS: AtomicU64 = AtomicU64::new(0);

/// Per-CPU switch-phase stamp (arch context_switch): the OUTGOING task
/// stores its pre-asm stamp, the INCOMING task on the same CPU (IRQs are
/// off across the switch) reads it back and closes the ASM bracket —
/// prev's frame is frozen mid-__switch_to, so the stamp cannot live in a
/// local.
pub static SW_STAMP: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

// === wake path =============================================================
/// Task::wake_up_impl sleeping-target wakes.
pub static WK_N: AtomicU64 = AtomicU64::new(0);
/// select_task_rq (sched-class CPU choice).
pub static WK_SEL_NS: AtomicU64 = AtomicU64::new(0);
/// wake_up_enqueue_opt (GRQ lock + RUNNING transition + enqueue + preempt
/// check). A cross-CPU steer adds its own GRQ acquire inside this window.
pub static WK_ENQ_NS: AtomicU64 = AtomicU64::new(0);
/// idle-claim + resched IPI / preempt_check_wake dispatch.
pub static WK_DISP_NS: AtomicU64 = AtomicU64::new(0);

// === IPI ===================================================================
pub static IPI_N: AtomicU64 = AtomicU64::new(0);
pub static IPI_NS: AtomicU64 = AtomicU64::new(0);

// === semop =================================================================
pub static SM_N: AtomicU64 = AtomicU64::new(0);
/// entry -> first try_apply (copies, perms find, nsems validate).
pub static SM_PRE_NS: AtomicU64 = AtomicU64::new(0);
/// Every try_apply_semops call (count SM_TRY_N).
pub static SM_TRY_N: AtomicU64 = AtomicU64::new(0);
pub static SM_TRY_NS: AtomicU64 = AtomicU64::new(0);
/// Blocking park: find_blocking + register + retry apply + schedule().
pub static SM_BLK_N: AtomicU64 = AtomicU64::new(0);
pub static SM_BLK_NS: AtomicU64 = AtomicU64::new(0);

// === pipe / unix ===========================================================
pub static PW_N: AtomicU64 = AtomicU64::new(0);
pub static PW_NS: AtomicU64 = AtomicU64::new(0);
pub static PR_N: AtomicU64 = AtomicU64::new(0);
pub static PR_NS: AtomicU64 = AtomicU64::new(0);
pub static UW_N: AtomicU64 = AtomicU64::new(0);
pub static UW_NS: AtomicU64 = AtomicU64::new(0);
pub static UR_N: AtomicU64 = AtomicU64::new(0);
pub static UR_NS: AtomicU64 = AtomicU64::new(0);

/// Bracket timestamp (0 when the probe is off — skips the clock read).
#[inline]
pub fn cctx_now() -> u64 {
    if cctx_on() {
        crate::timer::monotonic_ns_since_boot()
    } else {
        0
    }
}

/// Close a bracket opened with now() (no-op when t0 == 0, i.e. off).
#[inline]
pub fn cctx_add(dst: &AtomicU64, t0: u64) {
    if t0 != 0 {
        dst.fetch_add(
            crate::timer::monotonic_ns_since_boot().saturating_sub(t0),
            Ordering::Relaxed,
        );
    }
}

/// Bump a count (no-op when off).
#[inline]
pub fn cctx_bump(c: &AtomicU64) {
    if cctx_on() {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

/// Probe armed? Tri-state exactly like fsw (0 = unresolved, 1 = off, 2 =
/// on); never cache a verdict before the cmdline exists.
static CCTX_ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn cctx_on() -> bool {
    match CCTX_ARMED8.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let cctx_v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("cctx").as_deref() == Some("1"),
            };
            CCTX_ARMED8.store(if cctx_v { 2 } else { 1 }, Ordering::Relaxed);
            crate::pr_warn!(
                "cctx: {} (cmdline cctx param = {:?})",
                if cctx_v { "armed" } else { "off" },
                crate::cmdline::get_param("cctx")
            );
            cctx_v
        }
    }
}

fn cctx_v(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}

fn cctx_avg(sum: &AtomicU64, n: &AtomicU64) -> u64 {
    let n = cctx_v(n);
    if n == 0 {
        0
    } else {
        cctx_v(sum) / n
    }
}

/// Print the counter snapshot (called from the shutdown cascade; every
/// cctx probe rig ends with reboot(2), so the numbers ride the serial).
pub fn cctx_dump() {
    if !cctx_on() {
        return;
    }
    let secs = crate::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    crate::pr_warn!(
        // Schedule-side anatomy (all NS sums; averages = sum / leftmost
        // count). SCH rows divide by SCH_N, CW rows by CTX_N.
        "CCTX1 t={}s sch n={} pre={} lock={} acct={} deq={} pick={} pub={} ctxsws={}",
        secs,
        cctx_v(&SCH_N),
        cctx_avg(&SCH_PRE_NS, &SCH_N),
        cctx_avg(&SCH_LOCKWAIT_NS, &SCH_N),
        cctx_avg(&SCH_ACCT_NS, &SCH_N),
        cctx_avg(&SCH_DEQREQ_NS, &SCH_N),
        cctx_avg(&SCH_PICK_NS, &SCH_N),
        cctx_avg(&SCH_PUB_NS, &SCH_N),
        cctx_v(&CTX_N),
    );
    crate::pr_warn!(
        // context_switch anatomy: fpu save / mm (avg over switches THAT
        // wrote CR3: CW_MM_N) / msr / asm / fpu restore.
        "CCTX2 t={}s csw n={} fpusave={} mm={}/{} (n={}, avg={}) msr={} asm={} fpurest={}",
        secs,
        cctx_v(&CW_N),
        cctx_avg(&CW_FPUSAVE_NS, &CW_N),
        cctx_v(&CW_MM_NS),
        cctx_avg(&CW_MM_NS, &CTX_N),
        cctx_v(&CW_MM_N),
        cctx_avg(&CW_MM_NS, &CW_MM_N),
        cctx_avg(&CW_MSR_NS, &CW_N),
        cctx_avg(&CW_ASM_NS, &CW_N),
        cctx_avg(&CW_FPUREST_NS, &CW_N),
    );
    crate::pr_warn!(
        "CCTX3 t={}s wake n={} sel={} enq={} disp={} ipi n={} avg={}",
        secs,
        cctx_v(&WK_N),
        cctx_avg(&WK_SEL_NS, &WK_N),
        cctx_avg(&WK_ENQ_NS, &WK_N),
        cctx_avg(&WK_DISP_NS, &WK_N),
        cctx_v(&IPI_N),
        cctx_avg(&IPI_NS, &IPI_N),
    );
    crate::pr_warn!(
        // grq-10ns: GRQ hold + CFS timeline anatomy. hold = the exact
        // critical section (lock acquired -> unlock, irq-retained); tl =
        // the intrusive-timeline mutation cost; walks = candidates
        // consulted per pick walk.
        "CCTX6 t={}s grqhold n={} avg={} tl n={} avg={} picks={} walkavg={}",
        secs,
        cctx_v(&SCH_N),
        cctx_avg(&SCH_HOLD_NS, &SCH_N),
        cctx_v(&FAIR_TL_N),
        cctx_avg(&FAIR_TL_NS, &FAIR_TL_N),
        cctx_v(&FAIR_PICK_N),
        cctx_avg(&FAIR_WALK_N, &FAIR_PICK_N),
    );
    crate::pr_warn!(
        "CCTX4 t={}s sem n={} pre={} try n={} avg={} blk n={} avg={}",
        secs,
        cctx_v(&SM_N),
        cctx_avg(&SM_PRE_NS, &SM_N),
        cctx_v(&SM_TRY_N),
        cctx_avg(&SM_TRY_NS, &SM_TRY_N),
        cctx_v(&SM_BLK_N),
        cctx_avg(&SM_BLK_NS, &SM_BLK_N),
    );
    crate::pr_warn!(
        "CCTX5 t={}s pipe w n={} avg={} r n={} avg={} unix w n={} avg={} r n={} avg={}",
        secs,
        cctx_v(&PW_N),
        cctx_avg(&PW_NS, &PW_N),
        cctx_v(&PR_N),
        cctx_avg(&PR_NS, &PR_N),
        cctx_v(&UW_N),
        cctx_avg(&UW_NS, &UW_N),
        cctx_v(&UR_N),
        cctx_avg(&UR_NS, &UR_N),
    );
}


// ════════════════ reaper ══════════════════════════════
// Reaper forensics — the OH launcher/systemui app-cycle death hunt
// (wake-history TASK §110, the waitid leg of the reaper probe).
//
// The first leg (a32046b8d) printed reaps at the BLOCKING wait4 path
// only: 17 reaps in 240 s, all clean EXITCODE helpers, and NO launcher
// reap. The launcher/systemui app-cycle children of appspawn die with
// childexit status=0xfffffffe (exit_group(-2): the raw user arg, which
// the Linux ABI masks to WEXITSTATUS 254) and either die through a
// waitid reap, a WNOHANG wait4 reap, or linger un-reaped as zombies
// between dumps. This module completes the picture with four arms:
//
//   SPAWN   — a task naming itself com.* (PR_SET_NAME or exec basename):
//             the appspawn-child incarnation clock starts here.
//   ZEXIT   — every LEADER task entering ZOMBIE: pid, comm, decoded
//             exit status, parent pid+comm, age since SPAWN, and the
//             task's always-on last-syscall latch (what it was doing).
//   REAP4/REAP4N/REAPW — every successful wait4 reap (blocking and the
//             WNOHANG arm the first leg missed) and waitid reap
//             (WNOWAIT peeks marked `-nw`): who reaped which child,
//             status decoded. REAPW-POLL samples the no-event poll
//             shape (caller + options) so poll loops are visible
//             without flooding.
//   ZLINGER — 1 Hz scan from the timer softirq: zombies un-reaped
//             longer than ZLINGER_AFTER_SECS, with the parent's state
//             and last-syscall latch (what the non-reaping parent is
//             blocked on instead of reaping).
//
// Status decode reference (this kernel's encoding): sys_exit stores
// `(status & 0xff) << 8`; a fatal signal stores `-signo`. wait4's
// user wstatus uses the Linux wait-status layout (sig in bits 0-6,
// core in bit 7, exit code in bits 8-15); waitid reports si_code
// CLD_* (1 EXITED, 2 KILLED, 3 DUMPED, 5 STOPPED, 6 CONTINUED) and a
// si_status that is the raw exit code (0-255) or term signal.
//
// Everything is OFF unless booted with `rux:schedtrace (retired debug=schedtrace)` — the SAME
// switch as the first-leg wait4 arm. The debug switch table is frozen;
// this is an extension of that existing gated probe, not a new switch.
//
// Locking: one Spinlock per map; sections are short copy in/out. The
// timer-softirq scan uses the try-variant of the pid-hash walk (a
// partial snapshot is fine — the next second retries).
//
// CRATE-FLIP RUNG debug: the SPAWN/ZEXIT/REAP4/REAPW/POLL print
// arms moved to the debug service crate (kernel/debug/src/
// reaper.rs) — every arm fires only through these same-named wrappers,
// whose call sites (do_exit, exec, the wait4/waitid syscall bodies)
// pre-gate on reaper::on(), so the row dispatch is armed-boot-only.
// THIS file keeps the on() gate, the 1 Hz ZLINGER scan (timer
// softirq) and the EXIT_MAP/NAME_MAP statics (pub peek + write
// surface for the crate's arms).

/// Name-set (SPAWN) tracking capacity. Small: only com.*-prefixed names
/// are recorded (OH bundle names — appspawn children + a handful of
/// services), dozens per boot. PUB: the debug arms index it.
pub const NAME_MAP_LEN: usize = 32;

/// Zombie exit-time map capacity (pid → exit jiffies + code), for
/// ZLINGER age. Rotating overwrite: the ZEXIT prints carry the same
/// data durably; the map only has to survive the 10 s linger window.
/// PUB: the debug arms index it.
pub const EXIT_MAP_LEN: usize = 96;

/// A zombie un-reaped longer than this is reported as lingering.
const ZLINGER_AFTER_SECS: u64 = 10;

/// Re-report a still-lingering zombie at this cadence (age growth).
const ZLINGER_REPEAT_SECS: u64 = 30;

/// Print budget for the ZLINGER arm (runaway exit storms must not eat
/// the serial).
const MAX_ZLINGER_PRINTS: u32 = 96;


pub static NAME_MAP: crate::sync::types::Spinlock<[NameRec; NAME_MAP_LEN]> =
    crate::sync::types::Spinlock::new([NameRec { pid: 0, t_set: 0 }; NAME_MAP_LEN]);

pub static EXIT_MAP: crate::sync::types::Spinlock<[ExitRec; EXIT_MAP_LEN]> =
    crate::sync::types::Spinlock::new([ExitRec { pid: 0, t_exit: 0, code: 0, last_report: 0 }; EXIT_MAP_LEN]);

/// The gate for every arm here (rux:schedtrace (retired debug=schedtrace)).
#[inline]
pub fn reaper_on() -> bool {
    crate::trace::EVENT_SCHEDTRACE.armed()
}

/// jiffies → "123.4s" seconds string (one decimal). PUB: the debug
/// arms print with it.
pub fn secs(j: u64) -> alloc::string::String {
    let hz = crate::config::KERNEL_HZ as u64;
    let whole = j / hz;
    let tenths = (j % hz) * 10 / hz;
    alloc::format!("{}.{}s", whole, tenths)
}

/// Decode this kernel's internal exit_code: `>= 0` = `(status<<8)`,
/// `< 0` = death by signal `-code`. PUB (debug arms).
pub fn decode_kernel_code(code: i32) -> (&'static str, i32) {
    if code < 0 {
        ("SIG", -code)
    } else {
        ("exit", (code >> 8) & 0xff)
    }
}

/// Decode a wait4 wstatus (Linux wait-status layout). PUB (debug arms).
pub fn decode_wstatus(st: i32) -> (&'static str, i32) {
    let sig = st & 0x7f;
    if sig == 0 && st & 0xff00 != 0 {
        ("exit", (st >> 8) & 0xff)
    } else if sig == 0 {
        ("exit0", 0)
    } else if st & 0x80 != 0 {
        ("SIGcore", sig)
    } else {
        ("SIG", sig)
    }
}

/// Decode a waitid si_code (CLD_*). PUB (debug arms).
pub fn decode_si_code(c: i32) -> &'static str {
    match c {
        1 => "CLD_EXITED",
        2 => "CLD_KILLED",
        3 => "CLD_DUMPED",
        5 => "CLD_STOPPED",
        6 => "CLD_CONTINUED",
        _ => "CLD-?",
    }
}

/// comm bytes → printable &str (NUL-trimmed, lossy). PUB (debug arms).
pub fn comm_str(raw: &[u8]) -> &str {
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    core::str::from_utf8(&raw[..len]).unwrap_or("?")
}

/// SPAWN arm (moved to the debug crate): same-named wrapper; the call
/// sites (exec basename / PR_SET_NAME) pre-gate on on().
pub fn note_nameset(pid: u32, comm: &[u8]) {
    crate::api::debug::reaper_note_nameset(pid, comm)
}

/// ZEXIT arm (moved to the debug crate): same-named wrapper; called
/// from do_exit right after the ZOMBIE store + notify resolution (the
/// call site pre-gates on on()).
pub fn on_zombie(pid: u32, code: i32, parent_pid: u32, comm: &[u8; 16], last_nr: u32, last_a0: u64) {
    crate::api::debug::reaper_on_zombie(pid, code, parent_pid, comm, last_nr, last_a0)
}

/// REAP arm: successful wait4 reap (moved to the debug crate):
/// same-named wrapper; `nohang` distinguishes the blocking do_wait path
/// (REAP4) from the WNOHANG do_wait_nonblock path (REAP4N).
pub fn on_reap_wait4(reaper: u32, child: u32, wstatus: i32, nohang: bool) {
    crate::api::debug::reaper_on_reap_wait4(reaper, child, wstatus, nohang)
}

/// REAP arm: successful waitid event (moved to the debug crate):
/// same-named wrapper; `wnowait` marks a peek that left the child
/// reapable; `via_pidfd` marks the pidfd idtype arm.
pub fn on_reap_waitid(
    reaper: u32,
    si_pid: u32,
    si_code: i32,
    si_status: i32,
    idtype: i32,
    id: i32,
    options: i32,
    wnowait: bool,
    via_pidfd: bool,
) {
    crate::api::debug::reaper_on_reap_waitid(
        reaper,
        si_pid,
        si_code,
        si_status,
        idtype,
        id,
        options,
        wnowait,
        via_pidfd,
    )
}

/// Sampled no-event waitid print (moved to the debug crate):
/// same-named wrapper; proves the poll shape without flooding.
pub fn poll_note(reaper: u32, idtype: i32, id: i32, options: i32) {
    crate::api::debug::reaper_poll_note(reaper, idtype, id, options)
}

/// ZLINGER arm: 1 Hz zombie scan, called from the timer softirq under
/// the SchedTrace gate. Uses the try-variant pid-hash walk (a held
/// bucket lock means a partial snapshot — fine, next second retries).
pub fn zombie_scan(now: u64) {
    use core::sync::atomic::AtomicU32;

    static N: AtomicU32 = AtomicU32::new(0);

    // Collect under the bucket locks; print after (no printing while
    // holding a pid-hash bucket spinlock).
    let mut found: [(u32, i32, [u8; 16], u32, u32); 16] = [(0, 0, [0; 16], 0, 0); 16];
    let mut nfound = 0usize;
    crate::process::hotpath::pid_hash_for_each_task_try(|t| {
        if nfound >= found.len() {
            return;
        }
        let task = t;
        if task.state() == crate::process::types::TaskState::new(
            crate::process::types::TaskState::ZOMBIE,
        ) && task.pid() != 0
        {
            found[nfound] = (
                task.pid(),
                task.exit_code(),
                *task.comm(),
                task.ppid(),
                task.ext.last_syscall_nr.load(Ordering::Relaxed),
            );
            nfound += 1;
        }
    });

    if nfound == 0 {
        return;
    }

    let hz = crate::config::KERNEL_HZ as u64;
    for i in 0..nfound {
        let (pid, code, comm, ppid, _lastsys) = found[i];
        let (t_exit, last_report) = {
            let map = EXIT_MAP.lock();
            match map.iter().find(|r| r.pid == pid) {
                Some(r) => (r.t_exit, r.last_report),
                None => {
                    // Exited before the probe armed / map rotated away:
                    // lower-bound age from the scan's own first sight.
                    (0, 0)
                }
            }
        };
        let age = if t_exit != 0 {
            now.saturating_sub(t_exit)
        } else {
            0
        };
        let due = if t_exit != 0 {
            age >= ZLINGER_AFTER_SECS * hz
                && now.saturating_sub(last_report) >= ZLINGER_REPEAT_SECS * hz
        } else {
            // Unknown exit time: report at the repeat cadence only.
            now.saturating_sub(last_report) >= ZLINGER_REPEAT_SECS * hz
        };
        if !due || N.load(Ordering::Relaxed) >= MAX_ZLINGER_PRINTS {
            continue;
        }
        if N.fetch_add(1, Ordering::Relaxed) >= MAX_ZLINGER_PRINTS {
            break;
        }
        {
            let mut map = EXIT_MAP.lock();
            if let Some(slot) = map.iter_mut().find(|r| r.pid == pid) {
                slot.last_report = now;
            } else {
                // Unknown-exit zombie adopted into the map so the repeat
                // cadence applies to it too.
                let mut oldest = 0;
                for k in 1..EXIT_MAP_LEN {
                    if map[k].t_exit < map[oldest].t_exit {
                        oldest = k;
                    }
                }
                map[oldest] = ExitRec { pid, t_exit: 0, code, last_report: now };
            }
        }
        let (kind, val) = decode_kernel_code(code);
        // Name the non-reaping parent and what it is blocked on instead.
        let parent_desc = crate::process::hotpath::find_task_by_pid(ppid)
            .map(|p| {
                alloc::format!(
                    "{}('{}' state={:#x} lastsys={} a0={:#x})",
                    ppid,
                    comm_str(p.comm()),
                    p.state().bits(),
                    p.ext.last_syscall_nr.load(Ordering::Relaxed),
                    p.ext.last_syscall_a0.load(Ordering::Relaxed)
                )
            })
            .unwrap_or_else(|| alloc::format!("{}(gone)", ppid));
        let linger = if age != 0 {
            secs(age)
        } else {
            alloc::string::String::from(">=scan-window")
        };
        crate::pr_warn!(
            "ZLINGER t={} pid={} comm='{}' parent={} {}={} linger={}",
            secs(now),
            pid,
            comm_str(&comm),
            parent_desc,
            kind,
            val,
            linger
        );
    }
}
