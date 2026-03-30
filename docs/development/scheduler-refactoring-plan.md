# Scheduler Refactoring Plan

Based on the analysis in [scheduler-analysis.md](scheduler-analysis.md), this document outlines the phased implementation plan to bring Rux's scheduler to functional SMP capability.

---

## Phase 1: Activate SMP (P0 — Foundation)

**Goal:** Make all 4 CPUs functional with basic scheduling and timer-driven preemption.

### 1.1 Wire Up Timer Tick

**Files:** `kernel/src/arch/riscv64/trap.rs`

**Current state:** `scheduler_tick()` and `schedule()` are commented out in the timer interrupt handler (trap.rs:235-241).

**Changes:**
- Uncomment `scheduler_tick()` call in timer handler
- Uncomment `schedule()` call with guard: `if need_resched() && !is_locked() { schedule(); }`
- `is_locked()` check ensures we don't schedule while holding the BKL
- `preempt_count == 0` check for nested interrupt safety

**Expected result:** Tasks are preempted based on time slice expiry. RT RR tasks get round-robin switching.

### 1.2 Secondary CPU Scheduler Entry

**Files:** `kernel/src/arch/riscv64/smp.rs`, `kernel/src/sched/sched.rs`

**Current state:** `secondary_cpu_start()` (smp.rs:83) enters infinite WFI loop. No idle task, no tp setup, no interrupts, no timer.

**Changes:**
1. **smp.rs:** Replace WFI loop with:
   ```
   secondary_cpu_start:
     mark_cpu_started(hart_id)
     wait for SMP_INIT_DONE barrier
     create per-CPU idle task
     set tp = idle task
     enable interrupts (sie CSR: STIE | SSIE)
     set up stimecmp for 10ms timer
     call cpu_idle_loop()
   ```
2. **sched.rs:** Add `init_secondary_cpu(cpu_id)` function:
   - Initialize per-CPU `PER_CPU_RQ[cpu_id]` if not already initialized
   - Create per-CPU idle task via `create_idle_task(cpu_id)`
   - Set idle task's `ti_cpu = cpu_id`

**Expected result:** All 4 CPUs enter the scheduler idle loop. Each CPU has its own runqueue and idle task.

### 1.3 CPU Affinity Infrastructure

**Files:** `kernel/src/process/task.rs`, `kernel/src/sched/sched.rs`

**Current state:** No `cpus_allowed` field in Task. `select_task_rq()` always returns current CPU.

**Changes:**
1. Define `Cpumask` type:
   ```rust
   pub struct Cpumask(pub u8);  // bit i set = CPU i allowed
   impl Cpumask { fn all() -> Self; fn cpu(cpu: usize) -> Self; fn test(&self, cpu: usize) -> bool; ... }
   ```
2. Add to Task struct:
   ```rust
   cpus_allowed: AtomicU8,   // Cpumask
   wake_cpu: AtomicI32,      // last CPU task ran on
   ```
3. Implement `task_set_cpus_allowed(task, mask)` and `task_cpus_allowed(task)`
4. Default `cpus_allowed = Cpumask::all()` for all new tasks

**Expected result:** CPU affinity mask can be set and queried. Foundation for `sched_setaffinity`/`sched_getaffinity` syscalls (future).

### 1.4 Fix IPI Infrastructure

**Files:** `kernel/src/arch/riscv64/ipi.rs`

**Current state:** `IpiType` enum defined with `Reschedule` and `Stop`, but type is never encoded. `handle_software_ipi()` assumes all software interrupts are reschedule.

**Changes:**
1. Encode IPI type using RISC-V `sip` CSR bits or a per-CPU shared variable
2. Add handler dispatch:
   ```rust
   pub fn handle_software_ipi(hart: usize) {
       let ipi_type = pending_ipi[hart].swap(0, AcqRel);
       if ipi_type & IPI_RESCHEDULE != 0 { set_need_resched(); schedule(); }
       if ipi_type & IPI_STOP != 0 { /* enter stop state */ }
   }
   ```
3. Prepare for future IPI types (TLB shootdown, call_function)

**Expected result:** IPI type is properly dispatched. Non-reschedule IPIs can be added.

---

## Phase 2: Task Migration & Load Balancing (P1)

**Goal:** Tasks can migrate between CPUs; load is distributed automatically.

### 2.1 Implement `select_task_rq` for CFS

**File:** `kernel/src/sched/fair.rs`

**Current state:** Always returns current CPU (fair.rs:904).

