# preempt_count per-CPU migration — inventory, design, verification

Lane p23-percpupreempt. Base: origin/main (3363d5160) + the three
p23-p1panic prerequisites (HZ calibration / tp-truth adjudication /
user-return boundary repair), hand-ported onto the restructured tree in
ef6de5ad7 / f401b7010 / 1d2a80b2d.

## 1. Problem

The tree stores preempt_count in a per-task field (`Task::ti_preempt_count`,
an AtomicI32). Every guard bracket does `+1` by resolving `current()` at
acquire and `-1` by resolving `current()` at release. When a continuation
changes task identity between the two resolves (a switch inside the bracket:
s14 IRQ-PAIR family, the sc69-sc73 add/drop split family), the `+1` stays on
the acquire-time task and the `-1` lands on the release-time task. The result
is a stranded count on an innocent task (permanently unpreemptible
monopolist) plus an underflow on the executor, and every downstream
remediation built on the field (tick-side PREEMPT-HEAL, PARKED-HEAL,
PICK-PC-ZERO, WAKE-HEAL, UR-PC-REPAIR) both depends on and amplifies that
state (heal on a live holder = the lm300 crash family).

The reference kernel (x86) has no such split surface: preempt_count is a
per-CPU variable (`pcpu_hot.preempt_count`; `raw_cpu_read/add`), so the add
and sub of one bracket always hit the SAME CPU word, whatever task identity
changes happen in between.

## 2. Inventory (all read/write points at base commit)

### Authoritative mutation points (all funnel through `interrupt/preempt.rs`)

| Site | What it does today |
|---|---|
| preempt.rs `preempt_count()` | resolves `current()`, reads task field |
| preempt.rs `preempt_count_add/_at` | resolves `current()`, task `fetch_add` |
| preempt.rs `preempt_count_sub/_at/_sub2_at` | resolves `current()`, task `fetch_sub` (+ underflow clamp/report, leakorigin outermost-release shadow, per-field run resets) |
| preempt.rs `preempt_count_reset` | resolves `current()`, zero field (R74 fault-path self-heal) |
| preempt.rs `irq_enter` | `+HARDIRQ_OFFSET` on current task + booking (IRQ_BOOKED depth stack; IRQ_PAIR_TASK leakhunt capture) |
| preempt.rs `irq_exit` | `-HARDIRQ_OFFSET` on BOOKED task (remote release `release_hardirq_base_on` when the bracket spanned a switch; IRQ-BOOK-SPLIT capture) |
| task.rs `Task::{preempt_count,inc/dec,add/sub_preempt_count*}`, `ti_preempt_count_zero/clear_base/keep_tick_nesting`, `release_hardirq_base_on/_remote` | the field accessors the above call; also called directly by remediation paths |

Every lock family brackets through preempt_count_add/sub (GRQ
lock_irqsave/lock_plain/try_lock_irqsave, SpinlockGuard, rwlock, rwsem,
mutex, RCU, softirq __do_softirq bracket, SeqLock::write, NMI bracket) —
they need no per-site change once preempt.rs is switched.

### Arch entry asm (direct field-offset access)

| Site | Access |
|---|---|
| riscv64/trap.S user gate (~line 815) | `lw t0, {task_ti_preempt}(tp)` — i32 offset-4 load; template const `task_ti_preempt` = `task_offsets::TI_PREEMPT_COUNT` (global_asm at arch/unsafe.rs:4192) |
| riscv64/trap.S kernel twin (.Lcheck_resched_kernel ~988-1069) | deliberate wide `ld` (kernel-mode trap-exit preemption stays disabled) |
| aarch64/trap.S aarch64_exit_to_user (~238) | `ldr w21, [x20, #TI_PREEMPT]`; template const at arch/unsafe.rs:24133 |
| x86_64/trap.S (~460, ~562) | calls `x86_preemptible()` (Rust) — no raw access; re-points automatically |

### Same-CPU decision/diagnostic readers (re-point to the CPU word)

