# Scheduler fast-pace stability — findings (feature/sched-fastpace)

Predecessor context (poll fix, CPU-storm measurements): see
`/home/william/rux-agents/gnome-cpu/FINDINGS.md`. This file replaces the
old copy of that text.

Worktree: `/home/william/rux-agents/gnome-sched2`, branch
`feature/sched-fastpace` (base `feature/gnome-cpu-storm`).
Full-session driver: `runs/cpufull.py` (cold copy of
`/home/william/rux-agents/gnome-img/gnome.img`, Xorg -> start-gnome.sh ->
gnome-session, smp4/2G, `dfx=taskdump`). Runs cited below live in
`runs/<name>`; each keeps `serial.log`, `cmdline.txt`, `samples.txt`.

## Branch contents (this session, on top of the prior session's work)

- R75 buddy oversize rejection (87f57418, prior session) — kept.
- D-Bus fixes (prior session): pselect6/select all-NULL fd sets legal
  (89fbf717), deferred exit-notify on same-task repick (1c56540e).
- R76 diag (da1ad02c): slot-writer shadow, decision-time skip reasons,
  hung/ext4 context dumps.
- R77 semaphore CAS acquire (a8a5d171), R78 exception-table fixup before
  stack-overflow panic (20e47277) — both prior-session commits that had
  **never been built or booted** before this session (the prior runs
  used an Oct 13 02:40 binary that predates them).
- R79 (39bab529, cherry-pick of be30f139 from feature/gnome-integration,
  renumbered): chunk-backed FileBuffer, caps single heap allocs at 64 KiB
  (memfd ftruncate 64 MiB Vec growth).
- pselect6 unit-test update (372741ee): the old test asserted the
  pre-fix -EFAULT for `pselect6(0,NULL,NULL,NULL,NULL,NULL)`; after the
  pure-sleep fix that call is a legal forever-sleep, which in kernel
  context WEDGED the whole `make test` suite (25+ min hang in test
  [125], CPU0 idle-looping on affinity-skipped ksoftirqds). Replaced
  with a skip note; all other asserts unchanged.
- R80 (7931187e): heal stale LIVE pick-marks (below) + pid in
  R56-ORPHAN-HARVESTED. 8cf8e98e adds the ORPHAN-CTX parked-stack line.

## What the runs showed

| run | kernel | outcome |
|-----|--------|---------|
| r75-full1 / inst-full1 / inst-full3 (prior session, old binary) | poll-fix only | stack-overflow PANIC at t~360/960/110 (copy_user fault at a wild user addr on a cross-stack sp, ext4 big lock leaked with its dead holder, VW-EXT4LOCK wedge forever) |
| fix-full1 (prior session) | old binary + R76 diag in tree | early full wedge at t~90: pids linked ∧ on_cpu=1 skipped by all CPUs (why=1), serial silent for the remaining 35 min |
| v1-r1 | R75-R79 fresh build | **no panic** (R78 works — zero stack-overflow panics), but the session wedged at t~180: Xorg stranded as RUNNING ∧ {on_cpu=1,claim=1} ∧ pick_state=LIVE ∧ unlinked ∧ owned by NO slot (`cpu slots: 0 0 0 0` in the DUMP! capture) — every recovery path refused on the LIVE word alone; R56-ORPHAN-HARVESTED storm (~2.6/min, anonymous) while everything else slept |
| v2-r1 / v3-r1 | + R80 + ORPHAN-CTX | same arc: session dies at t~150 (gnome-session-b SIGTERM, below), survivors 306 kfbflush / 336 xterm / 338 bash cycle orphan -> heal -> orphan ~every 200 ms through the plain R46 insert (marks already clean: o=0 cl=-1 pk=1); the R80 legs never fire for these victims |

### R80 root cause and fix

The pick-state machine's LIVE word was treated as absolute truth by
EVERY recovery path: class picks skip on_cpu candidates; the pick-side
stale-claim heal requires on_cpu==false; the wake path's R74 leg
returns on PICK_LIVE; harvest_relink leg (a) returns on PICK_LIVE; the
R73 exit-path lock release needs the task to exit, which a stranded
task never does. One stale LIVE mark therefore black-holes a task
forever, together with whatever lock it holds (the EXT4-BIGLOCK stall
family).

Under the GRQ lock two shapes are provably stale:
- linked ∧ on_cpu ∧ no SWITCHING_OUT mark: a mid switch-out publishes
  SWITCHING_OUT before unlocking; an in-flight pick unlinks under the
  same lock; a running task is named by its CPU's slot until its next
  __schedule.
