//! Process-tree invariant validator (dfx=treehunt).
//!
//! Leg-29A root-cause hunt for the E8-REPAR "unlinked snapshot" family:
//! sc304's reparent Oops walked a LIVE task's ancestor chain into an
//! already-released Task whose `signal` slot held recycled garbage
//! (0x7ff08b80f820 — user-range). The defensive range check (740ca427)
//! stopped the fault, but WHO leaves a live chain node pointing at a
//! released task stayed open. Prime suspect by code shape:
//! `thread_group_join` copies the leader's parent into every member as
//! an UNLINKED snapshot and NOTHING maintains it — when the leader is
//! reparented (move_child), every member's snapshot goes stale forever;
//! the member's later exit walks `release_task`'s parent_ptr() through
//! freed memory (remove_child on a recycled parent) and any future
//! chain consumer reads garbage.
//!
//! This validator runs ~1 Hz from the timer heartbeat and prints:
//!   TREE-PARENT-DEAD  a live task's parent pointer is implausible
//!                     (poisoned/freed/recycled) — the sc304 class
//!                     caught in the act, named by pid.
//!   TREE-SNAP-STALE   a live member's parent snapshot differs from its
//!                     leader's maintained parent — the divergence
//!                     marker; the leader was moved, the member was not.
//!   TREE-UNLINKED     a leader-shaped task has parent set but is not
//!                     linked into any children list (head == null) —
//!                     the missed-reparent/double-add shape. (Known tiny
//!                     false window: a member between thread_group_leave
//!                     and ZOMBIE-set shows this shape for one tick.)
//!
//! Skips dying tasks (ZOMBIE/DEAD states legitimately diverge during
//! teardown). Reads are racy-by-design diagnostics under the pid-hash
//! bucket lock (for_each_task contract); no PROCESS_TREE_LOCK nesting.
//! Per-(pid,kind) dedupe ring plus a 64-lines/60-s cap; zero cost when
//! dfx=treehunt is off.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static LINES: AtomicU32 = AtomicU32::new(0);
static WINDOW_START: AtomicU64 = AtomicU64::new(0);

/// Dedupe ring: (pid | kind<<24) keys seen in the current window.
static SEEN: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

pub fn tick() {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::TreeHunt) {
        return;
    }
    // Rate window: 64 lines per 60 s.
    let now = crate::drivers::timer::get_jiffies() as u64;
    let w0 = WINDOW_START.load(Ordering::Relaxed);
    if now.wrapping_sub(w0) >= 6000 {
        if WINDOW_START
            .compare_exchange(w0, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            LINES.store(0, Ordering::Relaxed);
            for s in SEEN.iter() {
                s.store(0, Ordering::Relaxed);
            }
        }
    }
    if LINES.load(Ordering::Relaxed) >= 64 {
        return;
    }

    crate::sched::for_each_task(|task| {
        // SAFETY: callback runs under the pid-hash bucket lock; reads of
        // the task's own fields and diagnostic reads of plausible peer
        // pointers only; no writes except the diagnostic counters.
        unsafe {
            let t = &*task;
            let pid = t.pid();
            if pid == 0 {
                return;
            }
            let state = t.state();
            if state.is_dead() || state.contains(crate::process::task::TaskState::ZOMBIE) {
                return;
            }
            let parent = t.parent_ptr();
            let leader = t.group_leader_ptr();
            let is_member = leader as *const _ != task as *const _;

            // Class 1: parent pointer names implausible memory.
            if let Some(p) = parent {
                if !crate::process::task::Task::is_plausible_task_ptr(p) {
                    // Shape the holder and the target page so the serial
                    // line separates freed-Task-page (a real missed
                    // unlink) from recycled-garbage (long-stale pointer)
                    // from never-a-task addresses (pointer corruption).
                    let member = is_member as u64;
                    let lpid = if is_member { (*leader).pid() as u64 } else { 0 };
                    // Read the target's first word ONLY inside the
                    // kernel linear map (sc304 lesson: the implausible
                    // pointer may be user-range — reading it would fault
                    // in timer context). Outside the window, mark 0xBAD.
                    const MAP_BASE: usize = crate::arch::mm::memory_layout::PAGE_OFFSET;
                    const MAP_SPAN: usize = 0x10_0000_0000; // 64 GiB
                    let pa = p as usize;
                    let w0 = if pa >= MAP_BASE && pa + 8 < MAP_BASE + MAP_SPAN {
                        // SAFETY: diagnostic read of one word from a
                        // kernel-linear-map address — mapped for the
                        // machine's whole RAM, cannot fault.
                        unsafe { core::ptr::read_volatile(pa as *const u64) }
                    } else {
                        0xBAD_BAD_BAD_BAD_BAD
                    };
                    hit3(pid, p as usize, member, lpid, w0, t.state().bits() as u64);
                    return;
                }
            }

            // Class 2: member snapshot diverged from the leader's parent.
            if is_member
                && crate::process::task::Task::is_plausible_task_ptr(leader)
            {
                let l = &*leader;
                let lst = l.state();
                if !lst.is_dead() && !lst.contains(crate::process::task::TaskState::ZOMBIE) {
                    let lp = l.parent_ptr();
                    let same = match (parent, lp) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a as *const _ == b as *const _,
                        _ => false,
                    };
                    if !same {
                        hit2(
                            pid,
                            parent.map(|p| p as usize).unwrap_or(0),
                            l.pid(),
                            lp.map(|p| p as usize).unwrap_or(0),
                        );
                        return;
                    }
                }
            }

            // Class 3: parent set but not linked (leader-shaped tasks —
            // members carry head == null by design, skip them).
            if parent.is_some()
                && !is_member
                && t.parent_children_head_ptr().is_null()
            {
                hit(pid, 3, parent.map(|p| p as usize).unwrap_or(0), 0);
            }
        }
    });
}

