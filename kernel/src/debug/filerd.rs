//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! debug=filerd — cached-file read cost anatomy probe (bw_file_rd lane).
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
//! timekeeper and RDTSC. All hooks are behind a single `debug=filerd`
//! cmdline switch (one relaxed atomic load per read call when enabled,
//! zero instrumentation cost when off). OFF by default like every debug
//! probe.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicU64};

/// Fast-path gate (one relaxed load). Published from the debug cmdline
/// switch by `set_enabled`; read on every instrumented read call.
static ENABLED: AtomicBool = AtomicBool::new(false);

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

/// Enable/disable at runtime (debug cmdline switch, sysrq hook, tests).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Relaxed);
}

/// Is the probe armed? One relaxed load on each instrumented call.
#[inline]
pub fn on() -> bool {
    ENABLED.load(Relaxed)
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

/// Estimate the RDTSC frequency by bracketing a ~10 ms window between
/// the calibrated 10 MHz timekeeper and raw RDTSC.
fn measure_tsc_hz() -> u64 {
    // 10 MHz timekeeper: 100_000 ticks = 10 ms.
    const WINDOW: u64 = 100_000;
    let t0 = crate::drivers::timer::read_time();
    let c0 = crate::arch::cpu::read_counter();
    while crate::drivers::timer::read_time().wrapping_sub(t0) < WINDOW {
        core::hint::spin_loop();
    }
    let c1 = crate::arch::cpu::read_counter();
    let dt = crate::drivers::timer::read_time().wrapping_sub(t0);
    if dt == 0 || c1 <= c0 {
        return 0;
    }
    // cycles/s = (c1-c0)/dt * 10_000_000
    (c1.wrapping_sub(c0)) * 10_000_000 / dt
}

/// Render /proc/filerd: phase counters (count, total cycles, ns/call
/// and share of the outer total) plus the TSC frequency.
pub fn generate() -> alloc::vec::Vec<u8> {
    use alloc::format;
    use alloc::string::String;

    let hz = measure_tsc_hz();
    let outer_n = OUTER_N.load(Relaxed);
    let outer_c = OUTER_CYCLES.load(Relaxed);
    let mut out = String::with_capacity(512);
    out.push_str(&format!("tsc_hz {}\n", hz));
    let phases: [(&str, &AtomicU64); 5] = [
        ("lock", &LOCK_CYCLES),
        ("pin", &PIN_CYCLES),
        ("copy", &COPY_CYCLES),
        ("unpin", &UNPIN_CYCLES),
        ("epilog", &EPILOG_CYCLES),
    ];
    for (name, ctr) in phases {
        let c = ctr.load(Relaxed);
        let ns = if hz > 0 && outer_n > 0 {
            c / outer_n * 1_000_000_000 / hz
        } else {
            0
        };
        let share = if outer_c > 0 { c * 100 / outer_c } else { 0 };
        out.push_str(&format!(
            "{}_cycles {} ns_per_read {} pct_of_outer {}\n",
            name, c, ns, share
        ));
    }
    let outer_ns = if hz > 0 && outer_n > 0 {
        outer_c / outer_n * 1_000_000_000 / hz
    } else {
        0
    };
    out.push_str(&format!(
        "outer_calls {} outer_cycles {} ns_per_read {} miss_calls {}\n",
        outer_n,
        outer_c,
        outer_ns,
        MISS_N.load(Relaxed)
    ));
    out.into_bytes()
}