**Changes:**
1. Wake-affine heuristic: if waking from sleep and `wake_cpu` is idle, prefer `wake_cpu`
2. Fallback: find least-loaded CPU in `cpus_allowed`:
   ```rust
   fn select_task_rq_fair(task, cpu, flags) -> usize {
       let wake_cpu = task.wake_cpu.load(Acquire);
       if wake_cpu >= 0 && is_cpu_idle(wake_cpu) && task.cpus_allowed.test(wake_cpu) {
           return wake_cpu;
       }
       // fallback: least loaded CPU in cpus_allowed
       find_least_loaded_cpu(task.cpus_allowed)
   }
   ```
3. Call from `wake_up_process()` to place task on optimal CPU at wake-up time

### 2.2 Implement Task Migration

**File:** `kernel/src/sched/sched.rs` (NEW functions)

**New functions:**
1. `detach_task(src_cpu, task)`:
   - Lock source rq
   - Dequeue task from source rq (per-class dequeue)
   - Update `task.ti_cpu = dst_cpu`
   - Unlock source rq
2. `attach_task(dst_cpu, task)`:
   - Lock destination rq
   - Enqueue task on destination rq (per-class enqueue)
   - Set `NEED_RESCHED` on dst CPU
   - If dst CPU != current, send IPI
   - Unlock destination rq
3. `migrate_task(task, dst_cpu)`:
   - Lock ordering: lower CPU id first to prevent deadlock
   - `detach_task(src_cpu, task)` + `attach_task(dst_cpu, task)`

### 2.3 Implement Push/Pull Load Balancing

**File:** `kernel/src/sched/sched.rs`

**Changes to existing `load_balance()`:**
1. `push_task()`: when current CPU is overloaded (load > threshold):
   - Find least-loaded CPU within cpus_allowed
   - Migrate one task from current to target
   - Call from `scheduler_tick()` every N ticks
2. `pull_task()`: when current CPU is idle/underloaded:
   - Find most-loaded CPU with tasks that can migrate
   - Pull one task from source to current
   - Call from `cpu_idle_loop()` before WFI

### 2.4 Implement Sched Domain Topology

**File:** `kernel/src/sched/topology.rs` (NEW)

**New structures:**
```rust
pub struct SchedDomain {
    id: usize,
    level: usize,           // 0 = MC (multi-core), 1 = DIE, 2 = NUMA
    span: Cpumask,          // CPUs in this domain
    groups: Vec<SchedGroup>,
    flags: SdFlags,
}

pub struct SchedGroup {
    span: Cpumask,          // CPUs in this group
    group_weight: usize,    // number of CPUs
    asym_cap: bool,         // asymmetric capacity (big.LITTLE)
}
```

**For QEMU virt (4 CPUs):** Single domain with 1 group containing all 4 CPUs.

**New functions:**
1. `sched_domain_topology_init()` — create domain hierarchy based on CPU topology
2. `find_busiest_group(domain, this_cpu)` — find group with highest imbalance
3. `find_busiest_queue(group, this_cpu)` — find busiest rq in group
4. `can_migrate_task(task, dst_cpu)` — check cpus_allowed, cache affinity, migration cost

---

## Phase 3: RT & DL Enhancements (P1-P2)

**Goal:** RT and DL classes have proper load balancing and bandwidth control.

### 3.1 RT Load Balancing

**File:** `kernel/src/sched/rt.rs`

**Changes:**
1. `push_rt_task()`: when `rt_rq.overloaded == true`:
   - `find_lock_lowest_rq(p)` — find CPU in cpus_allowed with no RT tasks or lower RT priority
   - Double-lock rq (ordered by CPU id)
   - Move task via dequeue/enqueue
2. `pull_rt_task()`: when current CPU's rt_rq is empty:
   - Scan other CPUs for RT tasks
   - Pull highest-priority RT task from most-overloaded CPU
3. Set `overloaded` flag in `enqueue_task()` when `rt_nr_running > 1`
4. Call `push_rt_task()` from `enqueue_task_rt()` and `scheduler_tick()`

### 3.2 RT Throttling

**File:** `kernel/src/sched/rt.rs`

**New fields on `RtRunQueue`:**
```rust
rt_throttled: AtomicBool,
rt_time: AtomicU64,      // accumulated runtime this period
rt_runtime: u64,          // max runtime per period (default 950ms)
rt_period: u64,           // period length (default 1000ms)
```

