# Scheduler Analysis: Linux vs Rux

## Executive Summary

Rux has a scheduler framework modeled after Linux with the correct class hierarchy (Stop > Deadline > RT > Fair > Idle) and the `SchedClass` trait mirroring Linux's `sched_class`. However, most classes are skeletons with stub implementations. The most severe gaps are:

1. **Secondary CPUs never enter the scheduler** — they WFI-loop forever after boot
2. **Timer tick is not wired up** — `scheduler_tick()` exists but is never called (commented out in trap.rs)
3. **Load balancing is primitive** — only steals one task when current CPU load <= 1
4. **No CPU affinity, no task migration, no sched domains, no NOHZ**
5. **Big Kernel Lock serializes all kernel entry on all CPUs**

---

## 1. Core Scheduler (`kernel/src/sched/sched.rs`)

### `RunQueue` Structure

| Field | Rux | Linux (`struct rq`) |
|---|---|---|
| `cfs_rq: CfsRunQueue` | BTreeMap-based CFS queue | `struct cfs_rq` with rbtree |
| `rt: RtRunQueue` | Priority bitmap + lists | `struct rt_rq` with prio_array |
| `dl: DlRunQueue` | BTreeMap sorted by deadline | `struct dl_rq` with rbtree |
| `tasks: [*mut Task; 256]` | Legacy flat array | Not present in Linux |
| `current: *mut Task` | Running task | `rq->curr` |
| `nr_running: usize` | Task count | `rq->nr_running` |
| `idle: *mut Task` | Per-CPU idle task | `rq->idle` |
| `stop: *mut Task` | Stop task | `rq->stop` |
| Lock | `Mutex<RunQueue>` | `raw_spinlock_t rq->lock` |

**Missing from Linux's `struct rq`**: `clock`, `clock_task`, `cpu_load`, `nr_switches`, `nr_uninterruptible`, `calc_load_update`, `calc_load_active`, `cfs/rt/dl bandwidth`, `online`, `rd`, `sched_class` pointers, `balance_callback` list, `cpu_capacity`, `idle_balance`.

### `__schedule()` (line 460)

**Rux flow:**
1. Clear `NEED_RESCHED` flag
2. Acquire `Mutex<RunQueue>`
3. If `nr_running == 0`, attempt `load_balance()`
4. If CFS enabled and current is RUNNING, re-enqueue via `put_prev_task`
5. Call `pick_next_task()` — iterates class chain: stop > dl > rt > CFS > idle
6. Drop lock, disable interrupts, call `context_switch()`

**Linux `__schedule()` additions:**
- RCU read-side critical section (`rcu_note_context_switch`)
- `prev->on_rq` tracking (prev may stay on rq after switch)
- `rq->nr_switches` counter
- `balance_callback` processing
- `psi` (Pressure Stall Information) accounting
- `perf` event context switch hooks
- `rq->last_seen_update` for NOHZ

### `scheduler_tick()` (line 234)

**Rux:** Exists and is correct in logic — calls `current->sched_class.task_tick(rq)`. But **never called** because it is commented out in `kernel/src/arch/riscv64/trap.rs:235-241`.

**Linux:** Called from `tick_sched_timer()` (hrtimer), which runs every `HZ` jiffies. Handles NOHZ tick stop, updates rq clock, calls `task_tick()` per class.

### `context_switch()` (line 569)

**Rux:** Basic arch register save/restore via `switch_to()` assembly.

**Linux additions:**
- `mm_switch()` — address space switch (lazy TLB)
- `next->active_mm` tracking
- `mmdrop()` for old mm
- `switch_fpu()` — floating point state
- `perf_event_context_sched_out/in`
- `kvm_arch_vcpu_put/load`
- `uaccess_flush()` — speculative execution mitigation

### `load_balance()` (line 979)

**Rux:**
```rust
fn load_balance(rq: &mut RunQueue, cpu: usize) {
    if rq.nr_running > LOAD_IMBALANCE_THRESH { return; }  // only if underloaded
    let busiest = find_busiest_cpu(cpu);  // scan all CPUs, pick highest load
    if let Some(src_cpu) = busiest {
        steal_task(src_cpu, cpu);  // steal ONE task
    }
}
```

