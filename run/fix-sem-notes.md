# fix-sem: SysV semaphore blocking race (lmbench lat_sem EAGAIN)

Worktree: /home/william/rux-agents/fix-sem, branch feature/fix-sem (off main 09d2969c).
Date: 2026-10-31.

## Root cause (confirmed — the located suspect was exactly right)

`kernel/src/ipc/sysv_sem.rs`, sys_semtimedop NEED_BLOCK path:

1. try_apply_semops() returns SEMOP_NEED_BLOCK for an op set that cannot
   proceed (no IPC_NOWAIT).
2. A concurrent post on the same set can land between (1) and the
   find_blocking_op() scan (separate lock sections).
3. The scan then finds nothing blocking → the old code did
   `None => return -EAGAIN`. The reference kernel NEVER returns EAGAIN
   for a plain blocking op — it re-runs the atomic apply and either
   consumes the post or blocks on the wait queue.

lmbench lat_sem (lmbench-master lat_sem.c) hits this every boot:
writer loops `semop([sem0:-1, sem1:+1], 2)`, reader loops
`semop([sem1:-1, sem0:+1], 2)`; the reader's sem0:+1 racing the writer's
scan turns the writer's blocking P into EAGAIN ("(w) error on semaphore:
Resource temporarily unavailable"), writer exit(1), reader sleeps
forever → case TIMEOUT.

