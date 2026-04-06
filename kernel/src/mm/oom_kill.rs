//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! OOM Killer
//!
//! Following mm/oom_kill.c: when the system runs out of memory and
//! reclaim fails, the OOM killer selects the "worst" process (highest
//! memory usage) and sends it SIGKILL to free its pages.
//!
//! Scoring follows Linux's oom_badness():
//!   points = mm->total_vm + oom_score_adj * totalpages / 1000
//!
//! The process with the highest score is killed.

use core::sync::atomic::Ordering;

use crate::process::task::{Task, TaskState, TIF_MEMDIE};
use crate::mm::mm_struct::MmFlags;

// ==================== Constants ====================

/// OOM score adjustment: immune from OOM kill.
/// Following Linux's OOM_SCORE_ADJ_MIN in include/uapi/linux/oom.h.
pub const OOM_SCORE_ADJ_MIN: i32 = -1000;

/// OOM score adjustment: maximum priority to be killed.
/// Following Linux's OOM_SCORE_ADJ_MAX in include/uapi/linux/oom.h.
pub const OOM_SCORE_ADJ_MAX: i32 = 1000;

// ==================== OOM Control ====================

/// OOM control structure, following struct oom_control in Linux.
///
/// Passed through the OOM kill decision chain:
/// out_of_memory() -> select_bad_process() -> oom_kill_process().
pub struct OomControl {
    /// Selected victim task pointer
    pub chosen: Option<*mut Task>,
    /// OOM badness score of the selected victim
    pub chosen_points: u64,
    /// Total managed pages in the system (for score scaling)
    pub totalpages: u64,
    /// GFP mask of the allocation that triggered OOM
    pub gfp_mask: u32,
    /// Order of the allocation that triggered OOM
    pub order: u32,
}

impl OomControl {
    /// Create a new OomControl for the given system parameters.
    pub fn new(totalpages: u64, gfp_mask: u32, order: u32) -> Self {
        Self {
            chosen: None,
            chosen_points: 0,
            totalpages,
            gfp_mask,
            order,
        }
    }
}

// ==================== oom_badness ====================

/// Compute the OOM badness score for a task.
///
/// Following Linux's oom_badness() in mm/oom_kill.c:
/// - Skip kernel threads (no address space)
/// - Skip init (PID 1)
/// - Skip tasks with MMF_OOM_DISABLE
/// - Skip tasks with oom_score_adj == OOM_SCORE_ADJ_MIN
/// - Baseline: mm.total_vm (virtual pages, upper bound for RSS)
/// - Adjustment: oom_score_adj * totalpages / 1000
///
/// Higher score = more likely to be killed.
pub fn oom_badness(task: &Task, totalpages: u64) -> u64 {
    // Skip kernel threads (no address space)
    let mm = match task.address_space() {
        Some(mm) => mm,
        None => return 0,
    };

    // Skip if MMF_OOM_DISABLE is set
    if mm.has_flag(MmFlags::MMF_OOM_DISABLE) {
        return 0;
    }

    // Get oom_score_adj (defaults to 0)
    let oom_score_adj = task.oom_score_adj();

    // If oom_score_adj == OOM_SCORE_ADJ_MIN, immune from OOM kill
    if oom_score_adj <= OOM_SCORE_ADJ_MIN {
        return 0;
    }

    // Baseline score: total virtual pages
    // Linux uses exact RSS, but Rux has no per-process RSS counter.
    // total_vm is an upper bound — acceptable for initial implementation.
    let mut points = mm.total_vm();

    // Scale adjustment: oom_score_adj * totalpages / 1000
    // This matches Linux's scaling to make adjustment proportional to system memory.
    if totalpages >= 1000 {
        let adj = (oom_score_adj as i64) * (totalpages as i64) / 1000;
        if adj >= 0 {
            points = points.saturating_add(adj as u64);
        } else {
            points = points.saturating_sub((-adj) as u64);
        }
    }

    points
}

// ==================== select_bad_process ====================

