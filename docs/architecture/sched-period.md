# Scheduling Period and Granularity (minimum scheduling period)

Answer to "does the system have a minimum scheduling period?" — YES,
two bounds, both in code:

## Tick cadence

`KERNEL_HZ = 100` (Kernel.toml -> kernel/src/config.rs): the scheduler
tick fires every **10 ms** per CPU. Preemption decisions for a RUNNING
task are evaluated at tick boundaries (the tick path calls
update_curr -> check_preempt) — a CPU-bound task cannot be preempted
more often than once per tick, and a task that yields voluntarily
re-enters through the run queue immediately.

## CFS granularity (the real minimum slice)

`SCHED_MIN_GRANULARITY_NS = 700_000` (**0.7 ms**, sched/fair.rs) —
also used as the wakeup granularity. Once a task is picked it runs at
least its time slice; `sched_slice()` computes
`max(SCHED_LATENCY_NS * weight / total_weight, 0.7 ms)` (the latency
window stretches to `0.7 ms * nr_running` when the period would
otherwise be oversubscribed). No task is preempted before 0.7 ms of
its slice has elapsed; a waking task preempts the current one only if
it beats it by the same 0.7 ms granularity (vruntime difference >
wakeup granularity).

## The "no frequent queue churn" property

A task re-enters the run queue (enqueue -> pick -> dequeue) at most
once per its slice in the CPU-bound case, and wakeups are throttled by
the same granularity — so the tasks_timeline BTreeMap sees at most
~1.4 structural update pairs per task per millisecond under pure
churn, and typically one per 10 ms tick per running task. This bound
is what keeps the GRQ critical section short; the leg-55 slimming work
(perf-report 8.10.25) targets the per-walk cost inside it, not the
churn rate.

Companion constants: `SCHED_LATENCY_NS` (the rolling window),
`cfs_min_granularity_ns = 700_000` (fair.rs ~1605), wakeup granularity
= same value.
