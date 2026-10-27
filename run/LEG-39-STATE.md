# LEG-39 state (perf-audit lane, 2026-10-26)

Worktree: /home/william/rux-agents/perf-audit, branch feature/perf-audit.
Head at time of writing: 36e34336 (all leg work COMMITTED).

## Leg tasks -> status

1. **Owed measurements — armed as ONE chain, waiting out the loudest
   host of the week.** The leg opened at load 0.74 / 0 QEMU (host
   reboot ~11:06 killed leg-38's pa60/pa61 deciders UNFIRED — their
   logs show gate-blocked all morning, zero boots). All six kernels
   were built/banked in 15 min (main 94c670da x86-dbg census; M=pure
   main strong; B=branch strong; ab-fast=main+two-file wake/epoll
   delta; fswm + LTP branch debug — fswm/LTP RE-BUILT at 12:26 to
   carry route-4 + the D-6 conversions, so the chain measures the
   CURRENT branch). run/pa63-chain.sh (committed, setsid) carries:
   census boot 780 s -> perf-gate + slowtxn parses -> pbench pair
   M,B,B,M -> LTP spot (17 rows) -> fsw pair (mech=branch, ctl=pure
   main) -> wake/epoll A/B (6 boots, A=M, B=ab-fast). Two-stage gate
   (9/8 for 2 h from 11:48, then 13/10; 5 h deadline = ~16:48) — the
   foreign agent queue resumed 11:35 and held load 15-33 / 14-18 QEMUs
   through 12:27. The chain fires itself the moment their boots cycle
   down; parses land in run/pa63-gate-parse.txt +
   run/pa63-slowtxn-parse.txt + run/pa63-pb-*.log / pa63-ab-*.log.

2. **A/B clarification for the next reader:** the wake/epoll lever A/B
   was ALREADY measured clean (8.10.9, p2_ctxsw -40% arm-aligned).
   The pa63 A/B stage re-runs it on the MERGE SHAPE — and on the
   94c670da class the clean lever kernel is main + the branch's exact
   wait.rs+misc.rs delta (pa55-a worktree, commit f735ffcc): 6fdada6b
   alone cannot apply to main because its epoll half presupposes the
   L3 sharding, which main never took (main still carries the
   UNSHARDED registry — worth flagging to the main session when the
   wake/epoll set merges: the sharding comes with it).

3. **Track-2 route-4 LANDED (20f64449): getdents64 listing cache —
   mechanism + LTP evidence BANKED this leg.**
   Per-open-file snapshot keyed by (inode id, VFS_DIR_EPOCH); all 9
   mutation dispatch sites bump the epoch; gated to filesystems whose
   content changes only via those ops (ext4+tmpfs cacheable; the
   dynamic filesystems stay per-call); racing scans served but not
   pinned (seqlock discipline). getdents-cache unit group 10/10;
   FULL SUITE ALL TESTS PASSED (run/pa63-maketest2.log) after fixing
   the test's own fd leak (40 leaked create-fds flipped the later
   closed-fd ioctl row to ENOTTY). Mechanism boot pa63r4m (fsw=1,
   480 s, branch debug) launched 12:29 — the claim is the fsw
   list_dir volume collapse (pre-change class ~27.6K/boot); the
   p6_getdents median rides the next pbench pair.

4. **Track-1 D-6 follow-on LANDED (dfca01e8): EXT4_FILE_OPS +
   TMPFS_FILE_OPS builder conversions + fileops-caps matrix rows.**
   ~49 struct-literal tables remain. GRQ typed lock key (the §3
   follow-on) deliberately NOT started this leg: it needs token
   threading through the wake path (multi-day); the dispatch said
   "typed per-CPU OR table migrations" and migrations are the safe
   slice for a measurement-heavy leg.

5. **STANDING.** sc2xx launcher.ready trigger NOT FIRED (watcher
   restarted post-reboot, newest oh-sc264, zero events). <=2 lane
   QEMUs held (chain holds 0 while gated; the r4m mechanism boot is
   the lane's only live QEMU). wip(snapshot) committed early.
   The oh-img trap on origin/main STILL PRESENT at e0685d49
   (run/oh-img tracked as the self-referential loop into this
   worktree — breaks every worktree that merges it; this worktree
   keeps the fixed symlink UNCOMMITTED on purpose).

## This leg's commits (chronological)

- a0dc3d4a merge main 94c670da (design-divergences conflict resolved
  keeping the branch's long-form D-6 + D-7).
- 7bd41311 wip(leg-39 snapshot) + pa63 chain armed.
- 20f64449 F4 route-4: getdents64 listing cache (+ tests).
- dfca01e8 D-6 follow-on: ext4/tmpfs builder conversions (+ caps
  rows; getdents test fd-leak fix).
- 36e34336 ledger 8.10.10.

## Merge set to report to the main session

READY (from leg-38, unchanged): 6fdada6b (wake/epoll levers — with the
sharding caveat above), d8d23064 (D-7 typed per-CPU). NEW THIS LEG,
functional-clean (suite ALL PASS), measurement-pending: 20f64449
(route-4 — mechanism boot in flight, median rides next pair),
dfca01e8 (dispatch-identical conversions; rides Tier-2 next pair).
HELD from leg-38: none (F4 route-1 landed on main via b58ff8be).

## Detached processes (handoff)

- pa63-chain.sh (setsid, armed 11:48): everything owed; 5 h deadline.
- pa-boot pa63r4m (setsid, 12:29): route-4 mechanism boot, ~10 min.
- pa39-watch.sh (restarted 11:51): sc2xx trigger watch.

## Build worktrees (removable)

- pa52-main: 94c670da detached (census + M arm + fsw ctl source).
- pa55-a: 94c670da + f735ffcc two-file lever (the ab-fast arm).
- Older removable per leg-38: pa46-main, pa46-f4r2, pa50-main.