/// Select the worst process to kill under OOM conditions.
///
/// Following Linux's select_bad_process() in mm/oom_kill.c.
/// Iterates all tasks via PID hash table and picks the one with
/// the highest oom_badness() score.
fn select_bad_process(oc: &mut OomControl) {
    use crate::process::pid_hash::pid_hash_for_each_task;

    pid_hash_for_each_task(|task_ptr| unsafe {
        let task = &*task_ptr;

        // Skip kernel threads (no address space)
        if !task.has_address_space() {
            return;
        }

        // Skip PID 0 (idle)
        let pid = task.pid();
        if pid == 0 {
            return;
        }

        // Skip PID 1 (init) — never kill init
        if pid == 1 {
            return;
        }

        // Skip dead/zombie tasks
        let state = task.state();
        if state == TaskState::new(TaskState::ZOMBIE) || state == TaskState::new(TaskState::DEAD) {
            return;
        }

        // Skip tasks already marked as OOM victims
        if task.test_ti_flag(TIF_MEMDIE) {
            return;
        }

        let points = oom_badness(task, oc.totalpages);
        if points == 0 {
            return;
        }

        if points > oc.chosen_points {
            oc.chosen = Some(task_ptr);
            oc.chosen_points = points;
        }
    });
}

// ==================== oom_kill_process ====================

/// Kill the selected OOM victim.
///
/// Following Linux's __oom_kill_process() in mm/oom_kill.c:
/// 1. Send SIGKILL to victim
/// 2. Kill all processes sharing victim's mm (different thread groups)
/// 3. Set TIF_MEMDIE on victim (grants memory reserve access)
fn oom_kill_process(oc: &mut OomControl) {
    let victim = match oc.chosen {
        Some(t) => t,
        None => return,
    };

    unsafe {
        let victim_pid = (*victim).pid();
        let victim_name = (*victim).comm();
        let name_str = core::str::from_utf8(
            victim_name.split(|&b| b == 0).next().unwrap_or(b"?"),
        ).unwrap_or("?");

        crate::pr_err!(
            "oom-killer: Killed process {} ({}) score {}",
            victim_pid, name_str, oc.chosen_points
        );

        // Step 1: Send SIGKILL to victim
        let _ = crate::signal::send_signal(
            victim_pid,
            crate::signal::Signal::SIGKILL as i32,
        );

        // Step 2: Kill all processes sharing victim's mm (different thread groups).
        // Compare AddressSpace raw pointers to detect mm sharing.
        let victim_mm_ptr = (*victim).address_space()
            .map(|mm| mm as *const _ as usize);
        if let Some(mm_ptr) = victim_mm_ptr {
            crate::process::pid_hash::pid_hash_for_each_task(|task_ptr| {
                let t = &*task_ptr;
                if t.pid() == victim_pid {
                    return; // Skip the victim itself
                }
                let their_mm_ptr = t.address_space()
                    .map(|mm| mm as *const _ as usize);
                if let Some(their_ptr) = their_mm_ptr {
                    if their_ptr == mm_ptr {
                        // Same mm, different task — kill it too
                        let _ = crate::signal::send_signal(
                            t.pid(),
                            crate::signal::Signal::SIGKILL as i32,
                        );
                    }
                }
            });
        }

        // Step 3: Set TIF_MEMDIE on victim
        // This grants the victim access to memory reserves so it can exit cleanly.
        (*victim).set_ti_flag(TIF_MEMDIE);
    }
}

// ==================== out_of_memory ====================

/// Main OOM killer entry point.
///
/// Following Linux's out_of_memory() in mm/oom_kill.c:
/// 1. Log OOM event
/// 2. Select worst process via select_bad_process()
/// 3. Kill the selected victim via oom_kill_process()
///
/// Returns true if a victim was killed, false otherwise.
pub fn out_of_memory(oc: &mut OomControl) -> bool {
    crate::pr_err!(
        "oom: out of memory (order={}, gfp={:#x})",
        oc.order, oc.gfp_mask
    );

    // Select the process with the highest badness score
    select_bad_process(oc);

    if oc.chosen.is_some() {
        oom_kill_process(oc);
        true
    } else {
        crate::pr_err!("oom: no killable processes found");
        false
    }
}