**Linux `sched_balance_rq()` (fair.c:11795):**
1. `sched_balance_find_src_group()` — find busiest sched_group within domain
2. `sched_balance_find_src_rq()` — find busiest rq within group
3. `load_balance_run()` — detach tasks (LBF_ALL_PINNED, can_migrate_task checks)
4. `attach_tasks()` — attach to dst rq
5. Multiple tasks moved per balance, up to `env->imbalance`
6. Per-class hooks: `load_balance_fair()` calculates load via `task_h_load()`

**Key gaps:**
- No `sched_domain` / `sched_group` hierarchy
- No `can_migrate_task()` (checks cpus_allowed, cache hot, migration cost)
- Only steals one task vs. Linux's imbalance-based multi-task migration
- No `active_balance` via stop task

### `resched_cpu()` (line 231)

**Rux:** Sets `NEED_RESCHED[cpu]`, sends IPI via `send_reschedule_ipi(cpu)`. Correct implementation.

**Linux additions:**
- `atomic_or(TIF_NEED_RESCHED)` on target task's `ti_flags`
- `CSD_TYPE_RESCHEDULE` via `smp_send_reschedule()`
- `WARN_ON(in_interrupt)` guard

### `cpu_idle_loop()` (line 1071)

**Rux:**
```rust
loop {
    schedule();
    if only_idle_running { load_balance(); schedule(); }
    wfi;
}
```

**Linux `cpu_idle_poll()` / `do_idle()`:**
- `cpuidle_idle_call()` — deep C-state selection
- `rcu_idle_enter/exit()` — RCU dyntick-idle
- `tick_nohz_idle_enter/exit()` — stop scheduler tick
- `idle_balance()` — pull tasks when going idle
- `local_irq_enable()` before WFI
- `play_dead()` for CPU hotplug

### `wake_up_process()` (line 255)

**Rux:** Sets task state to RUNNING, enqueues on current CPU's rq, calls `resched_curr()`.

**Linux `try_to_wake_up()` (core.c:2185):**
1. `p->pi_lock` — lock task
2. `select_task_rq(p, cpu, wake_flags)` — find best CPU (wake_affine)
3. `ttwu_stat()` — schedstat accounting
4. `ttwu_activate()` — set `ENQUEUE_WAKEUP` flag, enqueue
5. `ttwu_do_wakeup()` — `check_preempt_wakeup()`, set `TIF_NEED_RESCHED`
6. `ttwu_queue_remote()` — if waking on different CPU, send IPI

**Missing in Rux:** `select_task_rq`, `wake_affine`, `pi_lock`, remote wake-up.

---

## 2. CFS Scheduler (`kernel/src/sched/fair.rs`)

### `SchedEntity` Structure

| Field | Rux | Linux |
|---|---|---|
| `vruntime: AtomicU64` | Yes | Yes |
| `sum_exec_runtime: AtomicU64` | Yes | Yes |
| `exec_start: AtomicU64` | Yes | Yes |
| `on_rq: AtomicBool` | Yes | Yes |
| `slice: AtomicU64` | Yes | Yes (6.x) |
| `load_weight: LoadWeight` | Yes (embedded) | Yes (separate field) |
| `depth` | No | Yes — cgroup hierarchy depth |
| `parent` | No | Yes — parent entity pointer |
| `cfs_rq` | No | Yes — pointer to owning cfs_rq |
| `my_q` | No | Yes — pointer to child cfs_rq |
| `vlag` | No | Yes — EEVDF virtual lag |
| `vprot` | No | Yes — EEVDF virtual protection |
| `deadline` | No | Yes — EEVDF virtual deadline |
| `run_node` | No (BTreeMap key) | Yes — rbtree node |
| `group_node` | No | Yes — cgroup list node |

### `CfsRunQueue` Structure

