# sync-realign: lock primitives realigned with the reference kernel

Worktree: /home/william/rux-agents/sync-realign, branch feature/sync-realign
(off main c24e7397). Date: 2026-11-01. Charter: the "Design review vs the
reference kernel" section of /home/william/rux-agents/fix-lostpark/run/
lostpark-notes.md (binding).

Mission: eliminate the divergent "yielding spinlock" (the SMP lost-park
producer) by realigning with the reference discipline — rule 0 (I/O-holding
lock = mutex), rule 1 (spinlock never sleeps), rule 2 (queue-based wake-safe
sleeping slowpath), rule 3 (wait-path rechecks safe under spinlocks).

## R1 — the primitive

kernel/src/sync/mutex.rs: `SleepMutex` + `SleepMutexGuard`.

- Fast path: brief state-spinlock critical section (try_acquire_or_recurse
  variants); the state spinlock is held for instructions only, never I/O.
- Contended slowpath: park on a WaitQueueHead via prepare_to_wait
  (registration + state at the park site, EXCLUSIVE tail insert = FIFO),
  re-check under the state lock, own schedule(), finish_wait; release wakes
  the queue head (wake-one handoff token, reference mutex HANDOFF in
  miniature).
- Preserved battle-tested semantics, ported unchanged from the fs-era
  Ext4BigLock: task recursion (owner pid + depth, loop-device reentry),
  R84 fairness gate (fresh sleepable callers queue while waiters>0),
  R85 orphaned-handoff epoch + anti-starve self-wake timer (5 s), stall
  tripwires (3 s waiter / 10 s holder, rate-limited), vw_state /
  dfx_census_line diagnostics, release_if_owner (R73 exit-path force drop).
- IRQ context: forbidden (documented; the defensive early-boot /
  preempt!=0 pure-spin fallbacks from the fs-era lock are kept for
  behavioral compatibility).
- The diagnostic name is a field: ext4 passes "EXT4-BIGLOCK" so all
  stall/census print strings stay byte-identical to the old ones.
- fsw perf-gate counters (BIGLOCK/BIGLOCK_CONTEND) fire only for the
  ext4 instance (fsw_notes flag) so its census stays comparable.

## R2 — conversions (decisions)

Converted to SleepMutex:
- EXT4_BIG_LOCK (fs/ext4/mod.rs): the Ext4BigLock type + impl + guard
  (~670 lines) deleted; the static is now SleepMutex::new("EXT4-BIGLOCK",
  true). All 17 acquire sites renamed .lock_fair() -> .lock()
  (fs/ext4/mod.rs x12, fs/ext4/file.rs x4, fs/ext4/inode.rs x1). These
  are the I/O-holding loads: every ext4 entry point, holder may block in
  virtio I/O (rule 0).
- VFS_MUTATION_LOCK (fs/vfs.rs): was semaphore::Mutex (sleeping, wake-
  safe — compliant, but the wrong primitive family and non-recursive).
  Now SleepMutex::new("VFS-MUTATION", false); its 9 .guard() call sites
  compile unchanged (guard() alias). Sanctioned nesting recorded in
  docs/architecture/lock-ordering.md: VFS_MUTATION_LOCK -> EXT4_BIG_LOCK
  -> I/O (INV-LOCK-8). do_exit now force-drops it too (exit.rs, same
  R73 treatment as the ext4 lock).

Left on plain Spinlock (spin-briefly loads; rule 1 sections):
- Ext4BigLock's own state word (now SleepMutex's) — instructions only.
- SEM_IDS.slots and per-sem arrays (ipc/sysv_sem.rs) — the semop
  post-registration recheck runs under it; with the yield deleted the
  acquire is pure spinning, so the recheck window can no longer reach
  any schedule() (rule 3 restored; R3 verified by grep — no schedule()
  remains in spinlock.rs/rwlock.rs acquire paths).
- GRQ, futex buckets, waitqueue list locks, virtio driver locks,
  IpcIds, PENDING_REAPS, everything else taking Spinlock::lock().

Deleted divergence (R2 core):
- Spinlock::lock_fair() — had zero remaining callers (dead since the
  ext4 lock grew its own sleeping slowpath); deleted.
