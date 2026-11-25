//! The RT class (S1b sole copy): the RT policy over the framework's
//! RtRunQueue storage; the pick core is a FREE fn here.


#![forbid(unsafe_code)]
use core::sync::atomic::{AtomicU64, AtomicU32, AtomicBool, Ordering};
use crate::process::task::{Task, SchedPolicy};
use crate::sched::{hotpath::SchedClass, types::RunQueueRef};
use crate::list::ListHead;
use crate::sched::{types::RtRunQueue, types::SchedRtEntity, types::MAX_RT_PRIO, hotpath::RR_TIMESLICE_TICKS};
use crate::config::MAX_CPUS;

impl RtSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for RtSchedClass {
    fn name(&self) -> &'static str {
        "rt"
    }

    /// Actual enqueue happens in sched.rs::enqueue_task_locked.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual dequeue happens in sched.rs::dequeue_task.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        false
    }

    /// RT tasks don't yield to lower priority tasks
    fn yield_task(&self, _rq: RunQueueRef) {
    }

    /// Preemption checks happen in sched.rs::check_rt_preempt.
    fn wakeup_preempt(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual pick happens in sched.rs::pick_next_task.
    fn pick_next_task(&self, _rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        core::ptr::null_mut()
    }

    /// Actual re-enqueue happens in sched.rs::__schedule.
    fn put_prev_task(&self, _rq: RunQueueRef, _prev: *mut Task, _next: *mut Task) {
    }

    /// Minimal — actual setup happens in sched.rs.
    fn set_next_task(&self, _rq: RunQueueRef, _next: *mut Task, _first: bool) {
    }

    /// RT load balancing is done via push/pull operations
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        false
    }

    /// CPU selection — simplified for global RQ.
    fn select_task_rq(&self, _task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        cpu
    }

    /// Actual tick handling happens in sched.rs::scheduler_tick.
    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
    }

    /// RT doesn't track vruntime
    fn update_curr(&self, _rq: RunQueueRef) {
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, task: *mut Task) -> u32 {
        if task.is_null() {
            return 0;
        }

        // SAFETY: task is a valid pointer from the scheduler; null check above.
        {
            if crate::sched::r#unsafe::task_shared(task).policy() == SchedPolicy::Rr {
                RR_TIMESLICE_TICKS
            } else {
                0 // SCHED_FIFO has no time slice
            }
        }
    }

    /// Actual check happens in sched.rs via GlobalRunQueue::rt_rq.
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        false
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&crate::fair::FAIR_SCHED_CLASS)
    }
}

/// Global RT scheduling class instance
pub static RT_SCHED_CLASS: RtSchedClass = RtSchedClass::new();


/// The RT scheduling class singleton type.
pub struct RtSchedClass;

