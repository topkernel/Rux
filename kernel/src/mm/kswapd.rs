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

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::zone::{ZoneType, WMARK_HIGH};
use super::pglist::first_online_node_mut;
use super::vmscan::balance_pgdat;

// ============================================================================
// Static state
// ============================================================================

/// kswapd task pointer.  Set once during init.
static mut KSWAPD_TASK: *mut crate::process::task::Task = core::ptr::null_mut();

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
    crate::pr_info!("kswapd started");

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

                if let Some(node) = first_online_node_mut() {
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
    let node = match first_online_node_mut() {
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
        let task_ptr = KSWAPD_TASK;
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
        unsafe {
            KSWAPD_TASK = t_ptr;
        }
        KSWAPD_INIT.store(true, Ordering::Release);
        crate::pr_info!("kswapd: thread created");
    } else {
        crate::pr_err!("kswapd: failed to create thread");
    }
}
