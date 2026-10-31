# Review findings — proc-sched lane (rv-proc-sched)

Date: 2026-10-30/31. Scope: kernel/src/process/** + kernel/src/sched/**,
watch items: lifecycle races, zombie/reap semantics, wait queue discipline,
signal delivery ordering. Line-by-line against reference-kernel contracts,
re-derived from man-page semantics + in-repo R-numbered comments before
fixing. Gates per fix: reference contract / three builds rc=0 / no
upper-layer breakage. scripts/review_coverage.py untouched.

Builds (final tree, all three rc=0): `make build` (riscv64),
`make build PLATFORM=x86_64`, `make build PLATFORM=aarch64` — artifacts
verified newer than the last source edit.

## FIXED (committed)

### F1. do_waitid / do_wait_nonblock released zombies without claiming (double-reap race)
`kernel/src/process/exit.rs` — commit fe70d819 (amended acb8b3af lineage).

`do_wait` selects a zombie under PROCESS_TREE_LOCK (inside
`for_each_group_child`) and CLAIMS it there (`Task::claim_child_locked`,
the E8-REPAR fix) before calling `release_task` after the walk.
`do_waitid` and `do_wait_nonblock` never claimed: two thread-group members
woken by the same child-exit event (glib reaper thread + main, or two
concurrent WNOHANG pollers) could both select the same zombie — the state
stays ZOMBIE until release runs — and both pass `release_task` on the same
Task: second pass locks/unlinks/frees through freed memory (double free of
kernel stack, PID, Task slot).

Fix: identical claim discipline inside the selection walk.
- do_wait_nonblock: claim inside `for_each_group_child`; failed claim keeps
  scanning. The EFAULT copy-failure path now releases the claimed zombie
  before returning EFAULT — a claimed child must be reaped by the claiming
  call or it is unreapable (claim = unlink), which forces the divergence
  from the reference's unreaped-EFAULT (documented in code).
- do_waitid: claim only when the zombie will be reaped (not under WNOWAIT,
  which must leave it reapable). Selection made first-event-wins
  (`result_child.is_none()` guards on both zombie and stopped branches,
  matching the reference's wait_consider_task walk-stop) so a later match
  cannot overwrite — and strand — an earlier claimed zombie.

Gate evidence: reference kernel_wait/do_wait walk under tasklist/siglock
with release in the same critical region; here the claim is the equivalent
serialization point. Builds x3 rc=0. Guest suite wait4/fork/scheduler
tests exercise these paths (suite run below).

### F2. waitid id validation + pid-namespace reporting
`kernel/src/syscall/process.rs` (+ exit.rs si_pid) — commit 4053b176.

Reference `kernel_waitid` returns EINVAL when `P_PID` gets id <= 0 or
`P_PGID` gets id < 0; here those fell through to ECHILD. Also waitid lacked
the U1c pid-namespace handling wait4 already has: P_PID targets are now
resolved caller-ns → global (ECHILD when invisible), and the reported
si_pid is translated back at write time (child is still hash-resident
before release_task — verified ordering). POSIX: waitid(2) EINVAL for
invalid id.

### F3. do_exit_group: duplicated group-exit-status publication
`kernel/src/process/exit.rs` — folded into commit fe70d819.

The "publish group_exit_code/group_exiting" block (comment included) was
pasted twice back-to-back; the second pair of stores was pure redundancy.
Removed one copy (rode along in F1's commit, noted in its message).

### F4. fair.rs pick_next_cpu: duplicated timeline_remove on the stash path
`kernel/src/sched/fair.rs` — commit 61ea0152.

A copy-paste artifact: `self.timeline_remove(&key)` (+ its comment) appeared
twice on the not-allowed stash path. The second remove was a provable no-op
(BTreeMap::remove of an absent key; leftmost cache already invalidated by
the first), so no correctness impact — dead duplicated code on the
GRQ-held pick hot path. Removed.

### F5. do_wait stopped-child selection: last-match → first-match
`kernel/src/process/exit.rs` — commit 61ea0152.

`stopped_child` had no `is_none()` guard (zombie_child had one), so with
several stopped children the LAST was reported instead of the first —
inconsistent with do_wait_nonblock and the reference (walk stops at the
first child yielding an event). One-shot semantics unchanged (each stop
reports once either way).

## FILED (not fixed — needs its own change + tests)

### P1. execve discards pending signals (reference preserves them)
`kernel/src/process/exec.rs` (~line 169): `(*task_ptr).pending.clear()`.

POSIX execve/`man 2 execve`: pending (blocked, undelivered) signals are
PRESERVED across execve; only caught dispositions reset (which the code
correctly does via `flush_handlers`). The reference aborts the exec with
-ERESTARTNOINTR when a FATAL signal is pending, otherwise the pending set
survives and is delivered per the new image's dispositions. Here every
pending signal — including a SIGKILL racing the exec — is dropped, so a
`kill -9` landing in the window between signal-send and exec's clear is
lost and the target keeps running the new image. Rare in practice, real
contract divergence. Not fixed in this lane: flipping clear→keep changes
the exit/exec state machine's assumptions (the clear is tangled with the
H15 sigframe reset) and needs targeted LTP coverage (e.g. a pending-signal
exec test) before landing.

### P2. do_waitid does not walk the ptrace tracee list
`do_wait` scans tasks with `tracer_pid == current` (ptrace11: waitpid on an
attached non-child); `do_waitid` has no equivalent. Reference
`ptrace_do_wait` applies to waitid too. Cosmetic for current LTP set (no
waitid+ptrace test on the list); fix = factoring the tracee walk both
callers share.

### P3. waitid WCONTINUED is a no-op
Si_status for continued children is unimplemented (marked "future
enhancement" in-tree). POSIX optional; WCONTINUED requests would block
until a stop/exit event instead. Filed for the SIGCONT bookkeeping lane.

### P4. P_PGID in waitid is not namespace-translated (wait4's pgid paths neither)
U1c covers P_PID for waitid now; pgid comparisons still run on global pgids.
Only visible with nested pid namespaces using process-group waits. Filed
with the namespace lane's remaining pgid work.

## FALSE-POSITIVES RETRACTED (suspected, then re-derived and dropped)

### R1. WaitQueueHead::wake_up nr semantics differ from __wake_up nr_exclusive
Suspected defect: the reference counts ONLY exclusive entries against
nr_exclusive (non-exclusive waiters are all woken plus one exclusive);
here `awakened` counts every entry toward `max_wake`, so `wake_up_one`
wakes exactly one entry of any kind. Re-derived: every in-tree caller uses
either `wake_up_all` (nr=0, matches reference wake_up_all) or
`wake_up_one` (nr=1, users: UART read queues only — console.rs x3), and no
queue in the tree mixes exclusive and non-exclusive entries where the
reference's split matters. No userspace-visible divergence on any ABI
surface. Not changed (changing it would alter UART wake behavior for zero
contract gain).

### R2. do_wait ignores copy_to_user failure for the status word
Suspected: reference returns EFAULT (put_user before release, child NOT
reaped). Re-derived: here the child is CLAIMED (unlinked) before the copy;
returning EFAULT without reaping strands an unreapable zombie (no list
links it anymore — the claim's own doc states this invariant). The current
best-effort write is the correct behavior under the claim design, and
sys_wait4's access_ok pre-screen catches the common bad-pointer cases.
do_wait_nonblock (which does report EFAULT) now releases-then-reports
(F1). Retracted as a defect for do_wait; the divergence is documented in
F1's comment.

### R3. sched_clock hardcodes a 10 MHz timebase
`fair.rs sched_clock()`: `read_time() * 100`. Suspected wrong for hosts
with a different timebase. Re-derived: QEMU riscv64 `timebase-frequency`
is 10 MHz for the virt machine (the only supported riscv64 target), x86_64
TSC is normalized by the arch's read_time, aarch64 CNTFRQ handled by its
arch read_time. Correct for all three targets as configured. (If real
hardware lands, this needs a per-arch freq — not this review's surface.)

## Reviewed with no findings (notable surfaces)

- fork.rs: R52-checked enqueue with exact unwind (thread + process paths),
  CLONE_PARENT_SETTID/CHILD_SETTID split (parent-mm vs child-mm writes),
  vfork registration before enqueue. Clean.
- exit.rs: robust-list/clear_child_tid before mm drop, thread-group
  split (park-before-leave ordering), deferred exit-notify, reparent with
  pins + atomic move_child, TREE-RELEASE-HEAL. Sound beyond F1/F3/F5.
- task.rs: task_refcnt/try_pin discipline, mm_slot_lock Arc swap,
  publish/release pick-ownership qword protocol (layout-asserted), stack
  cache disabled-with-rationale. No new defects.
- sched core (__schedule/pick_next_task/enqueue/wake): poison/DEAD-pick
  gates, R41/R42/R85 heal machinery, RR rotation only for RUNNING tasks
  with paired nr_running, CFS tick uses activation-length vs slice
  (check_preempt_tick shape), min_vruntime advanced over the COMPLETE
  timeline post-stash. No new defects beyond F4.
- rt.rs/deadline.rs: inverted-bitmap priority (PROC-P06 fix verified),
  checked-sub counters, double-enqueue guards, stash-and-reinsert under
  GRQ. Clean.
- pid.rs/pid_hash.rs/kthread.rs/stats.rs/cgroup.rs hooks: clean.
  (pid_hash_remove's raw head write under bucket lock + Acquire readers
  is mixed atomicity but lock-serialized and aligned — target-safe,
  noted only.)
- wakewatch.rs: DFX instrumentation, gated — skimmed, out of hot path.

## Verification

- `make build` / `PLATFORM=x86_64` / `PLATFORM=aarch64`: all rc=0 on the
  final tree (post-61ea0152), binary mtimes verified newer than edits.
- Guest unit-test suite (`make test`, riscv64, includes tests/wait4.rs,
  tests/fork.rs, tests/scheduler.rs, tests/syscall_sched.rs): see
  maketest-verdict.txt next to this file for the recorded result.
