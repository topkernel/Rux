# Global RunQueue Scheduler Design

## 1. Motivation

Current implementation: each CPU has an independent `RunQueue` containing CFS/RT/DL queues, plus cross-CPU load balancing (steal, migrate) logic. Problems:

1. **Complexity**: load_balance / steal_task / find_busiest_cpu logic is difficult to maintain
2. **Double counting**: tasks exist in both per-class queue and legacy `tasks[]` array, counters easily inconsistent
3. **SMP safety**: cross-CPU RQ lock ordering (AB-BA deadlock prevention), per-CPU init synchronization
4. **Load imbalance**: tasks may pile up on one CPU while others are idle, relying on periodic rebalancing

New approach: **Global per-class RQ** + minimal per-CPU state. All runnable tasks of the same scheduling class share a single global queue. CPUs pull tasks on demand.

## 2. Architecture Overview

```
┌──────────────────────────────────────────────────┐
│                GlobalRunQueue                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐       │
│  │ DlRunQueue│  │ RtRunQueue│  │ CfsRunQueue│     │
│  │ (BTreeMap)│  │(bitmap+  │  │(BTreeMap) │      │
│  │  EDF)     │  │ lists)   │  │  vruntime)│      │
│  └──────────┘  └──────────┘  └──────────┘       │
│         Spinlock lock;                           │
│         AtomicUsize nr_running;                  │
└──────────────────────────────────────────────────┘

┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐
│  CPU 0     │  │  CPU 1     │  │  CPU 2     │  │  CPU 3     │
│ current ───┤  │ current ───┤  │ current ───┤  │ current ───┤
│ idle ──────┤  │ idle ──────┤  │ idle ──────┤  │ idle ──────┤
│ stop ──────┤  │ stop ──────┤  │ stop ──────┤  │ stop ──────┤
└────────────┘  └────────────┘  └────────────┘  └────────────┘
  (tp → Task)     (tp → Task)     (tp → Task)     (tp → Task)
```

### 2.1 Data Structures

```rust
/// Global run queue — one instance shared by all CPUs
pub struct GlobalRunQueue {
    /// Protects all three sub-queues
    lock: RawSpinlock,
    /// Deadline queue (EDF sorted by deadline)
    dl_rq: DlRunQueue,
    /// Real-time queue (bitmap + per-priority lists)
    rt_rq: RtRunQueue,
    /// CFS queue (BTreeMap sorted by vruntime)
    cfs_rq: CfsRunQueue,
    /// Total runnable task count (atomic for lock-free idle check)
    nr_running: AtomicUsize,
    /// Idle CPU bitmap: bit N = 1 means CPU N is idle
    idle_cpus: AtomicU32,
}

/// Per-CPU state — minimal, no queue
pub struct PerCpuState {
    /// Currently running task
    current: *mut Task,
    /// Per-CPU idle task (PID 0)
    idle: *mut Task,
    /// Per-CPU stop task (for hotplug)
    stop: *mut Task,
}
```

### 2.2 Static Layout

```rust
/// Single global instance
static GRQ: GlobalRunQueue = GlobalRunQueue::new();

/// Per-CPU state array (indexed by cpu_id)
static PER_CPU: [PerCpuState; MAX_CPUS] = [PerCpuState::new(); MAX_CPUS];
```

## 3. Task Selection (pick_next)

When `schedule()` is called on a CPU:

```
pick_next(this_cpu):
    // 1. Per-CPU stop task (no lock needed)
    if per_cpu[this_cpu].stop != null:
        return stop

    // 2. Lock global RQ
    grq.lock()

    // 3. Check in strict priority order
    task = dl_rq.pick_next_cpu(this_cpu)   // earliest deadline, cpu_allowed
    if task: goto found

    task = rt_rq.pick_next_cpu(this_cpu)   // highest priority, cpu_allowed
    if task: goto found

    task = cfs_rq.pick_next_cpu(this_cpu)  // min vruntime, cpu_allowed
    if task: goto found

    // 4. Nothing runnable → idle
    grq.unlock()
    mark_idle(this_cpu)
    return per_cpu[this_cpu].idle

found:
    per_cpu[this_cpu].current = task
    task.ti_cpu = this_cpu
    grq.unlock()
    return task
```

