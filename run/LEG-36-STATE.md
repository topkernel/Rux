# LEG-36 state (perf-audit lane, 2026-10-25/26)

Worktree: /home/william/rux-agents/perf-audit, branch feature/perf-audit.
Head at time of writing: (this commit) on top of the a5af759c merge
(0f89f064). All leg work is COMMITTED.

## Standing debts at leg start -> status

1. Census gate on current main-class (a5af759c) + binder check —
   STILL PENDING WINDOW. a5af759c = ddb3ed01 + one commit (RVV lazy
   enable) that touches kernel/src/process/fork.rs + cmdline.rs +
   main.rs — fork-path shared code, so the census roll is a real
   check. Decider armed 21:45 (run/pa52-census.sh, committed):
   two-stage gate (first 2 h load1<=9/qemus<=8 for a chance at a fully
   green parse with the new quiet-banked volume row; then 13/10 = the
   pa46-decider class), 6 h deadline = 03:45, stage-2 switch 23:45.
   Fleet load was 19-30 with 12-19 foreign QEMUs ALL EVENING — the
   window never opened before this note. Kernel:
   /home/william/rux-agents/pa52-main/target/x86_64-unknown-none/debug/bzImage
   (PURE a5af759c, built 21:42, BUILD_RC=0). If the log shows a boot
   ran (run/pa52-census.log), parse with
     bash run/perf-gate.sh --skip-boot --log run/pa-pa52main-serial.log --tag pa52main
     bash run/pa46-slowtxn.sh run/pa-pa52main-serial.log
   The slowtxn parse ALSO adjudicates the open code-14 question
   (below). If the deadline passed with no window, re-arm
   run/pa52-census.sh unchanged.

2. F4-R2 completion — DISCHARGED (leg-35 banked the claim, perf-report
   8.10.3). This leg verified the carry: a5af759c touches no ext4; the
   claim stands unchanged. Nothing owed.

3. Volume + nanosleep re-banks — DISCHARGED THIS LEG. The leg-35 armed
   decider (pa50-rebank) fired 16:45 at load1=0/qemus=1; parsed green
   on every enforced row; banked volume 107.5M / nanosleep 203 ms
   (commit 17f5b7e5, perf-gate.sh constants refreshed same-commit,
   nanosleep now enforced not INFO-only).

## Open question for the census boot: SLOW-TXN code 14

pa56branch (branch kernel with the wake/epoll levers, host load 29-30)
showed ONE code-14 SLOW-TXN row (40.7 s exec, queue 0 ms, pid 367 ->
pid 324). NO prior boot log has code 14 (checked pa40/pa41gate/pa45/
pa47smoke/pa46main/pa46f4r2/pa50main/pa48/pa49). No branch commit
touches binder; the levers are waitqueue/epoll fast paths whose lost-
wake safety is the standard recheck argument, and every wait site
audited re-checks. Likely a rare phase/load artifact of the load-30
shallow boot, but the adjudication belongs to the pure-main a5af759c
census boot: if it also shows code 14 (any load), the checker learns
the class; if no a5af759c boot ever shows it while branch boots do,
the branch gets a targeted look. The checker (pa46-slowtxn.sh) still
FAILs unknown codes by design — do not widen it for this before the
adjudication. Related this leg: the code-7 band top moved 95 s ->
140 s because the accepted QUIET baseline pa50main itself read one
code-7 row at 133.6 s exec (the old band was fit to loaded boots and
failed the quiet baseline by construction); checker revalidates PASS
on every prior boot incl. pa50main.

## This leg's commits (chronological)

- c0766c3d pa50 boot artifacts (decider fired).
- 0f89f064 merge main a5af759c (clean).
- 17f5b7e5 volume+nanosleep re-bank (see debt 3).
- bf349926 procfile! cols form + /proc/stat + /proc/schedstat
  conversions (track-1, conversions 3-4): derived row functions whose
  ARGUMENT LIST is the column declaration; stat whole-file line order
  pinned by stat_file_tests::line_order_check; unit suite green with
  the new checks in group [13] procfile-shape (run/pa53-maketest.log).
- 6fdada6b wake/epoll lock-free empty fast paths (track-2 first lever
  on the p5 residual): WaitQueueHead::waiters one-Acquire-load empty
  check before the irqsave lock (Linux wq_has_sleeper analogue) +
  EPOLL_SHARD_N per-shard binding count skip for epoll_notify_file.
  Functionally verified: suite green with the change
  (run/pa54-maketest.log), 3 full pbench boots DONE rc=0 0 panics
  (pa55 B arm), branch census boot pa56branch TERM 8 / REFUSED 32
  EXACT at host load 29-30. A/B MEASUREMENT PENDING: the pa55 series
  (6 boots, orders A,B,B,A,A,B, release-strong, protocol 8.10.4) was
  drift-blocked — boot 1 (A) read p5b send 5.6 us, then a host regime
  change pushed every later boot of BOTH arms to 40-73 us; adjacent
  same-window pairs scatter both directions. Re-run run/pa55-ab.sh
  (kernels banked: run/pa55-a-ctl-bzImage = bf349926,
  run/pa55-b-fast-bzImage = 6fdada6b) in a quiet window BEFORE any
  merge claim for 6fdada6b; expected effect is single-digit % (the
  fast path fires only for genuinely-empty queues — the boot-wide
  wake volume is where it pays, not the blocking p5b pair).
- bc83970a DFX note 8 records the cols form (coordination point).
- 8.10.6 + slowtxn band commits (docs + checker, see above).

## Merge set (reported to the main session this leg)

READY: ff48bbf3 (frrm/lazy-FP, urgent, independent), b3757d92 (socket
fast path, fully claimed leg-34/35), fd23662a (F4-R2, 8.10.3),
0baf6039+bf349926+bc83970a (procfile track), 17f5b7e5 (gate re-bank).
HELD: 6fdada6b (measurement pending, above). The D-4 divergence
register draft for procfile! was included in the merge-set report.

## Detached processes

- pa52-census decider (setsid, started 21:45): see debt 1 — the ONLY
  leg-36 process; it exits on its own after one boot or the deadline.
- pa39-watch.sh (pid 470703, leg-30 origin, cross-leg standing):
  read-only main-movement + sc2xx trigger watch, one line/120 s to
  run/pa39-watch.log. Kept per the leg-34/35 convention; next leg:
  keep or kill per your mandate (trigger check it automates: newest
  oh-sc* serial log must contain a bootevent.launcher.ready SET event;
  as of 23:09 the newest is sc241 and it has NOT fired).

## Build worktrees (removable)

- /home/william/rux-agents/pa52-main — KEEP until the census boot runs
  (it is the kernel the decider boots).
- /home/william/rux-agents/pa55-a — removable (A/B control kernel is
  banked in run/pa55-a-ctl-bzImage).
- Older, still-listed-removable from leg-35: pa46-main, pa46-f4r2,
  pa50-main.

## Standing items carried

- sc2xx launcher.ready trigger: NOT FIRED (sc241 newest, no event).
  Run day (v1.5 final) stays pending.
- /proc stat-family conversions: interrupts/softirqs/diskstats are the
  remaining hand-written candidates (dynamic-arity rows — like
  /proc/stat's intr line, they may stay explicit by design; DFX note 8).
- Trace events on main are free instrumentation; the wake-latency
  deliverable (8.10.5, pa49) has its scripts banked
  (run/pa49-wakelat.sh) for a main-class rerun if wanted.