- Spinlock::lock()'s preemptible-yield branch + yield_no_park() —
  lock() is now preempt_disable + pure bounded spinning (spin until
  acquired; DEADLOCK_WARN only prints), like the reference qspinlock.
- RwSpinlock::read()/write()'s yield branches — these still called BARE
  schedule() (the unfixed lost-park bug class, worse than spinlock's);
  both now preempt-disable + pure spin via RawRwSpinlock. RwSpinlock
  users audited: signal action table, fs_struct cwd/root, mm vma_manager
  / raw / vma_files / saved_auxv — all short sections, none held across
  virtio I/O. NOTE for a future lane: mm's vma_manager sections can run
  long (unmap loops) — if the OH boots show new preempt-disabled spin
  pain, the reference answer is a sleeping rwsem there (rule 0), not a
  yield. Nothing observed in this lane's gates.

## R3 — sem path

No code change. Verified: try_apply_semops takes SEM_IDS.slots.lock()
and entry.inner.sems.lock(); both are Spinlock::lock(), which now
contains no schedule() and no state change (source-audited; the only
spin-family schedule() mentions left in sync/ are comments).

## Verify mirror

kernel/verify/src/sync/mutex_test.rs — new proptest mirror of the state
machine (mutual exclusion, fairness gate blocks bargers, token spend
takes free lock unconditionally, epoch recovers dropped handoff,
recursion depth/outermost release, stranger release inert). 6 tests,
1110 total green. scripts/verify_sync_check.py mappings unaffected
(compares only RawSpinlock try_lock/unlock/is_locked, unchanged).

## Gates (final numbers)

- Dual-arch clean-config builds rc=0 both (run/sr-build-rv.log,
  run/sr-build-x86.log); release-opt x86 kernel built for the stress
  rigs (build-release-opt, LTO).
- make test riscv64: 1278 passed / 0 failed (the VW-WEDGE lines after
  "All tests passed! Halting" are post-halt idle chatter, not a wedge).
- make test x86_64: ALL TESTS PASSED (run/sr-maketest-x86.log).
- make verify: green; the mirror crate is 1110 tests incl. the 6 new
  SleepMutex protocol tests.
- Contended-I/O wedge stress (run/sr-wedge.py, runs/srwedge-kvm1):
  6 min, x86 release kernel, q35 KVM -smp 2, private lmbench-x86 image,
  3 parallel churn loops (dd+cp+rm; variable dd+rm; creat/unlink/mkdir/
  rmdir by name — the inode02/ftest/creat09 shape). Verdict CLEAN:
  zero DEADLOCK/KERNPANIC/panic markers, zero stall prints, no serial
  silence; progress at kill time SEQ=45400 RAND=46300 NAME=34100
  iterations. (PREEMPT-HEAL lines = the known phantom-pc informational
  family owned by the preempt-hunt lane, unchanged by this work.)
- 2M semop ping-pong (run/sr-sem.sh): PROBE done 18.86 s / 2M rounds =
  9.43 us/round, child st=0x0 — same class as the fix-lostpark post-fix
  baseline (19-21 s, 9.2-10.4 us).
- Semop soak (run/sr-sem-soak.sh): 25 cycles, 25 OK / 0 bad, all
  9.4-11.3 us/round, every child exit state clean.
- LTP wedge-family spot check (test/ltpscan/run1.sh, private image,
  riscv64 TCG): ftest02 PASS, ftest04 PASS (was FAIL at the r21
  baseline scan), ftest06 PASS, creat09 PASS, sendmsg02 PASS (was
  POISON at baseline), inode02 = known baseline FAIL (rc=1 at r21);
  my isolated rerun hit the 650 s TCG timeout without wedging — serial
  stayed live, SIGTERM handled. No DEADLOCK prints anywhere.

## What is NOT done here

- mm vma_manager / fs_struct rwlocks stay spin-family (no I/O under
  them; long-section pain is a preempt-hunt-lane topic, noted above).
- semaphore::Mutex (sync/semaphore.rs) keeps its jbd2/ipc users; it is
  compliant (sleeping, wake-safe) — only vfs moved off it.
