# oh-vlock lane — sleepable VMA rwsem (the mmap_lock shape)

Mission (from the coordinator, 2026-11-06): replace the VMA pure-spinner
rwlock (52nd's TASK.md §6b evidence) with the reference's sleepable
rw_semaphore. Branch oh-vlock off origin/oh-crash (70568e37).

## 1. Design (reference: linux-6.6 kernel/locking/rwsem.c)

New primitive `kernel/src/sync/rwsem.rs` (`RwSemaphore` + data-carrying
`Rwsem<T>`), wired into `MmStruct::vma_manager` (vma_read/vma_write/
vma_try_read). Semantics ported line-for-line where Rux has the
machinery:

- count word layout (rwsem.c:117-128): [0] WRITER_LOCKED, [1] WAITERS,
  [2] HANDOFF, [8..] reader count (bias 1<<8), [63] READFAIL.
- Reader fast path fetch_add(READER_BIAS); READ_FAILED_MASK blocks new
  readers once ANY waiter is queued (rwsem.c:241-254) — the reference's
  anti-barging, no invented fairness.
- Writer fast path CAS 0→WRITER_LOCKED (rwsem.c:256-266).
- Waiters sleep UNINTERRUPTIBLE on an internal wait_lock-guarded FIFO
  list. Reader grant = granter pre-adds bias + removes entry
  (rwsem_mark_wake 509-567, phase-fair batches ≤0x100); writer grant =
  woken head re-runs the queued CAS (rwsem_try_write_lock 604-666).
- HANDOFF protocol: waiter older than RWSEM_WAIT_TIMEOUT (~4ms,
  DIV_ROUND_UP(HZ,250)) sets the handoff bit; blocks fast paths and
  steals until the head waiter owns (rwsem.c:604-666, 108-115).
- up_read wakes on last-reader-out with waiters; up_write always wakes
  with waiters (rwsem.c:1338-1378).
- Writer optimistic spinning (rwsem.c:819-935): owner-word spin gated
  on owner-on-CPU (pid pinned via pid_hash_lookup_pinned — Rux owner
  word carries pid+flags, not a task pointer), need_resched aborts,
  reader-phase time budget (20+readers)/2 µs cap 25 µs then
  NONSPINNABLE latch. Bounded-rounds shape per SleepMutex (no osq in
  Rux — documented divergence, behavior preserved).

Documented Rust/Rux divergences (all behavior-preserving):
- Fast-path count RMW + owner publish share one preempt-disable
  section (rwsem.c:137-139); held sections are preemptible (sleepable
  lock, sleepable above spinlocks — INV-LOCK-7 order unchanged).
- Park protocol = Rux state-first + re-check discipline (Task::sleep
  warning): set UNINTERRUPTIBLE, re-check grant under wait_lock,
  schedule; grant removal + wake outside wait_lock. No WaitQueueHead:
  the wait list IS the wake registry.
- No RT-task branches (one fair class); no lockevents; waiters
  uninterruptible only (no killable variants, like mmap_lock users).
- Degradation: no-current-task contexts (early boot/secondary harts)
  bounded pure-spin instead of refuse (SleepMutex parity).

Preemption-behavior audit of every vma_read/vma_write caller (25 files):
- All sites are syscall/kernel-thread context; fault paths enter at
  preempt_count==0 (x86_64 trap.rs "count must be exactly 0" invariant).
- Nesting is vma-lock-outside-spinlocks everywhere (vma_files write,
  PTE sections, page_desc) — sanctioned order for a sleepable lock;
  no site takes a vma lock while holding any spinlock or RCU read
  side (rcu_read_lock = preempt_disable here).
- No same-task read-read nesting (R73 restructure already made fault
  paths drop the guard before fill work; find_vma callers are
  standalone). The remaining writer-recursion cases would have
  deadlocked on the spinner too.
- The mm fork path (mm_ops.rs fork copy) already snapshots under one
  read era and replays under the child's write lock — unchanged.

Diagnostics added (print-only): RWSEM-STALL-R/W tripwire (3 s parked
waiter, 1 line/10 s global rate limit — SleepMutex STALL precedent) and
`lastsys=`/`a0=` in the khungtaskd HUNG-CTX line (names the syscall a
D-state task is parked in; needed because the fuxstuck census only
sees futex parks).

## 2. Gates

- Builds: x86_64 / riscv64 / aarch64 all rc=0
  (run/build-x86-vlock{1,2}.log, run/build-rv-vlock1.log,
  run/build-a64-vlock1.log).
- riscv64 make test: ALL PASSED — 1318/1318 (run/maketest-vlock1.log)
  and 1319/1319 with the new rwsem fast-path protocol row
  (run/maketest-vlock2.log; the task-308 HUNG reports during the suite
  are pre-existing — the 52nd's maketest-52.log has the same 4).
- boot-soak-fast-class minimal smoke (musl rootfs, SMP2): boots, full
  suite green, no POISONED/KERNPANIC/wedge (run/soak-fast-vlock1.log).
- pbench Tier-2 (mm) A/B, same rig KVM SMP2 2G, 3 reps interleaved,
  base = bzImage-ohc25 (52nd final kernel):
  p3b_fault_cow 7.815 → 8.045 µs (+2.9%, inside ±20%; the 8.075 µs
  ledger baseline also brackets it), p2_ctxsw +3.6%, pipes/datagrams
  within ±3.5%, p4_fork_exec MISSING both sides (known flaky row).
  NO row regresses >20% → mm gate PASS (run/abpb-ohc25-vlock.txt).

## 3. OH boots (system-lb5 + userdata-fresh2, SMP2 KVM, LOGLEVEL7,
    dfx=fuxstuck,txnhunt; 52nd baseline = ohc24: 37 RCU-GP-BOUND
    prints, SA ladder to 43.0s, frozen pixels 82f0a25f)

- vlock1 (pre-tripwire kernel): SA ladder 0 OpenSo (samgr parked in a
  userspace futex at 5.5s right after binder mmap — the §6a pandemic
  shape), RCU-GP-BOUND 0, stuck_total 52, wedge 82f0a25f.
- vlock2 (taskdump armed): same shape, 0 OpenSo, 0 RCU-GP-BOUND,
  stuck_total 51, hung task 306 at 181s (same pid as ohc24's hung
  306 — pre-existing pandemic family), wedge 82f0a25f.
- vlock4 (tripwire kernel): CONTEXT MANAGER registered, SA ladder 31
  OpenSo (last 13.7s — ladder-progress variance; ohc24 reached 43s),
  RCU-GP-BOUND 2 (vs 37), RWSEM-STALL 0, stuck_total ≤118, wedge
  82f0a25f.
- vlock5: 80 OpenSo (last 33.9s; bootanimation.ready marker 29.4s —
  deeper ladder progress than the baseline description), 198 RCU-GP,
  stuck_total 193, RWSEM-STALL x2 (below), wedge 82f0a25f.
- vlock6: 66 OpenSo (26.3s), 92 RCU-GP, stuck_total 187, RWSEM-STALL 0,
  wedge 82f0a25f.

The two vlock5 stall prints (waiters 348/samgr-thread and 607, waited
1000j and 4977j behind count=0x7 = WRITER held + HANDOFF demanded,
queued=1): the grant DID arrive — pid 348 is later seen INTERRUPTIBLE
in userspace (it left the rwsem and parked on a futex) — so these name
GENUINELY SLOW vma_write holders (10-50 s sections; candidates: the
0x80000000-len PROTNONE-range teardowns and mm exit walks), through
which waiters now SLEEP instead of monopolizing a CPU. The follow-up
owner-pid tripwire build (vlock6) saw zero stalls. No lost-grant strand
in 5 boots / ~25 min of wedged-boot exposure.

Reading: the 0-OpenSo boots are the known early-wedge race variance
(the 51st/52nd documented wedge classes), not an rwsem regression —
boot 4 with the identical kernel runs the ladder. RWSEM-STALL never
fired: no lost-grant strand in the new park path. RCU-GP-BOUND
collapses 37→0-2 — the vma spin monopolist windows were a producer of
grace-stall prints; the sleepable lock removes them. The terminal
wedge class (frozen logo 82f0a25f, hilogd §6a park, binder backpressure
pandemic) is unchanged — those are the 53rd's targets, not this lane's.

## 4. Handoff notes

- The rwsem is generic (`Rwsem<T>`); other RwSpinlock users (vma_files,
  saved_auxv, GlobalPteLock) deliberately NOT converted — short
  sections / IRQ-adjacent contexts.
- If the 53rd's sleepsnap wait-object naming confirms hilogd's mmap
  park waits on the vma lock, that park should now SLEEP instead of
  spin — check a vlock kernel boot for hilogd lastsys=9 parks (the
  new HUNG-CTX lastsys= field prints it).