| Field | Rux | Linux (`struct cfs_rq`) |
|---|---|---|
| `tasks: BTreeMap<(vruntime, task_id), *mut Task>` | Yes | `rb_root_cached tasks_timeline` |
| `curr: *mut Task` | Yes | Yes |
| `min_vruntime: AtomicU64` | Yes | Yes |
| `nr_running: AtomicU32` | Yes | Yes |
| `load_weight: LoadWeight` | Yes | Yes |
| `rq: *mut RunQueue` | No | Yes — back-pointer to parent rq |
| `nr_spread_over` | No | Yes — spread counter |
| `exec_clock` | No | Yes — execution clock |
| `idle_nr_running` | No | Yes — idle entity count |
| `h_nr_running` | No | Yes — hierarchical count |
| `throttled_clock` | No | Yes — bandwidth throttle |
| `curr_time` | No | Yes — current EEVDF time |

### `update_curr()` (fair.rs)

**Rux (correct):**
```rust
fn update_curr(rq: &mut RunQueue) {
    let now = sched_clock();
    let delta = now - curr_entity.exec_start.load(Acquire);
    curr_entity.exec_start.store(now, Release);
    // vruntime += delta * NICE_0_LOAD / weight
    let vdelta = calc_delta_fair(delta, &weight);
    curr_entity.vruntime.fetch_add(vdelta, Release);
    cfs_rq.min_vruntime = max(min_vruntime, curr_entity.vruntime);
}
```

**Linux additions:**
- `cfs_rq->exec_clock += delta`
- `account_cfs_rq_runtime()` — bandwidth control
- `cfs_rq->runtime_remaining` tracking
- `psi_task_change()` — PSI accounting

### `enqueue_task()` (fair.rs:500)

**Rux:** Inserts `(vruntime, task_id) → *Task` into BTreeMap. Sets `on_rq = true`.

**Linux `enqueue_task_fair()` (fair.c):**
1. For each level in cgroup hierarchy:
   - `enqueue_entity()` → `__enqueue_entity()` → rbtree insert
   - `update_load_add()` — propagate load upward
   - `update_cfs_group()` — update shares
2. `h_nr_running++` at each level
3. If WAKEUP: `place_entity()` adjusts vruntime for sleeper fairness
4. If ENQUEUE_WAKEUP: `check_schedstat_required()`

### `pick_next_task()` (fair.rs:600)

**Rux:** `cfs_rq.tasks.first_key_value()` → returns lowest vruntime task. O(log n).

**Linux `pick_next_task_fair()`:**
- `__pick_first_entity()` → `rb_first()` → O(1) cached pointer
- `set_next_entity()` → clears `on_rq`, updates `exec_start`
- `check_cfs_rq_runtime()` — bandwidth throttle check
- EEVDF logic: checks `se->deadline` vs `cfs_rq->curr_time`

### `select_task_rq()` (fair.rs:904) — **STUB**

**Rux:** Always returns current CPU:
```rust
fn select_task_rq(task: *mut Task, cpu: usize, flags: u64) -> usize { cpu }
```

**Linux `select_task_rq_fair()` (fair.c:7900+):**
1. `wake_affine_idle()` — if dst CPU idle and task is cache-cold, prefer idle CPU
2. `wake_affine_weight()` — weighted wake-affine based on load
3. `find_energy_efficient_cpu()` — EAS (Energy Aware Scheduling)
4. `select_idle_sibling()` — pick idle core in same L1/L2 domain
5. `sd_flag` based search through sched_domain levels
6. Fallback: `select_idle_cpu()` — scan for any idle CPU

### `wakeup_preempt()` (fair.rs:700)

**Rux:** Simple vruntime comparison:
```rust
if wake_vruntime < curr_vruntime { resched_curr(); }
```

**Linux `check_preempt_wakeup()` (fair.c):**
1. `wakeup_preempt_entity(se, curr)` — EEVDF preemption check
2. `WAKEUP_PREEMPTION` feature flag
3. `sysctl_sched_wakeup_granularity` — preemption granularity control
4. `resched_curr_lazy()` for non-sync wakeups

---

## 3. RT Scheduler (`kernel/src/sched/rt.rs`)

### `RtRunQueue` Structure (line 22)

