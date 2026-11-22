# Crate-flip ladder — rung S1 measurement: sched policy/core split

Method: the syscall pilot's (docs/development/syscall-crate-pilot.md)
and rung 2's (docs/development/net-crate-rung.md). Measured 2026-11-22
on the post-rung-2 tree. ONE PAGE, no flip yet.

## The tree's real shape (three findings that reshape the rung)

1. **The class files are SHELLS.** fair.rs (3,386 lines), rt.rs (818),
   deadline.rs (560), idle.rs, stop_task.rs implement the SchedClass
   vtable — but the fair/rt impls are largely STUBS ("Actual pick
   happens in sched.rs::pick_next_task"); the decision logic
   (pick, enqueue, wake placement, balance, tick policy, throttling)
   lives in sched.rs itself. Moving the class files out buys little:
   the policy mass is fused into sched.rs (12,810 lines, 168 fns,
   53 statics).

2. **The raw-pointer storm is ONE trait signature.** 108 `*mut Task`
   sites in the policy files, 96 of them fn PARAMS — the SchedClass
   trait's `enqueue_task(&self, rq: RunQueueRef, task: *mut Task,
   flags: i32)` family (class.rs). Zero direct `(*t).field` derefs in
   the class-file bodies. The design's abort criterion ("policy reads
   raw task pointers in more than a handful of sites that cannot ride
   task_shared") therefore does NOT fire at the file level: the seam
   is the trait, and redesigning ONE signature family (TaskBorrow
   handles) converts it. The pointer idioms concentrate in sched.rs
   proper (the fused file), which the carve (below) separates anyway.

3. **The class files are already (nearly) unsafe-free.** Before this
   measurement round: deadline/class/idle/stop_task/stats/sched_ev/
   wakewatch carried file-level `#![forbid(unsafe_code)]`; fair/rt/
   cgroup carried only `unsafe impl Send/Sync` markers — ALL SIX
   VESTIGIAL (auto-derived; deletion verified by green build — the
   same fear-marker class as rung 2's four socket Sync impls).
   reapdrain's `unsafe impl Send for ReapClaim` is LOAD-BEARING
   (NonNull<Task> field feeding the PENDING/GRACE_BATCH statics;
   INV-RD-CLAIM). Deletion banked with this doc: fair/rt/cgroup now
   file-forbid — 10 of 14 sched files forbid-enforced pre-split.

## File-level qualified cross-references (comments stripped)

| from | to sched | to edges | to reapdrain | to class files |
|---|---|---|---|---|
| fair | 165 | 68 | 0 | class 1, cgroup 2, idle 1 |
| rt | 44 | 26 | 0 | class 1, cgroup 2 |
| deadline | 13 | 0 | 0 | class 1 |
| cgroup | 22 | 12 | 0 | fair 3 |
| edges (core) | 9 | . | 0 | fair 1, cgroup 2 |
| side_edges (core) | 9 | 0 | 0 | deadline 2, sched_ev 7 |
| reapdrain (core) | 6 | 0 | . | 0 |
| sched (mixed) | . | 336 | 1 | fair 27, sched_ev 37, rest ≤2 |

Reading: the CORE→POLICY direction is tiny (edges 9, side_edges 9,
reapdrain 6 qualified refs) — because the policy lives INSIDE sched.rs,
not because the seam is clean. The POLICY→(sched+edges) direction is
the load (fair alone: 233 qualified refs + unqualified imports).

## Unsafe density (the core zone's mass)

sched.rs 14 blocks + 1 impl; edges.rs 83 blocks (the audited core
edge registry); side_edges 2+3; reapdrain 1+1 (both load-bearing:
the terminal-switch protocol). Everything else: zero.

## Verdict

S1 does NOT abort — but it decomposes into two sub-rungs:

- **S1a (the carve, the real cost):** split sched.rs IN-CRATE into
  sched_core.rs (GRQ + guards, switch/tp/stack, NEED_RESCHED, PerCpu,
  task-slot lifecycle, for_each walkers) and sched_policy.rs (pick,
  enqueue/dequeue decisions, wake placement, balance, tick policy,
  RT throttling, the diagnostic walks) — the 283-item triage. Done
  in-crate, each half gets its tree-level forbid (policy half) /
  documented core zone. Estimated: the largest single-file carve of
  the campaign — multiple focused sessions, item-by-item with the
  class-dispatch seam (SchedClass) as the boundary instrument.
- **S1b (the crate flip):** once carved, the flip is rung-2-cheap:
  the NetHooks recipe (hook table + register_all + composition bin),
  the class files move with the policy half, edges/side_edges/
  reapdrain stay core-side.

Banked with this measurement: the 6 vestigial Send/Sync deletions +
file-forbid on fair/rt/cgroup (10/14 files forbid pre-split).
