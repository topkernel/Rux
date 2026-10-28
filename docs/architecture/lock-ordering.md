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

### Path 6: virtio-net device locks

```
VirtIONetDevice lock order (drivers/net/virtio_net.rs):

    {rx_queue | tx_queue} -> stats          (poll/xmit statistics update)
    rx_queue -> rx_last_used                (poll RX cursor)
    rx_queue -> rx_buffers                  (refill_rx_buffers)
    stats                                   (get_stats — never nested)

**INV-LOCK-6**: stats may nest inside rx_queue/tx_queue, never the
reverse — nothing may hold stats and then acquire a queue lock. The
poll() fast path once kept its stats guard alive across
recycle_rx_buffer() -> refill_rx_buffers() -> rx_queue (stats -> rx),
an AB-BA against another CPU's rx_queue -> stats; under sustained RX
(hdc file send) the watchdog flagged rx_queue (VIRTIO_NET+0x68) vs
stats (VIRTIO_NET+0xE8) spinning on both CPUs. The guard is now
block-scoped to the two counter updates.

## Invariants

| ID | Invariant | Enforcement |
|----|-----------|-------------|
| INV-LOCK-1 | preempt_disable must precede any spinlock acquire | Spinlock::lock() calls preempt_disable |
| INV-LOCK-2 | irq_save must precede preempt_disable when both needed | lock_irqsave() order: irq -> preempt -> lock |
| INV-LOCK-3 | Release order: unlock -> preempt_enable -> irq_restore | Guard Drop order |
| INV-LOCK-4 | No lock acquisition cycles (deadlock-free) | Hierarchy documented here, verified by SPIN |
| INV-LOCK-5 | GRQ nests inside bucket lock, never reverse | futex_wake path, verified by SPIN |
| INV-LOCK-6 | stats nests inside virtio-net queue locks, never reverse | virtio_net poll/xmit (drivers/net) |
| INV-LOCK-7 | leaf-PTE exclusion: vma_manager -> PTE section (global epoch + per-mm pte_lock) -> page_desc; global-epoch WRITE sections never nest inside ANY PTE section; per-mm PTE sections of the same mm never nest | mm/mm_struct.rs PteLock, arch mm_ops PTE_MODIFY_LOCK |

## Typed lock keys (the hierarchy as types, 2026-10)

`kernel/src/sync/lockkey.rs` gives the ordering classes of the table
above a TYPE surface: each class is a zero-sized key
(`GrqKey`, `FutexBucketKey`, `NetQueueKey`, `NetStatsKey`) and a
permitted nesting is an explicit `impl CanNestInside<Outer> for Inner`.
Acquiring a keyed lock while another keyed lock is held goes through
`KeyedRawSpinlock::lock_nested(&outer_guard)`, which only compiles when
that impl exists — INV-LOCK-5's reverse order (bucket inside GRQ) and
same-level nesting are compile errors, not deadlocks:

```text
impl CanNestInside<FutexBucketKey> for GrqKey      // INV-LOCK-5 forward
impl CanNestInside<NetQueueKey> for NetStatsKey    // INV-LOCK-6 forward
// reverse and same-class impls DO NOT EXIST — that absence IS the
// invariant
```

Migration is opportunistic (the D-6/D-4 doctrine): unkeyed spinlocks
keep compiling; a pair adopts keys when its invariant is named here,
and the adoption must not widen the subsystem's public API. Adopted so
far: the GRQ lock itself (`GlobalRunQueue.lock:
KeyedRawSpinlock<GrqKey>` — internal type swap only, the GrqGuard API
and the raw protocol around the lock are unchanged; merge of the sched/
side rides the scheduling-domain sign-off). Pending pairs: the futex
`HASH_HEADS` buckets (which completes INV-LOCK-5's compile fence: the
wake path's nesting becomes `grq_lock.lock_nested(&bucket_guard)`),
then the INV-LOCK-6 stats/queue pair. The unit group `lockkey` pins the
positive mechanics; the negative half is a compile-time fact (two
snippets that must not build are documented in the test).

The irqsave/preempt discipline (INV-LOCK-1/2/3) is deliberately NOT
keyed — that window is typed by CpuLocal's guard (divergence D-7); one
mechanism per concern.

## Per-mm PTE lock (leaf-PTE exclusion, 2026-10)

User-half leaf-PTE writers exclude through a two-level scheme that
replaces the former single global `PTE_MODIFY_LOCK` Spinlock (which
serialized every leaf-PTE write across ALL mms — the fork-walk CPU
monopoly of the SC-SESSIONS wall):

```
                    GlobalPteLock (arch mm_ops::PTE_MODIFY_LOCK,
                    RwSpinlock<()> — one static per build)
                   /                                       \
      SHARED (read): per-mm PTE sections        EXCLUSIVE (write): global
      = PteLock::lock() =                        = PTE_MODIFY_LOCK.lock()
        PTE_GLOBAL.read() + mm.pte_lock spin       the old single-lock
        (x86_64 split mode; cross-mm sections       semantics, whole-domain
        on different mms run CONCURRENTLY)          exclusion
```

Rules (INV-LOCK-7):

1. **x86_64 split mode**: every leaf-PTE writer that can reach the
   owning `MmStruct` takes `pte_lock_for(mm)` = the mm's `PteLock`
   (global epoch read + per-mm spin). Sites: demand/COW/swap faults,
   fork-walk parent chunks, brk/mmap/mprotect/mremap/mlock,
   rmap/compact (per-mm, one mm section at a time, mm Arc-pinned),
   ashmem, and mm teardown via the dying mm's own lock
   (uncontended by construction: last-Arc + synchronize_rcu drain).
2. **Global mode (riscv64, and x86_64 users without an mm)**:
   `PTE_MODIFY_LOCK` (GlobalPteLock) held exclusive. On riscv64 ALL
   PTE sites (arch files and `pte_lock_for` handles alike) map to the
   same global write — riscv64 keeps today's exact whole-domain
   serialization until its own split-mode migration.
   On x86_64 the remaining global users are: ipc/binder.rs +
   ipc/sysv_shm.rs installs (IPC line's files — excluded against
   per-mm readers by the epoch, no code change needed there),
   `map_user_region`/kernel-root maps (exec/fresh-table/device
   cold paths), and the unpublished-tree teardown handle
   (`FRESH_TREE_PTE_LOCK`).
3. **Nesting**: global-epoch WRITE never nests inside any PTE
   section (write-inside-read is a self-deadlock; write-inside-write
   likewise). Two per-mm sections may only overlap when they hold
   DIFFERENT mms' spinlocks (both hold the epoch shared). A path may
   never hold two sections of the SAME mm. These rules transfer the
   old single-lock deadlock-freedom: any nesting edge forbidden above
   would also have self-deadlocked on the former global Spinlock.
4. **Ordering position unchanged**: the PTE section sits where the old
   `PTE_MODIFY_LOCK` sat — after `vma_manager` (RwSpinlock, read or
   write), before `page_desc` locks. No site may take a vma lock
   while holding a PTE section.
5. Kernel-half table writes (copy_kernel_mappings at mm creation,
   boot-time alias installs) touch only fresh/unpublished tables and
   take no PTE section; they must never be moved under a PTE section
   held by the same task (rule 3).

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