| Field | Rux | Linux |
|---|---|---|
| `rt_nr_running: AtomicU32` | Yes | Yes |
| `rr_nr_running: AtomicU32` | Yes | Yes |
| `highest_prio: AtomicU32` | Yes | Yes |
| `overloaded: AtomicBool` | Yes (never set) | Yes (set when nr > cpus) |
| `bitmap: [AtomicU64; 2]` | Yes — 128 bits for 100 priorities | Yes — `struct prio_array` |
| `queue: [ListHead; 100]` | Yes — per-priority lists | Yes — `struct plist` |
| `rt_throttled` | No | Yes — throttle flag |
| `rt_time` | No | Yes — accumulated runtime |
| `rt_runtime` | No | Yes — runtime limit per period |
| `rt_period` | No | Yes — period length |
| `pushable_tasks` | No | Yes — plist for push balancing |

### Priority Bitmap (line 86)

**Rux `find_highest_prio()` — CORRECT:**
```rust
fn find_highest_prio(&self) -> Option<u32> {
    for word in &self.bitmap {
        let val = word.load(Acquire);
        if val != 0 { return Some(val.trailing_zeros()); }
    }
    None
}
```
This is O(1) and matches Linux's `sched_find_first_bit()`.

### `task_tick()` (rt.rs:436)

**Rux:** For SCHED_RR, decrements time_slice, on expiry requeues at tail and calls `resched_curr()`.

**Linux additions:**
- `sched_rt_runtime_exceeded()` — check if RT bandwidth exceeded
- `sched_rt_period_timer()` — periodic bandwidth timer

### `balance()` / Load Balancing — **STUB**

**Rux:** Returns `false`.

**Linux:**
- `push_rt_task()` — pushes RT task to lowest-priority runqueue
  - `find_lock_lowest_rq()` — searches cpus_allowed for CPU with lowest RT priority
  - Double rq lock (src + dst, ordered by CPU id)
  - Moves task via `dequeue_pushable_task()` / `enqueue_pushable_task()`
- `pull_rt_task()` — pulls RT task from overloaded CPU when current CPU has no RT tasks
  - Triggered by `schedule()` when RT rq is empty but other CPUs have RT tasks
  - `pick_next_highest_task_rt()` — selects task to pull

### `select_task_rq()` (rt.rs:426) — **STUB**

**Rux:** Always returns current CPU.

**Linux `select_task_rq_rt()`:**
1. `find_lowest_rq()` — find CPU in cpus_allowed with:
   - No RT tasks, or lower RT priority than this task
   - CPU is online
2. Lock found CPU's rq, verify it's still lowest
3. Return found CPU

---

## 4. Deadline Scheduler (`kernel/src/sched/deadline.rs`)

### `SchedDlEntity` Structure (line 198)

| Field | Rux | Linux |
|---|---|---|
| `deadline: AtomicU64` | Yes | Yes |
| `runtime: AtomicI64` | Yes | Yes |
| `dl_period: AtomicU64` | Yes | Yes |
| `dl_runtime: AtomicU64` | Yes | Yes |
| `dl_throttled: AtomicBool` | Yes | Yes |
| `on_rq: AtomicBool` | Yes | Yes |
| `dl_boosted: AtomicBool` | Yes (never used) | Yes |
| `dl_server` | No | Yes — CBS server pointer |
| `dl_bw_ratio` | No | Yes — cached bandwidth ratio |
| `dl_overrun` | No | Yes — overrun counter |
| `dl_yielded` | No | Yes — yield tracking |
| `dl_non_contending` | No | Yes — inactive bandwidth tracking |
| `dl_timer` | No | Yes — hrtimer for period replenish |

### `update_curr()` (deadline.rs:453) — **BROKEN**

```rust
fn update_curr(rq: &mut RunQueue) {
    let exec_start = 0; // TODO: track exec_start
    let delta = sched_clock() - exec_start;  // always sched_clock()!
    // This would consume ALL remaining runtime instantly
}
```

`exec_start` is hardcoded to 0, so `delta` equals the entire time since boot. This function is effectively non-functional. The only working runtime path is `task_tick()` which consumes 10ms per tick.

**Linux:** Tracks `se->dl_last_update` on every `update_curr_dl()`, calculates `delta_exec` correctly.

### `enqueue_task()` (deadline.rs:309)

**Rux:** Replenishes runtime, updates deadline to `now + period`, enqueues into BTreeMap.