Key: `pick_next_cpu()` picks the best task **that can run on this CPU** (checks `cpus_allowed` mask). If the best candidate is not allowed, skip to next.

### 3.1 CPU Affinity in pick_next

Each class needs an affinity-aware pick:

| Class | Affinity Strategy |
|-------|-------------------|
| DL    | Scan leftmost entries until finding one with `cpu_allowed(this_cpu)` |
| RT    | Check highest priority list; if no allowed task at that priority, try next priority |
| CFS   | Scan from leftmost until finding `cpu_allowed(this_cpu)` task |

For systems with few CPUs (MAX_CPUS=4), a linear scan is acceptable. If a task can't run on this CPU, skip it and try the next one.

> **Open question**: Should we skip non-allowed tasks or migrate them? Recommendation: skip (pick allowed task). If no allowed task exists, CPU goes idle even if global RQ is non-empty.

## 4. CPU Selection (select_task_rq)

When a task is woken up (enqueue), decide **which CPU** it should target:

### 4.1 RT/DL Tasks (High Priority)

```
select_task_rq_rt_dl(task):
    cpus = task.cpus_allowed

    // 1. If target CPU is idle → wake it immediately
    for cpu in cpus:
        if is_idle(cpu):
            wake_cpu_via_ipi(cpu)
            return cpu

    // 2. Find running CPU with lowest RT priority
    //    (preempt lowest-priority running RT task)
    target = find_lowest_prio_rt_cpu(cpus)
    if target.valid:
        resched_cpu(target)
        return target

    // 3. Any CPU in cpus_allowed
    return first_set(cpus)
```

### 4.2 CFS Tasks (Normal)

```
select_task_rq_fair(task):
    cpus = task.cpus_allowed

    // 1. Prefer wake_cpu (set by waker, usually same CPU as waker)
    if task.wake_cpu in cpus && is_idle(task.wake_cpu):
        wake_cpu_via_ipi(task.wake_cpu)
        return task.wake_cpu

    // 2. Any idle CPU in cpus_allowed
    for cpu in cpus:
        if is_idle(cpu):
            wake_cpu_via_ipi(cpu)
            return cpu

    // 3. All CPUs busy → enqueue to global RQ
    //    Running CPU will pick it up on next schedule
    return -1  // no specific CPU, just enqueue
```

### 4.3 Idle CPU Wake-up

When enqueuing to global RQ and an idle CPU exists in `cpus_allowed`:

```
enqueue_task(task):
    grq.lock()
    enqueue to class-specific queue
    grq.nr_running += 1
    grq.unlock()

    // Try to wake an idle CPU
    cpus = task.cpus_allowed
    for cpu in cpus:
        if try_mark_nonidle(cpu):   // atomic CAS idle_cpus bit
            send_reschedule_ipi(cpu)
            return
```

The idle CPU receives the IPI, exits WFI, calls `schedule()`, locks global RQ, and picks up the task.

## 5. Scheduling Flow

### 5.1 schedule()

```
schedule():
    clear_need_resched()
    prev = per_cpu[this_cpu].current

    // Mark prev state
    if prev.state == RUNNING && prev.pid != 0:
        // Re-enqueue prev to global RQ (still runnable)
        enqueue_locked(prev)

    // Pick next
    next = pick_next(this_cpu)

    if next == prev:
        return  // no switch needed

    // Context switch (IRQs remain disabled)
    context_switch(prev, next)

    // After switch returns (in prev's context later):
    restore_irq(flags)
```

### 5.2 scheduler_tick()

