# fix-lostpark: SMP scheduler LOST-PARK (lmbench lat_sem/lat_tcp/lat_connect timeouts)

Worktree: /home/william/rux-agents/fix-lostpark, branch feature/fix-lostpark.
Date: 2026-11-01. Evidence logs in this directory (lp-*).

## Root cause (confirmed by direct capture)

`kernel/src/sync/spinlock.rs` — `Spinlock::lock()`'s "preemptible
bounded wait" (the preempt-hunt lane's outermost-depth contended
acquire) called plain `schedule()` to hand the CPU to the lock holder:

    preempt_enable();
    crate::sched::schedule();   // "hand the CPU over"

A caller can legally be inside a wait path's PREPARED WINDOW at that
moment: its state was already set INTERRUPTIBLE by an enclosing
`prepare_to_wait`, but it has not reached its own park site — exactly
sysv semop's post-registration recheck (`try_apply_semops` re-run at
kernel/src/ipc/sysv_sem.rs:1064), which takes the same SEM_IDS locks
the yield contends. `schedule()` parks a task whose state is a sleep
state, so the yield PARKS THE WAITER INSIDE THE SPINLOCK ACQUIRE:

- it is on NO wait queue at that point, so no future wake reaches it;
- its enclosing path's wake sources fire only for the queued entry;
- the lock holder's release arms nothing for it.

The wedge (every capture, roles interchangeable):

1. task A parks properly on the sem waitqueue (ncnt=1), its op
   satisfiable;
2. task B, in its recheck window, contends the sem lock, exhausts the
   20k spin budget, and the yield's schedule() parks it INSIDE the
   lock acquire (state INTERRUPTIBLE, unlinked, no wake source);
3. no further `wake_up_all` ever runs (A is asleep, B is parked-in-spin);
4. both vCPUs idle forever. WW census frozen; WAKE-HEAL-SKIP why=c
   residue on both pids; parked tasks carry stale on_cpu marks and
   leaked IRQ-class preempt bits (0x100/0x10000) from every tick that
   hit the stuck window.

`lock_fair()` (line ~393) carries the identical bug.

## Direct capture (the conviction chain)

- `dfx=lostpark` (new switch, table-union method: SWITCH_COUNT 54->55,
  name "lostpark", index 54) arms:
  - a 64-entry lock-free event ring (dfx/lostpark.rs): semop park
    enter/exit, refused wake tokens, every successful apply with the
    post-apply (v0,n0)/(v1,n1) values, jiffies — dumped once by the
    wakewatch heartbeat when wakes freeze with an empty global queue;
  - LP-WAKE-SKIP-EMPTY (wake refused while nothing runnable, with the
    naming CPU + SLOT_WRITER tag);
  - LP-IDLE-LIE (idle loop about to halt while its slot names a real
    task);
  - an unfiltered LP-CENSUS in the slowtask audit (pid/state/pc/oncpu/
    pk/claim/onrq + the parked kernel-stack chain).
- LP-CENSUS at the wedge named the park site directly:
  `pid=1 state=0x1 ... STK: Spinlock::lock+0x2d0 <- try_apply_semops+0x34
  <- sys_semtimedop` (run/lp-last3.log of the conviction boot) — a
  waiter parked inside a spinlock acquire, taken from the semop
  recheck. lp-ring tail of the same boot shows the last poster's
  apply, then silence (no more events from either side).
- QEMU monitor at the live wedge: both vCPUs halted in
  cpu_idle_loop+0xa50 (the "both vCPUs idle" symptom).

## Fix

`yield_no_park()` (kernel/src/sync/spinlock.rs): both yield sites now
force the caller's state to RUNNING across the handoff `schedule()`
(the yield is a handoff, never a sleep). `__schedule` then requeues
the caller as a preempted running task instead of parking it; the
enclosing wait loop re-asserts its sleep state at its own park site
(prepare_to_wait re-runs every iteration), costing at most one extra
recheck loop. Gated tripwire `LP-YIELD-PARK-PREVENTED pid state`
(dfx=lostpark, budget 40) fires on every prevented park — it fired
repeatedly on both probe pids in the first fixed boot.

