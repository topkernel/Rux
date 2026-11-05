# D-1 runqueue sharding — evaluation (queue #8, no-boot analysis)

Question (CHARTER attack queue): would per-CPU run-queue sharding pay,
given lock acquire = 102 ns and wake-path queuing? Answer: **no at the
current machine class; keep D-1 closed** (matches the standing user
ruling 2026-10-28). Numbers below; revisit triggers at the end.

## Evidence base (all same-rig, docs/test/lmbench-report.md 2026-11-04)

- Context-switch machinery decomposition (cctx, n=2.4-2.6M switches):
  total machinery ~620 ns of which GRQ lock acquire 102 ns, pick walk
  96 ns. Whole lat_ctx = 2.13 us (3.15x AHEAD of the reference's 6.71).
- GRQ average hold 5.1-6.1 us class is volume-coupled, NOT
  contention-bound at 4 CPUs (design-divergences.md D-1 anchors; the
  µs-class section averages were shown to be tail-inflated, the pick
  walk itself is 96 ns).
- MAX_CPUS = 4 in the current config; all perf rows above are measured
  at smp2.

## Best case for sharding

Replace the global lock with per-CPU locks: acquire drops 102 -> ~20 ns
(uncontended) on each of the two GRQ touches per switch (enqueue side +
pick side), saving at most ~2 x 82 = 164 ns per context switch IF both
touches become uncontended. As a fraction of lat_ctx (2,130 ns): 7.7%.
Against lat_pipe (4,000 ns): 4.1%. Both rows are already 3.1-3.3x ahead
of the reference; the 2x campaign's open rows (open/stat/read/write,
pagefault, sem, unix) do not touch the GRQ lock on their hot path at
all — sharding moves none of them.

## Costs the best case must first pay back

1. Wake-path queuing does not disappear: a wake targeting a remote
   CPU's shard needs an IPI or a lockless-arrival structure PLUS a
   balancer decision; the current census stack (TERM-DESCHED,
   WAKE-REFUSED, WAKE-STRANDED) has no shard-crossing semantics to
   observe, and every OH scheduler bug family was found BY that
   single-structure observability (D-1's stated advantage).
2. Fairness: per-CPU shards need load balancing heuristics; the current
   design's fairness is exact by construction (one queue, class
   ordering). Balancer bugs are a new correctness surface for zero
   measurable gain at 4 CPUs.
3. Migration cost: cross-CPU enqueue means cache-cold picks; measured
   pick is 96 ns with warm locality today.

## Crossover model

With n CPUs, the global-lock penalty grows with hold-rate overlap. At
smp2/smp4 the measured sections are not contention-bound (waits are
tail events, and the avg-hold is volume-coupled). Contention becomes
first-order when (sum of GRQ section time across CPUs) approaches the
section length — at 102-96 ns machinery segments and us-class tails,
that is roughly n >= 8-16 with syscall-dense load. The config caps at 4.

## Verdict

Keep D-1 closed (user ruling 2026-10-28 stands). The 102 ns lock is
4.8% of lat_ctx — the campaign's remaining gaps are elsewhere (entry
floor x every syscall row, path walk, sem, unix). Revisit triggers:
(a) MAX_CPUS raised past 8; (b) census shows acquire-wait share rising
(means contention became first-order); (c) a workload family where
cross-CPU wake queuing dominates — none measured today.