```
scheduler_tick(this_cpu):
    current = per_cpu[this_cpu].current

    // Update runtime for current task
    match current.policy:
        Normal/Batch:
            cfs_update_curr(current)
            if time_slice_expired:
                set_need_resched()
        Fifo/Rr:
            if current.policy == Rr:
                tick_rt_timeslice(current)
                if timeslice_expired:
                    set_need_resched()
        Deadline:
            dl_update_curr(current)
            if throttled:
                set_need_resched()
```

Timer tick does **not** need the global RQ lock — it only examines/updates the currently running task. If preemption is needed, `set_need_resched()` triggers `schedule()` later.

### 5.3 cpu_idle_loop()

```
cpu_idle_loop():
    enable_timer_interrupt()  // for secondary CPUs
    loop:
        schedule()           // try to pick a task from global RQ

        // If still idle after schedule
        if is_idle(this_cpu):
            // Double-check: lock GRQ, see if anything available
            grq.lock()
            task = pick_next_class_queue(this_cpu)
            if task:
                grq.unlock()
                run(task)
                continue
            grq.unlock()

        // Nothing to run → WFI
        mark_idle(this_cpu)
        wfi()
        // Woken by IPI → back to top of loop, schedule() again
```

## 6. Lock Strategy

### 6.1 Single Lock Design

Use **one `RawSpinlock`** to protect all three class queues:

```rust
lock_irqsave() on GRQ.lock   →  protects dl_rq + rt_rq + cfs_rq + nr_running
```

Reasons:
- MAX_CPUS = 4, contention is minimal
- pick_next needs to check DL → RT → CFS atomically (single lock avoids priority inversion)
- Eliminates class-lock ordering concerns
- Simpler code, fewer bugs

### 6.2 Lock-free Paths

| Operation | Needs GRQ lock? |
|-----------|-----------------|
| scheduler_tick (update runtime) | No — only touches current task |
| need_resched check | No — per-CPU atomic |
| context_switch | No — only prev/next registers |
| enqueue/dequeue | Yes |
| pick_next | Yes |
| load_balance | **Eliminated** — global RQ is inherently balanced |

### 6.3 Lock Duration Optimization

Keep the lock hold time short:
- In `pick_next`: lock → scan queues → pick → unlock
- In `enqueue`: lock → insert → unlock → then IPI (outside lock)
- Never hold lock across context_switch

## 7. Preemption & IPI

### 7.1 When to Send Reschedule IPI

| Event | Action |
|-------|--------|
| RT task enqueued | If any CPU is running a lower-prio RT task → `resched_cpu(target)` |
| DL task enqueued | If any CPU is running a later-deadline DL task → `resched_cpu(target)` |
| CFS task enqueued | If any idle CPU in cpus_allowed → `resched_cpu(idle_cpu)` |
| Timer tick preemption | `set_need_resched()` on local CPU only (no IPI) |

### 7.2 Cross-CPU Preemption for RT/DL

```
check_preempt_wakeup(task):
    if task is RT:
        for each cpu in task.cpus_allowed:
            running = per_cpu[cpu].current
            if running.policy in [Normal, Batch, Idle]:
                resched_cpu(cpu)  // RT preempts fair
                return
            if running.policy in [Fifo, Rr] && running.rt_priority > task.rt_priority:
                resched_cpu(cpu)  // higher-prio RT preempts lower
                return

    if task is DL:
        for each cpu in task.cpus_allowed:
            running = per_cpu[cpu].current
            if running.policy != Deadline:
                resched_cpu(cpu)
                return
            if running.dl_deadline > task.dl_deadline:
                resched_cpu(cpu)
                return
```

Reading `per_cpu[cpu].current` is racy but safe — worst case we send an unnecessary IPI.

## 8. CFS Fairness in Global RQ

With a single global CFS queue, fairness is naturally maintained:

- **Single vruntime domain**: all tasks share the same `min_vruntime`, no cross-CPU vruntime normalization needed
- **No migration penalty**: tasks don't lose vruntime position when "migrated" (there's no per-CPU queue to migrate from)
- **Natural load balancing**: the busiest CPU simply picks the next task from the same queue
- **sched_slice**: time slice calculation uses total nr_running across all CPUs, which is correct