/// Class-1 print: holder shape + the target page's first word (poison
/// magic = freed Task awaiting reuse; other kernel data = recycled;
/// user-range = corruption).
fn hit3(pid: u32, ptr: usize, member: u64, lpid: u64, w0: u64, state: u64) {
    let key = (pid as u64) | (1u64 << 24);
    let slot = &SEEN[(key as usize) % SEEN.len()];
    if slot.load(Ordering::Relaxed) == key {
        return;
    }
    slot.store(key, Ordering::Relaxed);
    if LINES.fetch_add(1, Ordering::Relaxed) >= 64 {
        return;
    }
    use core::fmt::Write;
    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
    let _ = writeln!(
        w,
        "TREE-PARENT-DEAD pid={} member={} leader-pid={} state={:#x} parent={:#x} target-w0={:#x}",
        pid, member, lpid, state, ptr, w0
    );
}

/// Class-2 print: both pointers (the stale member snapshot AND the
/// leader's maintained parent — whether the stale side is already
/// freed/recycled is the root-cause discriminator).
fn hit2(pid: u32, stale: usize, leader_pid: u32, leader_parent: usize) {
    let key = (pid as u64) | (2u64 << 24) | ((leader_pid as u64) << 32);
    let slot = &SEEN[(key as usize) % SEEN.len()];
    if slot.load(Ordering::Relaxed) == key {
        return;
    }
    slot.store(key, Ordering::Relaxed);
    if LINES.fetch_add(1, Ordering::Relaxed) >= 64 {
        return;
    }
    use core::fmt::Write;
    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
    let _ = writeln!(
        w,
        "TREE-SNAP-STALE pid={} stale-parent={:#x} leader-pid={} leader-parent={:#x}",
        pid, stale, leader_pid, leader_parent
    );
}

/// Print one hit line (dedupe + global cap); `aux` is the leader pid for
/// class 2, else 0.
fn hit(pid: u32, kind: u32, ptr: usize, aux: u32) {
    let key = (pid as u64) | ((kind as u64) << 24) | (aux as u64) << 32;
    let slot = &SEEN[(key as usize) % SEEN.len()];
    if slot.load(Ordering::Relaxed) == key {
        return;
    }
    slot.store(key, Ordering::Relaxed);
    if LINES.fetch_add(1, Ordering::Relaxed) >= 64 {
        return;
    }
    use core::fmt::Write;
    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
    let name = match kind {
        1 => "TREE-PARENT-DEAD",
        2 => "TREE-SNAP-STALE",
        _ => "TREE-UNLINKED",
    };
    let _ = if kind == 2 {
        writeln!(
            w,
            "{} pid={} parent={:#x} leader-pid={} (leader parent diverged)",
            name, pid, ptr, aux
        )
    } else {
        writeln!(w, "{} pid={} parent={:#x}", name, pid, ptr)
    };
}
