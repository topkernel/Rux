//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! kswapd Kernel Thread
//!
//! Background page reclamation daemon, following mm/vmscan.c.
//! kswapd sleeps until a zone drops below its high watermark,
//! then runs balance_pgdat() to free pages.
//!
//! On UMA (single node) systems there is one kswapd thread.

use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use super::zone::{ZoneType, WMARK_HIGH};
use super::pglist::first_online_node_mut;
use super::vmscan::balance_pgdat;

// ============================================================================
// Static state
// ============================================================================

/// kswapd task pointer.  Set once during init.
static KSWAPD_TASK: AtomicPtr<crate::process::task::Task> = AtomicPtr::new(core::ptr::null_mut());

/// Wake flag — set by wakeup_kswapd(), cleared by kswapd loop.
static KSWAPD_WAKE: AtomicBool = AtomicBool::new(false);

/// The allocation order that triggered the most recent wakeup.
static KSWAPD_ORDER: AtomicUsize = AtomicUsize::new(0);

/// Whether kswapd has been initialized.
static KSWAPD_INIT: AtomicBool = AtomicBool::new(false);

/// Consecutive reclaim failures counter.
/// After KSWAPD_MAX_FAILURES consecutive failures, OOM killer is invoked.
static KSWAPD_CONSECUTIVE_FAILURES: AtomicUsize = AtomicUsize::new(0);
/// Maximum consecutive reclaim failures before triggering OOM killer.
const KSWAPD_MAX_FAILURES: usize = 16;
/// Whether the system has completed boot (set after shell starts).
/// OOM killer is not invoked before boot completes.
static OOM_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable OOM killer (called after boot completes).
pub fn enable_oom() {
    OOM_ENABLED.store(true, Ordering::Release);
}

// ============================================================================
// kswapd thread function
// ============================================================================

/// Main loop for kswapd.
///
/// Follows `kswapd()` in mm/vmscan.c:
///  1. Sleep until zone free pages < high watermark
///  2. Run balance_pgdat() to reclaim pages
///  3. Repeat
extern "C" fn kswapd_fn(_arg: *mut core::ffi::c_void) -> i32 {

    loop {
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // Clear wake flag before checking watermarks
        KSWAPD_WAKE.store(false, Ordering::Release);

        // Check if any zone is below high watermark
        let needs_reclaim = zone_below_high_watermark();

        if !needs_reclaim {
            // Nothing to do — sleep.
            if let Some(current) = crate::sched::current() {
                unsafe {
                    (*current).set_state(
                        crate::process::task::TaskState::new(
                            crate::process::task::TaskState::INTERRUPTIBLE,
                        ),
                    );
                }
            }
            crate::sched::schedule();
            continue;
        }

        // Reclaim pages at the requested order
        let order = KSWAPD_ORDER.load(Ordering::Relaxed) as i32;
        let reclaimed = balance_pgdat(order);

        // OOM escalation: if reclaim fails repeatedly, invoke OOM killer.
        // Following Linux's kswapd behavior where OOM is triggered when
        // reclaim cannot free enough pages after multiple attempts.
        // Guard: don't trigger OOM during early boot.
        if reclaimed == 0 && OOM_ENABLED.load(Ordering::Acquire) {
            let failures = KSWAPD_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
            if failures >= KSWAPD_MAX_FAILURES {
                // Reset counter and trigger OOM
                KSWAPD_CONSECUTIVE_FAILURES.store(0, Ordering::Relaxed);

                // DEEP PASS AT THE ESCALATION POINT (stress-fleet s18,
                // leg 51 — the kswapd leg of the 8.10.19 deep-drain
                // design). The 16 failed passes above each drained at
                // most ~32 pages against a referenced wall; the s18
                // evidence (OOM-FORENSIC #33) shows kills firing from
                // here while 172 MB of clean file cache sat one list
                // away, with ZERO OOM-DEEP-PASS lines — the page_alloc
                // slowpath's deep drain (39a515a5) never ran because
                // this escalation outran it (or the victim was the
                // task inside the slowpath itself). Before killing, run
                // the SAME deep drain the slowpath runs before its
                // `return 0`: chunked shrink past the wall + bulk
                // active-list demotion. Same context class as kswapd's
                // own balance_pgdat → shrink_reclaim (task context, no
                // spinlocks, no PteLock; the IN_DIRECT_RECLAIM gate
                // serializes DIRECT reclaimers only and must NOT be
                // waited on here — kswapd draining concurrently with a
                // direct reclaimer is the pre-existing balance_pgdat
                // concurrency shape).
                //
                // Escalation-gated by construction: this branch runs
                // only after 16 consecutive zero-reclaim passes, which
                // standard boots never produce — the DEEP_PASS counters
                // stay zero on the Tier-1 census (drct=0 discipline).
                let mut deep_freed = 0usize;
                if let Some(node) = unsafe { first_online_node_mut() } {
                    for zt in [
                        super::zone::ZoneType::ZoneNormal,
                        super::zone::ZoneType::ZoneDma32,
                        super::zone::ZoneType::ZoneDma,
                    ] {
                        if let Some(zone) = node.zone(zt) {
                            if !zone.is_initialized() {
                                continue;
                            }
                            // Same sizing contract as the page_alloc leg:
                            // the zone's WMARK_LOW shortfall (plus the
                            // request), clamped to the drain's bounds.
                            let wmark_low =
                                zone.watermark(super::zone::WMARK_LOW);
                            let shortfall = wmark_low
                                .saturating_add(1usize << order.max(0))
                                .saturating_sub(zone.nr_free());
                            let deep_target = shortfall.clamp(32, 8192);
                            deep_freed +=
                                super::vmscan::drain_file_cache_before_oom(
                                    deep_target,
                                );
                            break; // one zone's shortfall sizes the drain
                        }
                    }
                }
                if deep_freed > 0 {
                    // The escalation was premature: real pages were
                    // one wall away and the drain just freed them. The
                    // failure counter is already reset above; skip the
                    // kill on this wake and let the loop re-check
                    // watermarks against the freed pages. A genuinely
                    // dry pool returns 0 here (verified by the drain's
                    // own break-on-no-progress) and falls through to
                    // the OOM below.
                    if let Some(current) = crate::sched::current() {
                        unsafe {
                            (*current).set_state(
                                crate::process::task::TaskState::new(
                                    crate::process::task::TaskState::INTERRUPTIBLE,
                                ),
                            );
                        }
                    }
                    crate::sched::schedule();
                    continue;
                }

                // SAFETY: kswapd is the only reclaim thread; exclusive node access.
                if let Some(node) = unsafe { first_online_node_mut() } {
                    // Find total pages from any initialized zone
                    let mut totalpages = 0u64;
                    for zt in [super::zone::ZoneType::ZoneNormal, super::zone::ZoneType::ZoneDma32, super::zone::ZoneType::ZoneDma] {
                        if let Some(zone) = node.zone(zt) {
                            if zone.is_initialized() {
                                totalpages = zone.managed_pages() as u64;
                                break;
                            }
                        }
                    }
                    if totalpages > 0 {
                        let mut oc = crate::mm::oom_kill::OomControl::new(
                            totalpages, 0, order as u32,
                        );
                        crate::mm::oom_kill::out_of_memory(&mut oc);
                    }
                }
            }
        } else if reclaimed > 0 {
            // Reset failure counter on successful reclaim
            KSWAPD_CONSECUTIVE_FAILURES.store(0, Ordering::Relaxed);
        }

        // Always yield after a reclaim pass to avoid starving user processes.
        // Without a working reclaim path (try_to_unmap), kswapd may loop
        // indefinitely if the zone remains below watermarks.
        if let Some(current) = crate::sched::current() {
            unsafe {
                (*current).set_state(
                    crate::process::task::TaskState::new(
                        crate::process::task::TaskState::INTERRUPTIBLE,
                    ),
                );
            }
        }
        crate::sched::schedule();
    }

    0
}

