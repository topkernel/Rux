//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The filerd REPORT half (moved from kernel/src/debug/filerd.rs,
//! crate-flip rung rux-debug): the /proc/filerd renderer and the TSC
//! self-calibration. The counters (pub statics) and the on/now/charge/
//! note_miss record fns stay framework-side (ext4 read path). Body
//! moved verbatim.

use core::sync::atomic::Ordering::Relaxed;

use crate::debug::filerd::*;

/// Estimate the RDTSC frequency by bracketing a ~10 ms window between
/// the calibrated 10 MHz timekeeper and raw RDTSC.
fn measure_tsc_hz() -> u64 {
    // 10 MHz timekeeper: 100_000 ticks = 10 ms.
    const WINDOW: u64 = 100_000;
    let t0 = crate::timer::read_time();
    let c0 = crate::arch::cpu::read_counter();
    while crate::timer::read_time().wrapping_sub(t0) < WINDOW {
        core::hint::spin_loop();
    }
    let c1 = crate::arch::cpu::read_counter();
    let dt = crate::timer::read_time().wrapping_sub(t0);
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
    let phases: [(&str, &core::sync::atomic::AtomicU64); 5] = [
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