**Linux `enqueue_task_dl()` additions:**
1. `update_dl_entity()` — CBS algorithm:
   - If task depleted runtime and is non-contending: `replenish_dl_entity()`
   - If `dl_se->deadline < now`: deadline = now + period
2. `setup_new_dl_entity()` — first-time initialization
3. Admission control: `dl_bw_overflow(task)`
4. `dl_add_task_bw()` — update `this_bw` and `total_bw` for GRUB

### `balance()` — **STUB**

**Rux:** Returns `false`.

**Linux:**
- `push_dl_task()` — push DL task to CPU with earliest available deadline slot
- `pull_dl_task()` — pull DL task from overloaded CPU
- `find_lock_later_rq()` — find CPU with enough bandwidth

### Admission Control — **MISSING**

**Linux:**
- `dl_bw_overflow()`: checks if `total_bw > 100%` of CPU
- `dl_add_task_bw()` / `dl_sub_task_bw()`: track per-rq bandwidth usage
- `GRUB` (Greedy Reclaimation of Unused Bandwidth): reclaims bandwidth from inactive tasks
- `this_cpu_bw()` — actual bandwidth used on this CPU

Rux has `running_bw` field but it is never read or written.

---

## 5. Idle Scheduler (`kernel/src/sched/idle.rs`)

**Rux `pick_next_task()`:** Returns `rq.idle`. Correct.

**Rux `task_tick()`:** Calls `update_curr()` to update idle task's `exec_start`. Minimal.

**Linux `do_idle()` additions:**
- `cpuidle_idle_call()` — selects deepest valid C-state via governor
- `tick_nohz_idle_enter()` — stops scheduler tick entirely (tickless)
- `rcu_idle_enter()` — RCU dyntick-idle mode
- `idle_balance()` — pulls tasks from other CPUs before going idle
- `cpu_startup_entry()` — loop with CPUHP states

---

## 6. Stop Task Scheduler (`kernel/src/sched/stop_task.rs`)

**Rux:** Minimal skeleton. Stores/retrieves stop task from `rq.stop`. No CPU hotplug or migration logic.

**Linux:**
- `migration_thread()` — kernel thread for task migration
- `cpu_stopper_thread()` — handles `cpu_stop` work items
- `active_load_balance_cpu_stop()` — uses stop task to pull task from busy CPU
- `cpu_stop_queue_work()` — queue work on stop task
- CPU hotplug: `cpuhp_invoke_callback()` via stop tasks

---

## 7. SchedClass Trait (`kernel/src/sched/class.rs`)

### Method Coverage

| Method | Rux | Linux | Status |
|---|---|---|---|
| `enqueue_task` | Implemented (basic) | Full | Partial |
| `dequeue_task` | Implemented (basic) | Full | Partial |
| `yield_task` | No-op for RT/Stop | Requeues at tail | Missing |
| `wakeup_preempt` | Basic check | Wake-affine + EEVDF | Partial |
| `pick_next_task` | Implemented | Full | OK |
| `put_prev_task` | Implemented | Full | OK |
| `set_next_task` | Implemented | Full | OK |
| `balance` | Returns false (all classes) | Full LB logic | **Stub** |
| `select_task_rq` | Returns current CPU | NUMA + wake-affine | **Stub** |
| `task_tick` | Implemented | Full | OK |
| `update_curr` | CFS OK, DL broken | Full | Partial |
| `get_rr_interval` | Implemented | Full | OK |
| `has_runnable` | Implemented | Full | OK |
| `next_class` | Implemented | Full | OK |

### Enqueue Flags

**Rux:** Only `ENQUEUE_HEAD` is used (for RT requeue on RR expiry).

**Linux:**
| Flag | Value | Purpose |
|---|---|---|
| `ENQUEUE_WAKEUP` | 0x01 | Task waking from sleep |
| `ENQUEUE_RESTORE` | 0x02 | Restore after migration |
| `ENQUEUE_MOVE` | 0x04 | Cross-CPU enqueue |
| `ENQUEUE_NO_CLOCK` | 0x08 | Skip clock update |
| `ENQUEUE_HEAD` | 0x80 | Insert at queue head |