// ============================================================================
// Watermark check
// ============================================================================

/// Returns true if any zone's free pages are below the high watermark.
fn zone_below_high_watermark() -> bool {
    // SAFETY: kswapd watermark check — exclusive node access from kswapd thread.
    let node = match unsafe { first_online_node_mut() } {
        Some(n) => n,
        None => return false,
    };

    for zone_type in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma] {
        if let Some(zone) = node.zone(zone_type) {
            if zone.is_initialized() && !zone.watermark_ok(0, WMARK_HIGH) {
                return true;
            }
        }
    }

    false
}

// ============================================================================
// Wakeup (called from page allocator)
// ============================================================================

/// Wake kswapd to begin reclaiming.
///
/// Called from `alloc_pages()` when free pages drop below WMARK_LOW.
pub fn wakeup_kswapd(order: i32) {
    if !KSWAPD_INIT.load(Ordering::Acquire) {
        return;
    }

    KSWAPD_ORDER.store(order.max(0) as usize, Ordering::Relaxed);

    // Avoid redundant wakeups
    if KSWAPD_WAKE.swap(true, Ordering::AcqRel) {
        return;
    }

    unsafe {
        let task_ptr = KSWAPD_TASK.load(Ordering::Acquire);
        if !task_ptr.is_null() {
            crate::process::task::Task::wake_up(task_ptr);
        }
    }
}

// ============================================================================
// Initialization
// ============================================================================

/// Create the kswapd kernel thread.
///
/// Must be called after sched::init() since it uses kthread_run().
pub fn init() {
    let task = crate::process::kthread::kthread_run(
        kswapd_fn,
        core::ptr::null_mut(),
        "kswapd",
    );

    if let Some(t) = task {
        let t_ptr = t as *mut _;
        KSWAPD_TASK.store(t_ptr, Ordering::Release);
        KSWAPD_INIT.store(true, Ordering::Release);
    } else {
        crate::pr_err!("kswapd: failed to create thread");
    }
}