### 8.1 min_vruntime Advancement

`min_vruntime` in the global CfsRunQueue tracks the leftmost task's vruntime. This is always correct because all CFS tasks are in one tree.

### 8.2 Waker Affinity

CFS still benefits from cache warmth. `select_task_rq_fair` should prefer:
1. Previous CPU (task was last running on) — if idle
2. Waker's CPU — cache warmth from shared data
3. Any idle CPU
4. Fallback: enqueue globally, any CPU picks it up

## 9. Comparison with Current Implementation

| Aspect | Current (per-CPU RQ) | New (global RQ) |
|--------|---------------------|-----------------|
| Data structures | 4 × RunQueue (each with CFS/RT/DL) | 1 × GlobalRunQueue |
| Locks | 4 per-CPU RQ locks | 1 global lock |
| Load balancing | Complex steal + migrate logic | Eliminated |
| CPU selection | Per-class select_task_rq | Simplified: idle-first |
| CFS fairness | Cross-CPU vruntime normalization | Single vruntime domain |
| Cache locality | Better (task stays on one CPU) | Worse (task may migrate) |
| Scalability | Good for many CPUs | Good for few CPUs |
| Code complexity | High | Low |

## 10. Implementation Phases

### Phase 1: Data Structure Migration

**Files**: `kernel/src/sched/sched.rs`

- Replace `PER_CPU_RQ` with `GRQ` (global) + `PER_CPU` (per-CPU state)
- Remove `RunQueue` struct, keep `GlobalRunQueue` and `PerCpuState`
- Update `init()`, `init_secondary()`, `init_per_cpu_rq()`

### Phase 2: Core Scheduling

**Files**: `kernel/src/sched/sched.rs`

- Rewrite `schedule()` / `__schedule()` to use global RQ
- Rewrite `pick_next_task()` to scan global DL → RT → CFS
- Remove `load_balance()`, `steal_task()`, `find_busiest_cpu_unlocked()`
- Remove `enqueue_task_locked()`, `remove_from_legacy_queue()`
- Remove legacy `tasks[]` array and `nr_running` from per-CPU state

### Phase 3: Class Updates

**Files**: `kernel/src/sched/fair.rs`, `rt.rs`, `deadline.rs`

- Add `pick_next_cpu(cpu_id)` method to each class queue (affinity-aware)
- Update `SchedClass` trait methods to work with `GlobalRunQueue` instead of per-CPU `RunQueue`
- Or remove `SchedClass` trait entirely and use direct method calls (simpler)

### Phase 4: Wake-up & IPI

**Files**: `kernel/src/sched/sched.rs`, `kernel/src/arch/riscv64/ipi.rs`

- Implement `select_task_rq()` for global RQ
- Implement idle CPU bitmap tracking (`idle_cpus`)
- Update `enqueue_task()` to wake idle CPUs via IPI
- Implement cross-CPU RT/DL preemption

### Phase 5: Cleanup

- Remove dead code (legacy queue, load balance, steal)
- Update `slab_stats()`, `for_each_task()`, `rq_load()`
- Update `scheduler_tick()` to not need RQ lock
- Test: boot + SMP + shell + toybox commands

## 11. Open Questions

1. **Idle CPU bitmap race**: `idle_cpus` bitmap is set before WFI and cleared after schedule(). There's a window where CPU is marked idle but hasn't entered WFI yet. An IPI at that point is lost. Solution: after WFI returns, re-check global RQ (loop back to schedule()).

2. **RT/DL pick_next_cpu scan cost**: If the highest-priority RT task can't run on this CPU, we must scan lower priorities. For 100 priority levels this is still O(1) via bitmap tricks, but needs careful implementation.

3. **SchedClass trait**: The trait currently takes `RunQueueRef` (= `*mut RunQueue`). With global RQ, the trait signature changes. Should we keep the trait or simplify to direct function calls?

4. **Per-CPU page (PCP)**: `mm::init_percpu_pages` is called during sched init. This is unrelated to the scheduler refactor but needs to be preserved.
