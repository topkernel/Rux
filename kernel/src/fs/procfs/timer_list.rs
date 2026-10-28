//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/timer_list — the pending kernel timers, in the standard
//! structure (verified against this host's live file).
//!
//! The reference layout:
//!
//! ```text
//! Timer List Version: v0.10
//! HRTIMER_MAX_CLOCK_BASES: 8
//! now at 97796069351310 nsecs
//!
//! cpu: 0
//!  clock 0:
//!   .base:       00000000634db09b
//!   .index:      0
//!   .resolution: 1 nsecs
//!   .offset:     0 nsecs
//! active timers:
//!  #0: <00000000df9f1c4f>, watchdog_timer_fn, S:01
//!  # expires at 97796251936169-97796251936169 nsecs [in 182584859 to 182584859 nsecs]
//! ```
//!
//! Rux has no hrtimer clock bases and no per-timer callback pointer:
//! the timers are a single list of deadlines with a delivery action
//! (signal, wake, timerfd). The structure above follows the reference
//! host byte-for-byte; the two fields that have no Rux counterpart are
//! filled with what the kernel does know:
//!
//! - the `<...>` field is the timer id (the reference prints the hrtimer
//!   struct address; Rux's timers have no such struct);
//! - the name field names the timer's ACTION instead of a callback
//!   function — "alarm" for a signal, "wake" for a wakeup, "timerfd"
//!   for a timerfd, "oneshot" for a plain deadline.
//!
//! The state column is "S:01" (HRTIMER_STATE_ENQUEUED): every
//! timer in the list is armed.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Nanoseconds per time-CSR tick (Rux's time base is 100ns).
const TICKS_TO_NS: u64 = 100;

/// Generate /proc/timer_list content.
pub fn generate() -> Vec<u8> {
    let mut out = String::new();

    out.push_str("Timer List Version: v0.10\n");
    out.push_str("HRTIMER_MAX_CLOCK_BASES: 8\n");
    let now_ns = crate::drivers::timer::monotonic_ns_since_boot();
    out.push_str(&format!("now at {} nsecs\n\n", now_ns));

    // One section per online CPU, each carrying all
    // HRTIMER_MAX_CLOCK_BASES (= 8) clock bases. Rux's timers live in
    // one global table armed on the monotonic clock — every timer is
    // listed under cpu 0's clock 0 (the CLOCK_MONOTONIC base) and the
    // remaining bases/CPU sections print empty, exactly as a base
    // with no armed timer shows on the reference host.
    let num_cpus = crate::arch::smp::num_started_cpus().min(crate::config::MAX_CPUS);
    let mut timers = crate::timer::snapshot_timers();
    // The timers are listed in expiry order.
    timers.sort_by_key(|t| if t.expires_time != 0 { t.expires_time } else { u64::MAX });

    for cpu in 0..num_cpus {
        out.push_str(&format!("cpu: {}\n", cpu));
        for base in 0..8u32 {
            out.push_str(&format!(" clock {}:\n", base));
            out.push_str("  .base:       0000000000000000\n");
            out.push_str(&format!("  .index:      {}\n", base));
            out.push_str("  .resolution: 1 nsecs\n");
            out.push_str("  .offset:     0 nsecs\n");
            out.push_str("active timers:\n");
            if cpu != 0 || base != 0 {
                continue;
            }
            for (i, timer) in timers.iter().enumerate() {
                let name = if timer.tfd_addr != 0 {
                    "timerfd"
                } else if timer.signo != 0 {
                    "alarm"
                } else if timer.wake_pid != 0 {
                    "wake"
                } else {
                    "oneshot"
                };
                out.push_str(&format!(" #{}: <{:016x}>, {}, S:01\n", i, timer.id, name));

                // The expiry, in nanoseconds. A jiffy-only timer has
                // no high-resolution deadline; it is reported at the
                // jiffy grid (KERNEL_HZ = 100).
                let expires_ns = if timer.expires_time != 0 {
                    timer.expires_time * TICKS_TO_NS
                } else {
                    timer.expires * 1_000_000_000 / crate::config::KERNEL_HZ as u64
                };
                let in_ns = expires_ns.saturating_sub(now_ns);
                out.push_str(&format!(
                    " # expires at {}-{} nsecs [in {} to {} nsecs]\n",
                    expires_ns, expires_ns, in_ns, in_ns
                ));
            }
        }
    }

    out.into_bytes()
}
