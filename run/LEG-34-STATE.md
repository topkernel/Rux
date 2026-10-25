# LEG-34 state (perf-audit lane, 2026-10-25)

Worktree: /home/william/rux-agents/perf-audit, branch feature/perf-audit.
Head at time of writing: 917efaa0 (docs) on top of merge 1e6963c3
(main 47d96961). All leg work is COMMITTED.

## Detached processes (leg-scoped, kill at close)

- pa46-decider (pidfile run/pa46-decider.pid, log run/pa46-decider.log):
  load-gated two-boot queue, armed 09:58, deadline 5h (~14:58).
  Boot A: pa46main on run/pa46-main-bzImage (main 47d96961 debug) —
  the owed Tier-1 census (band 8±2/32±4, ticks, volume/nanosleep
  re-bank, BINDER SLOW-TXN domain check) + boot-wall reference.
  Boot B: pa46f4r2 on run/pa46-f4r2-bzImage (47d96961 + fd23662a) —
  F4-R2 census + boot-wall comparison (run/pa46-bootwall.sh).
  Gate: load1<=13 AND qemus<=10. If you are a successor leg and the
  deadline passed: parse whatever ran (perf-gate.sh --skip-boot --log
  run/pa-pa46main-serial.log), else re-arm with the same script.

## Build worktrees (removable)

/home/william/rux-agents/pa46-main  (at 032df43b after A-kernel build)
/home/william/rux-agents/pa46-f4r2  (at 9660dc13 = 47d96961 + fd23662a)
Their bzImages are copied into run/ (pa46-*, pa47-*). Remove with
`git worktree remove` from the perf-audit worktree.

## This leg's commits (chronological)

- 53141235 wip snapshot: inherited leg-31 epoll-shard (misc.rs) + pa40 repoint
- 1e6963c3 merge main 47d96961 (panic= param)
- 032df43b pa46 census pair kernels + decider
- b3757d92 perf(net/unix): p5 socket fast path (charge counter, tap_class,
  Arc src name) — med-of-3 evidence: p5b_send -19%, recv_peek -38%,
  controls flat; OH smoke clean; NOT census-booted (net/ not in the
  mandatory census set; noted)
- e3b4087a feat(procfs): procfile! macro + /proc/vmstat conversion (§4
  lever, track-1); in-kernel unit tests green
- 5929c966 docs(dfx): procfile! adoption rule (DFX-lane coordination)
- ca839ebd/790c1355/cf878b82/917efaa0 pa47 measurement evidence + 8.10.2

## Unverified / owed by next legs

- fd23662a (F4-R2) merge claim: awaits decider boots A+B (census +
  boot-wall). If no window opened by deadline, the claim stays open.
- L3 epoll-shard (misc.rs, in every pa47 B-arm kernel): smoke-validated
  only (registry_total counter alive, 0 panics); perf-unattributed —
  pbench has no epolls so the p5 pairs do NOT measure it; right channel
  is an OH syscost A-B pair (pa44-main vs pa45-epollshard bzImages,
  both debug, exist in run/).
- Socket fast path: med-of-3 done; LTP unix/socket subset not run on
  the branch kernel (fleet covers main only) — run before merge.
- sc2xx launcher.ready trigger: NOT fired as of 11:00 (sc252 shows
  "launcher.ready not set" warning).
