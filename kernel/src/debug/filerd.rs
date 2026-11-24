//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! rux:filerd (retired debug=filerd) — cached-file read cost anatomy probe (bw_file_rd lane).
//!
//! The lmbench `bw_file_rd 1m io_only` row ran 50x slower than the
//! reference kernel (669 MB/s vs 33.6 GB/s, KVM, strong profile) while
//! `bw_mmap_rd` sat at parity and the PageCacheOps counters proved the
//! benchmark's reads were 100% page-cache hits with zero disk I/O.
//! That pinned the gap to pure CPU cost somewhere between `sys_read`
//! and the cached-page copyout, but not WHERE — this probe decomposes
//! the warm read(2) path into phases and reports cycle totals per phase
//! through /proc/filerd:
//!
//! - `outer`  — the ext4 fast-path call as seen from the syscall layer
//!              (fd lookup excluded: brackets the ext4_file_read_user_vfs
//!              call inside read_file_core)
//! - `lock`   — f_pos io_lock acquisition + position read
//! - `pin`    — page_cache::get_range_into (claim + per-inode lock +
//!              BTreeMap walk + per-page pin)
//! - `copy`   — the per-page dst.put copyout loop (copy_to_user)
//! - `unpin`  — page_cache::put_range (claim + lock + per-page unpin)
//! - `epilog` — set_pos + atime stamp + inotify notify
//! - `miss`   — how many read calls took the batch-fill path at all
//!
//! Cycles are raw RDTSC ticks; `tsc_hz` is self-measured at read time by
//! bracketing a ~10 ms busy window between the calibrated 10 MHz
//! timekeeper and RDTSC. All hooks are behind a single `rux:filerd (retired debug=filerd)`
//! cmdline switch (one relaxed atomic load per read call when enabled,
//! zero instrumentation cost when off). OFF by default like every DEBUG
//! probe.

#![forbid(unsafe_code)]

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::AtomicU64;

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
pub fn on() -> bool {
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
    crate::debug::debug_hooks::filerd_generate()
}