Second latent trigger (no concurrency needed): find_blocking_op re-read
the RAW per-sem value for each op, while try_apply_semops accumulates a
working copy (each op sees the previous ops' effects). sops=[{-1},{-1}]
on value=1: try_apply NEED_BLOCK (second -1 blocks against working 0),
scan said "nothing blocks" → EAGAIN with zero races.

## Fix (kernel/src/ipc/sysv_sem.rs)

- find_blocking_op now mirrors try_apply's accumulated working-copy
  semantics (same first-violating-op verdict for identical state).
- NEED_BLOCK + "nothing blocks" now means "state changed / set deleted":
  `continue 'outer` — retry the apply (succeeds → 0; deleted → EIDRM)
  or re-scan and block. EAGAIN for a non-IPC_NOWAIT op is now
  unreachable (only IPC_NOWAIT ops, sub-jiffy timeouts, and semtimedop
  deadline expiry return EAGAIN, matching reference semantics).
- Refactor: the post-uaccess apply/block loop is split into
  semop_blocking_loop(), with pub(crate) semop_kernel(semid, sops) as a
  kernel-context entry (same validation), so the in-kernel test drives
  the exact production loop.

## Test (kernel/src/tests/sysv_sem_block.rs)

New group "sysv sem blocking contract" (5 rows), runs after ipc_sysv:
- IPC_NOWAIT P on 0 -> EAGAIN (EAGAIN belongs to NOWAIT only)
- V applied (GETVAL==1) — satisfiable ops still apply
- blocking P completes after post (no EAGAIN) — helper kthread waits
  GETNCNT==1 (proof the op really registered+blocked), then posts a
  real V through semop_kernel
- accumulated 2xP blocks then applies (no EAGAIN) — deterministic pin
  of the second trigger ([{-1},{-1}] on 1)
- Z-wait blocks until zero (no EAGAIN) — sem_op==0 path, released by
  semctl(SETVAL 0)

## Proof (lmbench lat_sem, x86_64 strong kernel, q35+KVM)

Rig: perf-lmbench's run-rux.sh flow; image reflink-copied from
/home/william/rux-images/testimages/lmbench-x86.img into work/, one-case
boot `lmbench=lat_sem`, init=/test/lmbench_run, kernel
`make -C build build-release-opt PLATFORM=x86_64`.

- -smp 2 BEFORE (work/bzImage-prefix, run/lat_sem-before.log):
  "LMBENCH TIMEOUT lat_sem" — writer killed at the EAGAIN.
- -smp 2 AFTER (work/bzImage-postfix, run/lat_sem-after.log): the
  EAGAIN death is gone (both processes ping-pong; no "(w) error on
  semaphore"), but the case still times out — see "SMP wedge" below.
- -smp 1 BEFORE (run/lat_sem-before-smp1.log): benchmp child dies via
  the same EAGAIN cascade → no latency line, empty DONE.
- -smp 1 AFTER (run/lat_sem-after-smp1.log): **"Semaphore latency:
  1.4004 microseconds"** — case completes end-to-end.
  (reference kernel on the same host/class: 0.4437 us, lmbench-linux.txt)

Direct ping-pong probe (run/semprobe.c, static musl x86_64, injected
into a reflink image copy, init=/test/semprobe):
- 5,000 rounds on -smp 2 AFTER: 4.6 us/round, steady, no degradation.
- 2,000,000 rounds on -smp 1 AFTER: completes, 3.56 us/round, final
  waiter counts clean (run/probe-smp1.log).

## SMP wedge (pre-existing, NOT this lane's bug — reproduced on the
pre-fix kernel)

On -smp 2, both before and after the fix, a two-process semop ping-pong
eventually wedges (after ~40k-250k rounds ≈ 0.2-2.5 s of traffic;
run/probe-prefix2m.log wedges at ~43k rounds BEFORE the fix,
run/probe-post2m.log at ~57k, run/probe-hang.log at ~106k AFTER — same
signature). Evidence (run/probe-dbg2.log + QEMU monitor capture):

- Wedged state: both tasks parked INTERRUPTIBLE, BOTH vCPUs in wfi
  (idle) — no spin. Values (v0=1, v1=0) satisfy the registered writer's
  P (ncnt0=1, entry on the set's wait queue, entry woken=false), i.e. a
  post landed whose wake was never delivered.
- The kernel's wake-heal scanner repeatedly tries both pids and skips
  them with why=c (curr_on): the parked tasks still carry stale on-CPU
  marks, so the heal refuses them too. Permanent.
- Morphology matches the documented stranded-slot/pick-skip repair
  families in sched/ (R46/R49/R56d/R74/R85 comments: parked tasks whose
  slot/on_cpu bookkeeping never cleared are unwakeable and the heal
  skips curr_on targets).

This is a scheduler lost-park defect exposed by any high-frequency
blocking IPC; the semop path itself is correct (slots-lock ordering
makes registration vs apply+wake totally ordered; every interleaving of
the sem code recovers). It needs a sched/ lane with the pick/slot
guardianship. Until then lat_sem cannot complete its full run at stock
-smp 2; at -smp 1 it measures 1.4004 us.

## LTP sem-family isolation (2x)

19 cases: semctl01-09, semget01/02/05, semop01-05, sem_nstest,
semtest_2ns, sem_comm. Results (run/ltp-sem-a.txt, run/ltp-sem-b.txt —
identical classes both passes):

- semop01, semop02, semop03, semop04, semop05: PASS x2 (the entire
  semop family green, incl. EINTR/EIDRM-under-block and the
  waiter/poster churn of semop05).
- semget02, sem_nstest, semtest_2ns, semctl02/03/04/05: PASS x2.
- Pre-existing FAILs (identical on the pre-fix kernel,
  run/ltp-sem-prefix.txt — FAIL=5 PASS=12 TCONF=3 before AND after):
  semctl01 (1 subtest), semctl06, semctl07 (1 subtest), semget01
  (sem_nsems readback), sem_comm (ns semctl EINVAL). Unrelated to the
  blocking path; left for the semctl/semget lanes.
- TCONF x3: semctl08 (time_high fields), semctl09 (SEM_STAT_ANY),
  semget05 (/proc/sys/kernel/sem absent) — environment gaps.

## Gates

- make test (riscv64, unit-test): 1272/1272 PASS, rc=0 (base suite on
  this tree was 1267 rows; +5 new contract rows). Final clean-tree run:
  run/maketest-final.log.
- make build (riscv64) rc=0; make build-x86 rc=0 (final clean tree,
  run/build-rv-final.log / run/build-x86-final.log).

## Notes

- The old `None => EAGAIN` also mis-surfaced EIDRM (deleted set) as
  EAGAIN; the retry now surfaces EIDRM through try_apply, matching the
  reference outcome for a set removed under an in-flight op.
- Evidence files kept under run/: lat_sem-before*.log, lat_sem-after*.log,
  probe-*.log, semprobe.c, maketest-*.log, ltp-sem-*.txt.