### Wake Flags

**Rux:** Not defined.

**Linux:**
| Flag | Value | Purpose |
|---|---|---|
| `WF_EXEC` | 0x02 | Wake after exec |
| `WF_FORK` | 0x04 | Wake after fork |
| `WF_TTWU` | 0x08 | ttwu path |
| `WF_MIGRATED` | 0x10 | Task migrated |

---

## 8. SMP Infrastructure

### 8.1 Secondary CPU Startup (`kernel/src/arch/riscv64/smp.rs`)

**Rux `secondary_cpu_start()` (line 83):**
```rust
pub extern "C" fn secondary_cpu_start() -> ! {
    let hart_id = cpu_id();
    mark_cpu_started(hart_id);
    loop { unsafe { asm!("wfi", options(nomem, nostack)); } }
}
```

Secondary CPUs enter a dead WFI loop. They:
- Do NOT create an idle task
- Do NOT set `tp` register to a task pointer
- Do NOT enable interrupts
- Do NOT set up the timer
- Do NOT enter `cpu_idle_loop()`

**This is the #1 blocker for SMP.** Without secondary CPUs in the scheduler, all tasks run on CPU 0.

**Linux `secondary_start_kernel()` (ARM64) / `start_secondary()` (x86):**
1. `cpu_startup_entry(CPUHP_AP_ONLINE_IDLE)` — enter CPU hotplug state machine
2. `notify_cpu_starting(cpu)` — notify CPUHP subscribers
3. `smp_callin()` — set CPU online
4. `cpu_idle()` → `do_idle()` — enter idle loop

### 8.2 IPI Infrastructure (`kernel/src/arch/riscv64/ipi.rs`)

**Rux:**
```rust
pub enum IpiType { Reschedule = 0, Stop = 1 }

pub fn send_reschedule_ipi(target_cpu: usize) {
    sbi::send_ipi(1 << target_cpu);  // no type encoding
}

pub fn handle_software_ipi(hart: usize) {
    set_need_resched();  // assumes all IPIs are reschedule
    schedule();
}
```

**Problems:**
1. `IpiType::Stop` is defined but never used
2. No type encoding — receiver always assumes reschedule
3. Only 2 types vs. Linux's 6+

**Linux RISC-V IPI types:**
| Type | Purpose |
|---|---|
| `IPI_RESCHEDULE` | Reschedule request |
| `IPI_CALL_FUNC` | Execute function on target CPU |
| `IPI_CALL_FUNC_SINGLE` | Execute on single CPU |
| `IPI_IRQ_WORK` | IRQ work queue |
| `IPI_STOP` | CPU stop/hotplug |
| `IPI_TIMER` | Tick broadcast |
| `IPI_CPU_BACKUP` | CPU backup (RISC-V specific) |
| `IPI_VF` | Virtualization feature |
| `IPI_CLEAR_VDBF` | Clear virtual dirty bitmap |

### 8.3 Timer Tick Connection

**Rux `trap.rs:235-241` (commented out):**
```rust
// 3. TODO: Call scheduler tick
// crate::sched::scheduler_tick();
// 4. TODO: Check if reschedule needed
// if crate::sched::need_resched() && !crate::sync::is_locked() {
//     crate::sched::schedule();
// }
```

**Consequences of not calling `scheduler_tick()`:**
1. Time slices are never decremented → no time-slice-based preemption
2. `update_curr()` is never called from tick → vruntime not updated regularly
3. RT RR time slices never expire → RR tasks run forever
4. DL runtime never consumed (via tick path) → bandwidth control broken
5. Load balancing never triggered from tick → tasks stay on original CPU

**Linux:** `tick_sched_timer()` is an hrtimer that calls `scheduler_tick()` every `1/HZ` seconds (10ms at HZ=100).

### 8.4 Big Kernel Lock (`kernel/src/sync/kernel_lock.rs`)

**Rux:**
```rust
pub static mut KERNEL_LOCK: AtomicU64 = AtomicU64::new(0);

pub fn kernel_lock_acquire() {
    loop { amoswap.d.aq t1, 1, (t0); if t1 == 0 { break; } }
}
```