/// The RT pick core (S1b): a FREE fn over the framework storage.
pub fn rt_pick_next_cpu(rq: &mut crate::sched::types::RtRunQueue, cpu_id: usize,
        prev: *mut Task,
        skipped_owned_owner: &mut usize,) -> Option<*mut Task> {
        // Iterate bitmap words
        for word_idx in 0..2 {
            let word = rq.bitmap[word_idx].load(Ordering::Acquire);
            if word == 0 {
                continue;
            }

            // Iterate set bits in this word
            let mut remaining = word;
            while remaining != 0 {
                let bit = remaining.trailing_zeros() as usize;
                let prio = word_idx * 64 + bit;

                // Walk the list at this priority
                let head = &rq.queue[prio] as *const _ as *mut ListHead;
                let mut pos = rq.queue[prio].next;

                while pos != head {
                    // b50 #GP guard: a scribbled link yields a
                    // non-canonical ListHead; derefing it (next-loads or
                    // the container_of below) is a kernel #GP. The node
                    // cannot be unlinked safely either, so stop walking
                    // this priority for this pick and report — the
                    // entries past the break stay linked and skip-picked,
                    // which keeps the kernel alive instead of fatal.
                    // b61/b62 hardening: canonical alone also accepts
                    // kernel-NEGATIVE poison (0xfffffffffffffeXX read from
                    // a freed task's links) — require the PHYSMAP range
                    // for the link, where Task allocations actually live.
                    // b69 fix: the DERIVED task screen must also accept
                    // the static Task storages (boot init task, idle
                    // tasks) — a physmap-only check dropped PID 1 from
                    // the CFS timeline at its first pick and wedged the
                    // machine in WFI with nr_running==0. The pos node
                    // itself stays physmap-or-static too (it is the
                    // rt_run_list INSIDE the same Task allocation).
                    if pos as usize % 8 != 0
                        || !crate::sched::hotpath::kernel_task_ptr_range_ok(pos as *const u8)
                    {
                        crate::sched::hotpath::report_poisoned_rq_entry(cpu_id, pos as usize);
                        break;
                    }
                    // SAFETY: pos is an embedded ListHead in an enqueued Task; offset_of gives the correct back-pointer.
                    let task = crate::sched::r#unsafe::task_of_rt_list(pos);
                    if !crate::sched::hotpath::kernel_task_ptr_range_ok(task as *const u8) {
                        crate::sched::hotpath::report_poisoned_rq_entry(cpu_id, task as usize);
                        break;
                    }

                    // SAFETY: task derived from queue above; cpu_allowed is a simple field read.
                    // R8-1b: skip tasks whose context is not yet saved (a
                    // pick would resume a STALE thread.sp — NEW2 root
                    // cause), except the switching CPU's own prev (fast
                    // path: next == prev early-returns without a restore).
                    // R65: dual-bit ownership (on_cpu AND running_on_cpu
                    // claim must both be free) — see the fair-class twin.
                    // cgroup v2 (U1b): skip cpu-throttled cgroup chains —
                    // see the fair-class twin for the unthrottle story.
                    let allowed = {
                        // R65-pick (probe-slow): single-qword ownership
                        // read — see the fair-class twin.
                        let ownership_ok =
                            task == prev || crate::sched::r#unsafe::task_shared(task).ownership_pickable(cpu_id);
                        // timer-gap lane: an AFFINITY-ALLOWED task refused
                        // only for ownership is mid-switch-OUT on its
                        // departing cpu (the sched_setaffinity
                        // self-migration handoff) — the release store
                        // lands within microseconds. Name the owner so
                        // the caller can re-arm NEED_RESCHED and retry
                        // instead of consuming the migration poke.
                        if !ownership_ok && task != prev && crate::sched::r#unsafe::task_shared(task).cpu_allowed(cpu_id) {
                            let claim =
                                crate::sched::r#unsafe::task_shared(task).running_on_cpu.load(Ordering::Relaxed);
                            if claim >= 0 {
                                *skipped_owned_owner = claim as usize;
                            }
                        }
                        crate::sched::r#unsafe::task_shared(task).cpu_allowed(cpu_id)
                            && ownership_ok
                            && !crate::sched::hotpath::task_cgroup_throttled(task)
                    };

                    // Save next before potential dequeue
                    // SAFETY: pos is a valid ListHead pointer from the queue; next is saved before dequeue mutates the list.
                    let next_pos = crate::sched::r#unsafe::list_next(pos);

                    if allowed {
                        // Dequeue this task
                        rq.dequeue(task);
                        return Some(task);
                    }

                    // RT-2X item-7 hunt: a pinned-RT wake wedge shows the
                    // owning CPU spinning in its idle loop with runnable RT
                    // tasks — name the refusing guard live (cmdline
                    // rt2xdbg=1, 48-line PER-CPU budget — timer-gap lane:
                    // the single global budget let cpu1's affinity skips
                    // eat every line, hiding cpu0's ownership skips).
                    if !allowed
                        && crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1")
                    {
                        static SKP: [AtomicU32; crate::config::MAX_CPUS] =
                            [const { AtomicU32::new(0) }; crate::config::MAX_CPUS];
                        if cpu_id < crate::config::MAX_CPUS
                            && SKP[cpu_id].fetch_add(1, Ordering::Relaxed) < 48
                        {
                            use crate::debug::taskdump::putc_brief as putchar;
                            fn ph(mut v: u64) {
                                for _ in 0..16 {
                                    let n = (v >> 60) as u8;
                                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                                    v <<= 4;
                                }
                            }
                            {
                                let tr = crate::sched::r#unsafe::task_shared(task);
                                let pid = tr.pid() as u64;
                                let mask = tr.cpus_allowed() as u64;
                                let oncpu = tr.on_cpu() as u64;
                                let claim = tr
                                    .running_on_cpu
                                    .load(Ordering::Relaxed) as u64;
                                let pk = crate::sched::r#unsafe::task_shared(task).pick_state() as u64;
                                let throt =
                                    crate::sched::hotpath::task_cgroup_throttled(task) as u64;
                                for &b in b"RT-PICK-SKIP pid=0x" { putchar(b); }
                                ph(pid);
                                for &b in b" jif=" { putchar(b); }
                                ph(crate::timer::get_jiffies());
                                for &b in b" cpu=" { putchar(b); }
                                ph(cpu_id as u64);
                                for &b in b" mask=0x" { putchar(b); }
                                ph(mask);
                                for &b in b" oncpu=" { putchar(b); }
                                ph(oncpu);
                                for &b in b" claim=" { putchar(b); }
                                ph(claim);
                                for &b in b" pk=" { putchar(b); }
                                ph(pk);
                                for &b in b" throttled=" { putchar(b); }
                                ph(throt);
                                putchar(b'\n');
                            }
                        }
                    }

                    pos = next_pos;
                }

                remaining &= !(1u64 << bit);
            }
        }

        None
    }