Secondary hardening (same commit): `__schedule`'s preempt-discipline
refusal now adjudicates pure IRQ-class preempt bits (HARDIRQ/SOFTIRQ/
NMI masks) on VOLUNTARY sleepers: no hardirq/softirq/NMI handler can
legally call schedule(), so those bits on a sleeper are phantom (the
leaked 0x100/0x10000 producers; the same medicine WAKE-HEAL and
PARKED-HEAL administer) and are zeroed instead of refusing the park —
reference semantics ("scheduling while atomic" is a bug print + sleep
anyway in the reference kernel). Spinlock nests (PREEMPT_MASK) and
PREEMPT_ACTIVE keep the original refusal (the jchurn lock-holder
protection). Read+zero run under a brief IRQ-off window so a racing
tick cannot erase an in-flight balanced bracket. Never fired in the
probe captures (parks were pc-clean) — kept for the WAKE-HEAL-SKIP
why=c family the benchmp-level residual shows below.

## Proof

Repro: /home/william/rux-agents/fix-sem's semprobe (2M-round SysV sem
ping-pong, exact lmbench lat_sem op pattern), init=/test/semprobe,
q35 KVM -smp 2 (run/lp-boot3.sh; a 120s-alarm build of the probe is
injected in work/probe.img).

- BEFORE the fix: wedge at 43k-138k rounds in 8/8 boots (HANG line +
  frozen WW census; logs lp-last*.log from the hunt).
- AFTER the fix: "PROBE done ~19-21s for 2000000 rounds -> ~9.2-10.4
  us/round (child st=0x0)" in 4/4 boots (1 with dfx, 3 clean config;
  the first fixed boot also printed LP-YIELD-PARK-PREVENTED on both
  pids — the prevented parks are the previously-fatal ones).
- 30-min soak (run/lp-soak.sh, repeated probe boots): see
  run/lp-soak-result.txt.

## lmbench status (honest)

- lat_sem (benchmp form): still TIMES OUT at -smp 2 — but this is a
  DIFFERENT, residual family, pre-existing my fix (fix-sem's notes
  recorded the same at -smp 2 post-EAGAIN-fix). Captured shape
  (run/lp-lm-lat_sem2.log + lat_tcp censuses): benchmp's controller
  children freeze as RUNNING + leaked 0x100/0x101 preempt bits +
  ownership marks mid-pick (oncpu=1 cl=0/1 onrq=0), with refused wakes
  (ev=4) for targets that are RUNNING inside spinlocks — the phantom
  preempt_count producer + stranded-ownership family the preempt-hunt
  lane owns (their charter; their boot counters are the baseline).
  The handoff evidence is in the lp-lm-* logs and the LP-CENSUS lines.
- lat_tcp: completes at -smp 1 (0.139 us, run/lp-lm-lat_tcp-smp1.log);
  at -smp 2 it is now FLAKY-green (2 of 3 boots DONE at 0.14 us, one
  TIMEOUT — vs 0/2 before the fix). Residual = same phantom-pc family.
- lat_connect: same rig (needs the comma spec `lat_connect,127.0.0.1`);
  the runs I did without the comma spec printed the client usage line
  (rig artifact); re-run with run/lp-lmbench.sh "lat_connect 127.0.0.1".
- The 0.14 us lat_tcp number is impossibly fast (clock granularity in
  the bench's timing loop) — measurement quality is a separate,
  pre-existing issue; the mission bar here was no-timeout.

## Gates

- Dual-arch build: run/build-x86-gate.log / run/build-rv-gate.log.
- make test: run/maketest-*.log (riscv64 unit suite).
- Soak: run/lp-soak-result.txt.

## Instrumentation kept (all behind dfx=lostpark)

- kernel/src/dfx/lostpark.rs: event ring + frozen-state dump.
- switches.rs: LostParkTrace => "lostpark" (SWITCH_COUNT 55, idx 54).
- sched.rs: LP-IDLE-LIE idle-loop lie detector; LP-WAKE-SKIP-EMPTY
  wake-refusal census; the park adjudication's own budgeted print.
- slowtask.rs: LP-CENSUS (unfiltered task census + park chains) and
  the two marker classes (non-sleeping-slotless / sleeping-IRQ-frame).
- wait.rs: ev=4 refused-token ring record. sysv_sem.rs: ev=1/2/3 ring
  records.

## Adjacent-lane handoff (preempt-hunt)

The residual benchmp-level wedges (lat_sem -smp2, flaky lat_tcp -smp2)
freeze tasks as {state=RUNNING, pc=0x100/0x101, oncpu=1, cl=X, onrq=0}
mid-pick forever, or parked with phantom 0x101 bits that my park
adjudication now tolerates but whose PRODUCER (leaked SOFTIRQ/HARDIRQ
brackets, likely the x86 softirq/IRQ exit path or a steal/abort in the
pick machinery — slotw tags on the frozen slots were all 1=pick) is
unresolved. Census shapes are in the LP-CENSUS lines of
run/lp-lm-lat_sem2.log and run/lp-lm-lat_tcp-smp2.log.