**Changes:**
1. In `task_tick()`: track `rt_time += delta`, check `rt_time > rt_runtime` → throttle
2. When throttled: do not pick RT tasks, schedule CFS instead
3. Period timer: reset `rt_time = 0` at each period boundary

### 3.3 Fix DL `update_curr`

**File:** `kernel/src/sched/deadline.rs`

**Bug:** `update_curr()` hardcodes `exec_start = 0` (line 466), so runtime consumption is broken.

**Fix:** Add `exec_start: AtomicU64` to `SchedDlEntity`:
```rust
impl SchedDlEntity {
    fn save_exec_start(&self, now: u64) { self.exec_start.store(now, Release); }
    fn consume_runtime(&self, delta: u64) -> bool {
        let remaining = self.runtime.fetch_sub(delta as i64, AcqRel);
        if remaining - delta as i64 <= 0 { self.dl_throttled.store(true, Release); false }
        else { true }
    }
}
```

Update `set_next_task()` to save exec_start. Update `update_curr()` to use saved value.

### 3.4 DL Admission Control

**File:** `kernel/src/sched/deadline.rs`

**Changes:**
1. `dl_bw_overflow(task)`: check `running_bw + task_bw > DL_BW_MAX`
2. `dl_add_task_bw()` / `dl_sub_task_bw()`: maintain `running_bw` per rq
3. In `enqueue_task_dl()`: call `dl_bw_overflow()`, reject if exceeded (return -EBUSY)

---

## Phase 4: Advanced Features (P2-P3)

**Goal:** Remove BKL, implement NOHZ idle, CPU hotplug.

### 4.1 Replace BKL with Per-rq Locking

**Files:** `kernel/src/sched/sched.rs`, `kernel/src/sync/kernel_lock.rs`

This is the largest single change. Replace the global `KERNEL_LOCK` with per-CPU `rq->lock`:

1. Add `lock: Spinlock` to `RunQueue` struct
2. `rq_lock(rq)` / `rq_unlock(rq)` / `rq_lock_irqsave(rq)` helpers
3. Remove `KERNEL_LOCK` from trap entry/exit in `trap.S`
4. Update all `schedule()` callers to manage rq lock
5. Ensure syscalls release rq lock before returning to user

**Risk:** This is invasive. Must be done carefully to avoid deadlocks. Lock ordering: rq locks always taken in ascending CPU id order.

### 4.2 NOHZ Idle (Tickless)

**Files:** `kernel/src/sched/sched.rs`, `kernel/src/drivers/timer/riscv64.rs`

1. When CPU enters idle (only idle task runnable): stop timer (`stimecmp = u64::MAX`)
2. When task enqueues on idle CPU: restart timer
3. `nohz_idle_balance()`: periodically pull tasks from busy CPUs

### 4.3 CPU Hotplug (P3)

**Files:** `kernel/src/arch/riscv64/smp.rs`, `kernel/src/sched/sched.rs`

1. `cpu_down(cpu)`: migrate all tasks off, offline the CPU
2. `cpu_up(cpu)`: initialize rq, create idle task, enter scheduler
3. CPU hotplug notifier chain for other subsystems

---

## Per-Phase Workflow

Each phase follows this workflow:

1. **Write code** — implement the planned changes
2. **Build** — `make build`
3. **Smoke test** — 3 consecutive runs with cold reboot between each:
   ```bash
   echo -e "\n/test/smoke_test\nexit" | timeout 60 make run
   ```
4. **Update documentation** — update `docs/development/` and `docs/progress/roadmap.md`
5. **Wait for review** — no auto-commit; commit only after user approval

---

## Priority and Dependency Graph

```
Phase 1.1 (Timer Tick)  ─┐
Phase 1.2 (Secondary CPU)─┼── Phase 2 (Migration & LB) ── Phase 3 (RT/DL) ── Phase 4 (Advanced)
Phase 1.3 (CPU Affinity) ─┤
Phase 1.4 (IPI Fix)  ────┘
```

Phase 1 items can be done in any order but should be completed together before Phase 2.
Phase 3 depends on Phase 2 (migration infrastructure).
Phase 4 depends on all previous phases.

---

## Estimated Scope

| Phase | New/Modified Files | Complexity | Risk |
|---|---|---|---|
| Phase 1 | 4 files | Medium | Low — incremental, each step independently testable |
| Phase 2 | 3 files (1 new) | High | Medium — concurrent locking, migration race conditions |
| Phase 3 | 2 files | Medium | Low — additive features on top of Phase 2 |
| Phase 4 | 4+ files | Very High | High — BKL removal touches entire kernel |