| Site | Use |
|---|---|
| arch/unsafe.rs:3786 (timer trap path) | TICK_ENTRY_PC snapshot at tick entry (`pctick=` discriminator) |
| preempt.rs `tick_orphan_scan` | BIT-ORPHAN probe (own-CPU entry snapshot vs current word) |
| sched.rs scheduler_tick leak window (~8857) | the PREEMPT-LEAK probe sample + per-field present-run counters (ti_pc_{p,s,h}irq_present_run) + PREEMPT-HEAL adjudication |
| sched.rs ~2548 / 2623 / 3540 / 3800 / 8063 / 12296 | proc dump, TIMER-SOFTIRQ-STALL report, SCHED-IRQOFF print, schedule()-entry diagnostics, tick report_pc |
| dfx/leakorigin.rs `check_user_return` | UR-PC-REPAIR + LEAK-ORIGIN user-return reporter (own task at user return) |

### Per-task remediation (dissolved by the migration; retired in cleanup)

| Site | Why it dissolves |
|---|---|
| scheduler_tick PREEMPT-HEAL (~9070+) | zeroes a phantom per-task count held 2 s; a per-CPU word held 2 s is usually a LIVE lock hold — healing it is the lm300 crash family. Reference has no counterpart. |
| parkwalk PARKED-HEAL / PARK-WILDWRITE (~9400-9800) | "parked task with nonzero count" is structurally impossible once the count is CPU state |
| __schedule park adjudication (~4210-4270) | phantom IRQ-bit zeroing on the outgoing task; at switch the word holds only scheduler-owned brackets |
| PICK-PC-ZERO (~5485) | zeroes "resurrected phantom" at pick; phantoms cannot ride a task anymore |
| WAKE-HEAL wake-path adjudication (~7218) | refuses/heals wakes on tasks carrying counts; moot |
| context_switch SWITCH-COUNT-LEAK check | prev no longer carries a count; the discipline (never switch under a task-held bracket) is enforced at the schedule() entry gate (SCHEDULE-ATOMIC) exactly as the reference does with `in_atomic()` + `__schedule_bug` |
| UR-PC-REPAIR zeroing (leakorigin check_user_return) | the user-return boundary guarantee (count must be 0) is now the CPU word's job; the probe's capture line stays as the zero-occurrence witness |

### Remote/ambient readers (cleanup commit)

taskdump.rs (task list pc=), slowtask.rs, softlockup.rs (stuck-task pc),
wakewatch.rs, timer.rs stall report, rcu.rs stall report, sysv_sem.rs,
binder.rs, wait.rs lostpark record, syscall/misc.rs, allocwatch.rs,
sched.rs orphan-harvest gate (11779: `preempt_count() == 0` condition),
dfx/r83hunt.rs (plausibility scan via `task_offsets::TI_PREEMPT_COUNT`),
dfx/unsafe.rs `t_preempt_count` (forensics edge), interrupt/unsafe.rs:224.

## 3. Design (reference-form parity)

Reference facts checked against Linux v6.6 (kernel/sched/core.c,
arch/x86/include/asm/preempt.h):

1. `preempt_count()` is `raw_cpu_read_4(pcpu_hot.preempt_count)` — a
   per-CPU word; `__preempt_count_add/sub` are raw per-CPU RMW. No
   `current()` resolution on either side of a bracket.
2. The count is CPU state, NOT task state, across context switches: at
   `switch_to` the word holds exactly the scheduler's own brackets
   (schedule()'s `preempt_disable()` + rq-lock disable = 2 — the
   `finish_task_switch` WARN pins `preempt_count() == 2*PREEMPT_DISABLE_OFFSET`
   at switch-in). There is NO per-task save/restore; the incoming task
   inherits the scheduler's brackets and they drain as the switch unwinds.
   The precondition — at switch time no task-held bracket can exist —
   holds because preemption/sleep only reaches __schedule when the count
   is clean (our equivalent: the SCHEDULE-ATOMIC entry gate and the
   pick-side discipline).
3. `init_idle_preempt_count` sets PREEMPT_DISABLED for idle; our idle
   paths bracket through the same API and need no special case.
4. The reference's only count remediation is `__schedule_bug`'s
   `preempt_count_set(PREEMPT_DISABLED)` clamp when schedule() is entered
   in atomic context — a bug REPORTER, not a live-holder healer. We keep
   the same stance: report (SCHEDULE-ATOMIC, PREEMPT-LEAK window,
   BIT-ORPHAN, underflow clamp), never steal a live bracket.

### Rux form

- `interrupt/preempt.rs`: new `PREEMPT_WORDS: [PreemptWord; MAX_CPUS]`
  (AtomicI32, 64-byte-aligned rows — same-CPU-only writes by construction:
  you can only be preempted when the word is 0, and hardirq/softirq nest
  strictly on the owning CPU; remote READS allowed for diagnostics via
  `cpu_preempt_count(cpu)`).
