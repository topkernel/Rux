# stress-port lane: port industry stress tests, find stability bugs

## Charter (user directive 2026-11-02)
"Supplement stress test cases — stability is still a problem. If the
industry has stress tests for the reference kernel, port them directly
and run them." PRIMARY vehicle: x86_64 + KVM (platform priority).
Goal: panics, wedges, leaks, and scheduler starvation exposed by
sustained load — then kernel-side fixes.

## What to port, in order
1. stress-ng (the industry standard, ~300 stressors). Build a static
   x86_64 subset against the toolchain already used for probes (see
   test/ build patterns; musl static preferred). Inject via the LTP
   vehicle in /home/william/rux-images/testimages/ (boot rig there
   already runs x86_64+KVM). Start with the classes our bug ledger
   says are weak:
   - memory: malloc/mmap churn (allocator class-lock family),
     mmapfixed, bigheap (zone exhaustion vs the fake-OOM history)
   - fork/exec churn (p4 regression history; exit-path RCU grace)
   - filesystem metadata churn (mkdir/rmdir/unlink storms — the OH
     chmod-storm shape; bio write-plan under load)
   - IPC: pipe/socket storms, SysV sem/msg/shm
   - sched: many-thread yield/wakeup storms (phantom preempt family,
     WAKE-STRANDED census)
   - timer/clock: settime/interval storms (tick discipline)
2. LTP robustness subset already in the tree (fork12, mmapstress,
   mmap-corruption, sem/msg stress) — run long-loop, not just once.
3. (later) syscall-fuzzer class (trinity-like) once 1-2 are clean.

## Method
- One stressor class per boot; serial console is the log.
- PASS = zero panics/wedges over the budget; FAIL = named bug entry in
  run/findings.md + minimal reproducer.
- Every kernel fix: x3 builds rc=0 + make test green + stressor rerun
  clean before push (origin/stress-port).
- Use the in-tree DFX under load: dfx=allocwatch (class-lock holder
  watch), syscost census bands, PREEMPT-HEAL counters — stability runs
  double as Tier-1-under-load evidence for the perf gate.
- Host discipline: at most 2 own QEMUs; -snapshot + file.locking=off;
  kill only your PIDs; pkill forbidden (pgrep -f self-matches: verify
  by exact cmdline).

## Session log (append per session)

### 2026-11-02, round 1 — rs suite ported, 2 kernel bug families fixed, 2 stressors re-run clean

**Ported** (stress-ng stressor logic as standalone static musl probes,
per charter method; sources archived in userspace/tests/probes.tar.gz
per the 394c16ce policy, host driver test/stressng/run-class-x86.sh):
14 classes — malloc/mmap/bigheap (memory churn), fork/exec, fsmeta
(metadata storm with read-back verify), pipe/sock/sem/msg/shm (IPC),
futex/sched (wakeup storms), timer (create/settime/delete + CLOCK_
MONOTONIC discipline + nanosleep latency), plus --spawn passthrough
for minimal reproducers. Vehicle: the x86_64+KVM LTP rootfs (copy of
the permanent-store image in run/, rs/rsrun/renameco injected via
debugfs); PID-1 rsrun runs cmds.txt lines with per-class budgets.

**Round-1 campaign** (baseline 819a1be2, one family per boot,
dfx=allocwatch,taskdump,watchdog + syscost=1, logs in
run/stress-runs/): ipc / sched / timer / memory PASS (0 panics, 0
wedges, 0 probe errors); fsmeta FAIL (SP-2); forker survived but lit
up the SP-1 storm. Verdicts + census numbers: run/stress-runs/
findings.md.

**SP-1 (fixed, verified)**: PREEMPT-HEAL false-fires stripped live
guard bits of fresh fork/exec tasks (15,577 heals + 169 underflows per
120 s of fork churn). Two producers: (1) stale_200's j==0 arm treated
never-stamped fields as instantly stale — fixed with arm-on-first-
observation (sched.rs); (2) new_task_at did not zero the four
ti_pc_*_zero_jif stamps on recycled Task pages — same class as R68a
(task.rs). Verification boot (fork 8 + exec 4, full 100 s budgets):
**0 heals, 0 underflows, 0 panics** (was 15,577/169/0).

**SP-2 (fixed, verified)**: ext4 setattr corrupted freshly-written
files — the success tail ran page-cache invalidate_inode for EVERY
attr flavor (chmod dropped dirty unflushed pages) and metadata-only
setattrs wrote back the disk-read inode's stale i_size (0 for pending
writeback). renameco minimal reproducer isolated chmod (3/300 fails,
reads returned EOF; rename alone clean). Fix: invalidate gated to
ATTR_SIZE, metadata setattrs carry the live VFS size (ext4/mod.rs).
Verification boot: renameco 0/300 on every sequence; fsmeta 4-worker
class full 120 s rc=0 (was: all 4 workers corrupt on cycle 1).

**Driver bug found+fixed in-flight**: rsrun killed classes at an
absolute 60 s (RES rc=137 truncation in the first campaign's memory/
forker boots); now parses --timeout per class.

**Tier-1-under-load evidence** (syscost, memory boot): IF=0 cpu0
17.6 s/60 s (rest 17.2 s, hard 408 ms, uart 3 ms), top rest-site
sched.rs:2568 (37.4M ops avg 834 ns); no ALLOCTHROW / DEADLOCK /
KERNPANIC classes in any boot. Follow-up hunts queued: IRQ-OFF-WINDOW
idle-wake prints 65-142 ms under load (pre-existing, guarded domain
interrupts); WAKE-STRANDED 118 s face under memory churn (SP-1
family; re-check on fixed kernel in round 2).

**Round 2 queue**: dirstorm-style shared-tree churn + fsync pressure;
pipe/socketpair under CPU contention (2 vCPU); LTP robustness subset
long-loop (fork12, mmapstress, mmap-corruption); brk-churn probe via
glibc-static (musl pins brk); make test + full LTP spot-scan on the
fixed kernel to guard the setattr/preempt changes.