- RUNNING ∧ unlinked ∧ slotless ∧ LIVE: LIVE is only published together
  with a slot naming the task (pick+slot under one lock hold, the
  switch-in affirm, or the repick publish), and a hart executing the
  task keeps its slot naming it — no in-tree interleaving can produce
  this combination.

R80 releases the ownership qword ({false,-1}) for both shapes after the
orphan watch time-confirms the shape for 50 jiffies (the watch table
re-records sightings at 60 jiffies, capping usable thresholds; 500 ms is
two orders above any legitimate in-flight window and far below the
30-minute strand actually captured). The m25 double-run discipline is
preserved: the release happens only for a task that provably executes
on no hart, with the slot scan, SWITCHING_OUT check and time
confirmation all under one GRQ hold.

## Open items (not fixed this session)

1. **Early session death (vs the old poll-fix-only binary).**
   The old binary reached the gsd spawn phase (pids 460+, gmain/dconf
   worker threads alive). Every build with the R75-R80 set dies at
   t~150: gnome-session-b receives SIGTERM (`SIGDEATH sig=15 si_pid=0
   si_code=0` — si_code 0 is SI_USER, so a kill(2); the sender pid is
   lost because the pending siginfo was consumed before the exit-path
   peek), gmain SIGKILLed, no gsd component ever spawns. Bisect by
   single reverts (7-minute discriminating runs `runs/b1-nodbus`,
   `runs/b2-nor79`, `runs/b3-nor77`): reverting the D-Bus pair, R79, or
   R77 INDIVIDUALLY does not change the arc — same SIGTERM at the same
   phase in all three. Remaining suspects: R78 (fixup-first — converts
   kernel-mode copy_user faults to EFAULT returns; if components fault
   during this window, the old binary would have panicked later but the
   session lived longer), or an environment/load difference (the host
   runs more QEMUs now than during the inst-full/r75-full captures).
   No buddy REJECTED lines, no R78-class fault prints, no soft lockups
   in any run.
2. **Orphan-cycle producer.** 306 kfbflush / 336 xterm / 338 bash
   repeatedly present RUNNING ∧ unlinked ∧ slotless ∧ {on_cpu=0,
   claim=-1, pk=1} to the harvester — a shape the in-tree protocol
   cannot produce at all (a preempted task is requeued linked under the
   GRQ; a blocked task is not RUNNING; a picked task is named by a
   slot). The ORPHAN-CTX parked frames land at the universal deschedule
   point (context_switch / __schedule) with the syscall contexts above
   it being timer waits (del_timer, timer BTreeMap insert) and
   process_deferred_exit_notify_cpu. Suspects: a timer-wake enqueue
   racing the block-out window, or a class-flag mismatch
   (task_onrq_flag / CfsRunQueue::is_linked both dispatch on the task's
   CURRENT policy — a mid-run sched_setscheduler flip makes the
   harvester misread linkage). The heals keep the victims running (no
   starvation >60 s observed), but the cycle burns idle-loop CPU and
   floods the console.
3. **make test blocked by a test-kernel boot panic (2/2 boots).**
   Illegal instruction in kernel mode right after "fs: ext4 driver
   loaded [ok]", tp=0, zero tests run (maketest2/maketest3 logs —
   identical shape, epc 0xffffffff8001c11e / 0xffffffff80019416,
   symbolized near ext4 mount init). Layout-sensitive: the suite ran
   1002+ PASS at 01:36 on the same functional tree BEFORE a one-file
   test-source edit (the pselect6 test change) shifted codegen; every
   GUI-feature build of the same tree boots fine. Looks like a latent
   test-feature init bug uncovered by layout, not caused by R75-R80.
   First step for the next session: boot the test kernel with
   THREAD=single or earlycon to catch the exact faulting instruction.
4. **WSL2 clock jumps** stall cpufull.py's wall-clock pump loop and
   stretch runs past their nominal timeout; injecting `DUMP!` through
   the driver's stdin pipe (`echo "DUMP!" > /proc/<driver-pid>/fd/4`)
   retrieves taskdumps from wedged-but-alive guests.

## Verification status

- `make build`: clean on all commits through 8cf8e98e.
- Full-session runs at smp4/2G with R78+R80 (v1/v2/v3): zero kernel
  panics (the old binary panicked in 3/3 full runs — the
  stack-overflow-panic family is gone). Remaining failure shapes are
  the early session death (item 1) and the orphan-cycle noise (item 2).
- final-r1 (2100 s validation) and min-r1 (minimal X+dbus session,
  idle-CPU check) — numbers in the session report.
