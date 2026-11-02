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