Acquired on every kernel entry (trap/syscall in `trap.S`), released on return to user.

**Impact on SMP:**
- All 4 CPUs serialize on kernel entry — only one CPU can be in kernel mode at a time
- Prevents concurrent syscalls, interrupts, page faults from different CPUs
- Makes load balancing pointless — tasks can't actually run concurrently in kernel
- Timer tick preemption would be meaningless — `is_locked()` guard prevents scheduling

**Linux:** BKL was removed in v2.6.39 (2011). Modern Linux uses:
- Per-rq `raw_spinlock_t` for scheduler state
- Per-lock fine-grained locking throughout the kernel
- Lockdep for deadlock detection
- `PREEMPT_RT` for real-time preemption

---

## 9. Task Structure (Scheduling-Related Fields)

**Rux `Task` (kernel/src/process/task.rs):**

| Field | Line | Present | Notes |
|---|---|---|---|
| `ti_flags: AtomicU32` | 314 | Yes | TIF_* flags |
| `ti_preempt_count: AtomicI32` | 318 | Yes | Preemption count |
| `ti_cpu: AtomicI32` | 329 | Yes | Current CPU |
| `state: AtomicU32` | 340 | Yes | TaskState |
| `policy: SchedPolicy` | 353 | Yes | SCHED_* |
| `prio: i32` | 358 | Yes | Dynamic priority |
| `static_prio: i32` | 361 | Yes | Base priority |
| `normal_prio: i32` | 364 | Yes | Computed priority |
| `time_slice: u32` | 367 | Yes | Remaining slice |
| `sched_entity: SchedEntity` | 372 | Yes | CFS entity |
| `rt_priority: u32` | 377 | Yes | RT priority 0-99 |
| `rt_run_list: ListHead` | 382 | Yes | RT queue linkage |
| `rt_entity: SchedRtEntity` | 387 | Yes | RT entity |
| `dl_entity: SchedDlEntity` | 392 | Yes | DL entity |
| `cpus_allowed` | — | **No** | No CPU affinity mask |
| `wake_cpu` | — | **No** | No wake-up CPU tracking |
| `on_rq` | — | **No** | Per-entity `on_rq` exists |
| `nr_cpus_allowed` | — | **No** | No affinity count |
| `recent_used_cpu` | — | **No** | No NUMA tracking |
| `migration_cpu` | — | **No** | No migration tracking |
| `flags` | — | **No** | TaskFlags defined but not stored |
| `exit_state` | — | **No** | No separate exit state |

---

## 10. Missing SMP Features Summary

| # | Feature | Priority | Current Status | Impact |
|---|---|---|---|---|
| 1 | Secondary CPU scheduler entry | P0 | CPUs WFI-loop forever | Only CPU 0 runs tasks |
| 2 | Timer tick wiring | P0 | `scheduler_tick()` commented out | No preemption |
| 3 | CPU affinity (cpus_allowed) | P0 | Not implemented | No CPU restriction |
| 4 | select_task_rq implementation | P1 | Returns current CPU | No smart placement |
| 5 | Task migration | P1 | Basic steal only | No proper migration |
| 6 | Sched domain topology | P1 | Not implemented | No hierarchical balancing |
| 7 | RT load balancing | P1 | Stub (returns false) | RT tasks pinned to CPU |
| 8 | IPI expansion | P1 | 1 of 2 types used | Limited cross-CPU ops |
| 9 | BKL removal | P2 | Global spinlock | Serializes all CPUs |
| 10 | NOHZ/tickless idle | P2 | Not implemented | Unnecessary timer interrupts |
| 11 | CFS bandwidth control | P2 | Not implemented | No CPU quota enforcement |
| 12 | RT throttling | P2 | Not implemented | RT can starve CFS |
| 13 | DL admission control | P2 | Field exists, unused | DL can overcommit |
| 14 | DL update_curr fix | P2 | `exec_start = 0` (TODO) | DL bandwidth broken |
| 15 | CPU hotplug | P3 | Not implemented | No dynamic CPU mgmt |
| 16 | cpuidle integration | P3 | WFI only | No power management |
| 17 | cgroup CPU controller | P3 | Not implemented | No hierarchical scheduling |