- All preempt.rs API operates on the own-CPU word. Guard families change
  nothing (they call the same API).
- irq_enter/irq_exit: the booking discipline retires — enter books the
  CPU word, exit subs the SAME word. The leakhunt-gated capture lines
  (IRQ-PAIR-SPLIT enter/exit task comparison, IRQ-BOOK-SPLIT) are KEPT as
  migration-verification probes: post-migration a bracket that spans a
  switch can no longer split the count, so SPLIT-RELEASE must be zero.
  `hardirq_depth_live` keeps its depth counter (the __schedule voluntary-
  sleep exemption still needs "is a bracket in flight").
- The per-field present-run counters move to per-CPU statics (same-CPU
  tick observations, same-CPU resets on outermost pair) — the report gate
  semantics are unchanged; the heal consumer is retired with them.
- Arch gates: riscv64 user + kernel twins and aarch64 user gate replace
  the field loads with `call asm_preemptible` (extern "C", reads the CPU
  word) — the x86 precedent (`x86_preemptible`). Both gates are reached
  only when need_resched is already set, so the call sits on the
  resched-pending path only.
- The tick-side heal, PARKED-HEAL, PICK-PC-ZERO, WAKE-HEAL, park
  adjudication zeroing and UR-PC-REPAIR zeroing retire (structurally
  impossible states / unsound against the word). The REPORT lines they
  fed (PREEMPT-LEAK window, BIT-ORPHAN, LEAK-ORIGIN captures, parkwalk
  census prints) stay, reading the CPU word.
- `Task::ti_preempt_count` freezes at 0 during migration (accessors keep
  compiling; remote prints read 0) and is deleted with its accessors,
  the `task_offsets::TI_PREEMPT_COUNT` const and the r83hunt offset use
  in the cleanup commit.

## 4. Cleanup record (commit 3)

Retired with the per-task field (net -2.2k lines):
- PREEMPT-HEAL tick adjudication + HEAL-GATE / CARRIER-HEAL refusal
  legs + the per-field present-run counters (now per-CPU statics were
  added and then removed with their only consumer); the leak window
  (PREEMPT-LEAK report) stays, sampling the cpu word.
- PARKWALK (PARKED-LEAK / PARK-WILDWRITE / PARKED-HEAL) — "a parked
  task's count" no longer exists.
- __schedule park adjudication (phantom IRQ-bit zeroing); replaced by
  the schedule()-entry base-holdout gate (SCHEDULE-HOLDOUT +
  PARK-REFUSED-ORIGIN under leakhunt) — the reference's
  __schedule_bug boundary.
- PICK-PC-ZERO, WAKE-HEAL (+LP-OUTCOME), context_switch
  SWITCH-COUNT-LEAK, the wake-relink pc print, the orphan-harvest
  count gate, Task::release_hardirq_base_on/_remote, the softirq
  window's booked-task remote release (SOFTIRQ-EXIT-SPANNED keeps a
  silence probe for the switch-inside-window shape), UR-PC-REPAIR's
  zero store (the capture line stays).
- Task::ti_preempt_count, ti_pc_zero_jif, the three present-run
  fields, all 20 accessors, task_offsets::TI_PREEMPT_COUNT and the
  r83hunt offset-scan entry + WatchSnap.preempt.
- Every remote diagnostic reader re-pointed: same-cpu sites to
  preempt_count(), per-task listings to cpu_preempt_count(ti_cpu)
  (the task's last cpu's word) — taskdump, slowtask, softlockup,
  wakewatch, allocwatch, binder, sysv_sem, wait.rs lostpark, rcu/timer
  stall reports, misc syscall dump, trap noctx print, irq/softirq
  bracket forensics, r83hunt (removed entirely from r83hunt).

## 5. Verification plan (state at the cleanup commit)

1. Dual-arch build rc=0 (x86_64 + riscv64; aarch64 build gate).
2. HZ=1000 rig (the p23-p1panic repro): 300 s lmbench + 1 ms sampling
   window, 3 consecutive clean boots (no death, no new panic class).
3. SPLIT-RELEASE probe: leakhunt-on boot, IRQ-PAIR-SPLIT / IRQ-BOOK-SPLIT
   count == 0 across the window.
4. `make test` (riscv64) + x86 suite full green.
5. pbench x86/KVM A/B: no regression (Tier 2 — sched/ paths touched).
