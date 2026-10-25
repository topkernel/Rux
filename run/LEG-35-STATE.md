# LEG-35 state (perf-audit lane, 2026-10-25)

Worktree: /home/william/rux-agents/perf-audit, branch feature/perf-audit.
Head at time of writing: 0baf6039 (on top of merge d3cc1cb0 = main
ddb3ed01). All leg work is COMMITTED.

## URGENT for the main session: merge candidate ff48bbf3

`fix(riscv64): lazy-FP classifier misses fflags/frm/fcsr CSR accesses`
— a deterministic user-killing SIGILL: any binary whose compiler-rt
i128 emulation (__multf3) runs with sstatus.FS=Off SIGILLs at the
`frrm` inside it (the classifier knew the FP opcodes but not
SYSTEM-opcode FP-state CSR reads). 2/2 deterministic repros on the
minimal rootfs (mrsh pid 1 died mid-boot-script); fixed + verified
end to end (the pa49 wake-latency capture runs clean on the fixed
kernel). This is arch/rap, not lane-owned code — cherry-pick to main
independently of the rest of the branch.

## Detached processes (leg-scoped)

- pa39-watch.sh (pid 470703): the LANE's standing cross-leg watch
  (leg-30 origin; alive through leg-34's close — not leg-35-scoped).
  Read-only: main-movement + sc2xx trigger grep, one line/120s to
  run/pa39-watch.log. Next leg: keep or kill per your mandate; the
  trigger check it automates is the same one recorded below.

- pa50-rebank (pidfile none; pgrep pa50-rebank; log run/pa50-rebank.log):
  quiet-window re-bank boot, armed 15:09, deadline 6h (~21:09).
  Boot: pa50main on run/pa50-main-bzImage (PURE main ddb3ed01, x86_64
  debug) — the owed volume + nanosleep re-bank. Gate: load1<=9 AND
  qemus<=8. If you are a successor leg: if the log shows a boot ran,
  parse with
    perf-gate.sh --skip-boot --log run/pa-pa50main-serial.log
  and re-bank the volume row + nanosleep row in
  docs/development/perf-gate-baseline.md from a PASSING run; if the
  deadline passed with no window, re-arm run/pa50-rebank.sh unchanged.
  Host was load 21-27 all afternoon — the window may open in the
  evening lull.

## This leg's commits (chronological)

- 38e7b47d pa46 census pair banked — F4-R2 READY FOR MERGE (census
  PASS all enforced rows + boot wall NEUTRAL: the "-30-60s" route-2
  estimate is refuted at the 0.3s noise floor; volume -1.1% vs its
  main-class twin, inside the row's documented load swing); binder
  domain STABLE with the load-elastic family correction (the old
  "15/30/89s" was ONE code-7->Init exec class stretched by host load;
  pa46-slowtxn.sh now keys exec bands by binder code and re-validates
  PASS on all five boots spanning the de41d5ab reclaim).
- 6136bbf8 LTP socket-family subset (32 rows, fleet indices preserved)
  on the branch kernel — no NEW failure class vs the fleet's own
  main-class history (30/32 == fleet-last; recv01/recvmsg03 diffs are
  documented flaky states). Closes the socket-fast-path merge debt.
- ff48bbf3 the frrm/lazy-FP fix (see URGENT above).
- 2ae237c3 first T4 trace-event deliverable: wake-latency distribution
  under churn (ONE QEMU, ring overrun 0): wake->run p50 154us /
  p90 504us / p99 13.3ms / max 33.1ms from sched_wakeup; from
  sched_waking p50 234us (enqueue attempt ~80us at p50); bulk
  100-250us; tails 1-5ms (5.3%) and >5ms (2.6%). Also the end-to-end
  verification of ff48bbf3, and an MTTCG+churn data point for the T4
  record path (overrun 0 at 3-event production).
- 0baf6039 procfile! pad form + /proc/meminfo conversion (track-1
  second conversion; Linux-exact columns verified against the live
  reference /proc/meminfo and a real boot — run/pa51-meminfo-check.log);
  FIXED leg-34's suite gap: bare #[cfg(test)] mods never join the
  unit-test feature build, so the vmstat shape/snapshot tests never
  actually ran; macro now emits pub shape_check bodies wired into
  tests/mod.rs group 'procfile-shape' — 'All tests passed' WITH them
  this time (run/pa51-maketest3.log).

## Owed by next legs

- Volume + nanosleep re-bank: decider armed (see above); rows stay
  "re-bank from a passing quiet-window gate run" until it fires.
- F4-R2 (fd23662a): claim is READY FOR MERGE on the evidence banked
  (census + boot wall + pa43 pbench); the mechanism-level sync-write
  count (-4K prediction) is unverified — needs one fsw=1 boot if the
  fs lane wants that row; NOT a merge blocker.
- sc2xx launcher.ready trigger: NOT FIRED as of sc255 (11:38, the
  newest boot; no bootevent.launcher.ready SET event; the earlier
  sc252/254 "not set" warnings stand). Run day (v1.5 final) stays
  pending.
- The socket fast path (b3757d92) is fully merge-claimed now: med-of-3
  (leg 34) + LTP subset (this leg) + OH smoke (leg 34).
- /proc/stat-family procfile conversion: next `pad`/`kv` candidates
  when touched (DFX note §8 updated).

## Build worktrees (removable)

/home/william/rux-agents/pa46-main, pa46-f4r2 (leg-34), pa50-main
(this leg; its bzImage is copied to run/pa50-main-bzImage).

## Cleanup already done

- run/pa48-ltpsock-rootfs.img (1.3G private LTP image copy) deleted;
  reproducible via run/pa48-ltpsock.sh + run/pa48-sweep-socket-subset.list
  (debugfs-write the list into a fresh copy of the fleet image).
