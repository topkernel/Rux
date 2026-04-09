# Rux Kernel Lock Ordering

This document defines the lock acquisition hierarchy for the Rux kernel.
All code paths that acquire multiple locks must follow this order to prevent deadlocks.

## Lock Hierarchy

Locks must be acquired in order from Level 0 (outermost) to Level 4 (innermost).
Release order is the reverse.

```
Level 0 (outermost): IRQ disable (irq_save / save_and_disable_irq)
  |
  +-- Level 1: preempt_disable (preempt_count_add(PREEMPT_OFFSET))
        |
        +-- Level 2a: GRQ lock (kernel/src/sched/sched.rs: GlobalRunQueue.lock)
        |     |
        |     +-- Level 3a: per-zone lock (kernel/src/mm/zone.rs)
        |     |
        |     +-- Level 3b: futex hash bucket lock (kernel/src/sync/futex.rs: HASH_HEADS[i])
        |           |
        |           +-- Level 4: waiter slot lock (kernel/src/sync/futex.rs: WAITER_POOL[i])
        |
        +-- Level 2b: process tree lock (kernel/src/process/task.rs)
        |
        +-- Level 2c: inode lock (kernel/src/fs/)
        |
        +-- Level 2d: dentry cache lock (kernel/src/fs/)
```

## Key Nesting Paths

### Path 1: Spinlock (any variant)

```
irq_save -> preempt_disable -> spinlock::lock -> ... -> unlock -> preempt_enable -> irq_restore
```

Source: `kernel/src/sync/spinlock.rs` `lock_irqsave()` (line 192) and `Drop` (line 329).

### Path 2: GRQ lock (scheduler)

```
irq_save -> preempt_disable -> grq.lock -> enqueue/dequeue -> grq.unlock -> preempt_enable -> irq_restore
```

Source: `kernel/src/sched/sched.rs` `lock_irqsave()` (line 85) and `GrqGuard::Drop` (line 181).

### Path 3: Futex wait

```
irq_save -> preempt_disable -> bucket_lock -> waiter_slot_lock -> ...
  -> unlock_waiter_slot
  -> set_state(INTERRUPTIBLE) [under bucket_lock]
  -> unlock_bucket -> preempt_enable -> irq_restore
  -> schedule() [GRQ lock acquired independently]
```

Source: `kernel/src/sync/futex.rs` `futex_wait()` (line 253).

### Path 4: Futex wake (CRITICAL)

```
irq_save -> preempt_disable -> bucket_lock
  -> waiter_slot_lock -> unlock_waiter_slot
  -> Task::wake_up -> enqueue_task
    -> irq_save -> preempt_disable -> grq.lock  [NESTED: GRQ inside bucket]
    -> enqueue_task_locked
    -> grq.unlock -> preempt_enable -> irq_restore
  -> unlock_bucket -> preempt_enable -> irq_restore
```

Source: `kernel/src/sync/futex.rs` `futex_wake()` (line 317).

**INV-LOCK-5**: GRQ may nest inside futex bucket lock, but never the reverse.

### Path 5: Scheduler context switch

```
__schedule():
  irq_save -> preempt_disable -> grq.lock
  -> pick_next_task
  -> grq.unlock_irqretain() [unlock + preempt_enable, keep IRQ disabled]
  -> context_switch [runs with IRQ disabled]
  -> irq_restore
```

Source: `kernel/src/sched/sched.rs` `__schedule()` (line 556).

## Invariants

| ID | Invariant | Enforcement |
|----|-----------|-------------|
| INV-LOCK-1 | preempt_disable must precede any spinlock acquire | Spinlock::lock() calls preempt_disable |
| INV-LOCK-2 | irq_save must precede preempt_disable when both needed | lock_irqsave() order: irq -> preempt -> lock |
| INV-LOCK-3 | Release order: unlock -> preempt_enable -> irq_restore | Guard Drop order |
| INV-LOCK-4 | No lock acquisition cycles (deadlock-free) | Hierarchy documented here, verified by SPIN |
| INV-LOCK-5 | GRQ nests inside bucket lock, never reverse | futex_wake path, verified by SPIN |

## preempt_count Bitfield

The `preempt_count` field (AtomicI32 in Task struct) tracks nesting depth:

```
bits [0:7]   PREEMPT_MASK  (0x000000FF)  preempt disable depth
bits [8:15]  SOFTIRQ_MASK  (0x0000FF00)  softirq nesting count
bits [16:19] HARDIRQ_MASK  (0x000F0000)  hard IRQ nesting count
bit  [20]    NMI_MASK      (0x00100000)  NMI count
bit  [26]    PREEMPT_ACTIVE (0x04000000)  actively preempting (reserved)
```

Source: `kernel/src/interrupt/preempt.rs` lines 7-40.

`preemptible()` returns `true` only when preempt_count == 0 (all fields zero).
Schedule is called only when preemptible (enforced in `trap.S` line 626 and `trap.rs` line 261).

## Known Concerns

1. **Futex wake GRQ nesting**: `bucket_lock -> GRQ_lock` is the deepest nesting path.
   If any code path acquires GRQ first and then tries bucket_lock, deadlock results.
   Currently no such path exists, but new code must be audited.

2. **Waiter slot lock nesting**: `bucket_lock -> waiter_slot_lock -> unlock -> [GRQ_lock]`
   The waiter slot lock is short-lived (acquire+release within bucket_lock), but
   the GRQ lock acquisition in wake_up happens while bucket_lock is still held.

3. **Multiple waiter slot locks**: In futex_wake, multiple waiter slots may be
   locked/unlocked sequentially within a single bucket_lock hold. Each is independent
   (no nesting between waiter slots).

## Verification

SPIN/Promela models verify these invariants:
- `kernel/verify/spin/futex_wait_wake.pml` — no lost wakeup (INV-FUTEX-1)
- `kernel/verify/spin/lock_ordering.pml` — no deadlock (INV-LOCK-4)
- `kernel/verify/spin/interrupt_preempt.pml` — preempt_count balance (INV-PREEMPT-1)
- `kernel/verify/spin/sched_enqueue_dequeue.pml` — nr_running consistency (INV-SCHED-1)

Run: `make spin`

---

**Document Version**: v1.0
**Last Updated**: 2026-04-09
