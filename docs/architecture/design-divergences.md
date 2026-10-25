# Design Divergence Register

Internal design points where Rux deliberately keeps a different design from
the mainstream kernel approach, because our design carries an advantage —
gated by three conditions that every entry must hold:

1. **External compatibility intact** (syscall/ABI/ELF/filesystem interfaces
   behave to spec).
2. **LTP green** on the affected families.
3. **No upper-layer software breakage** (unmodified distro/OS images boot
   and run their full service set).

Entry template: what the divergence is · the advantage · the compatibility
evidence · the known cost · the guard that watches it (perf-gate domain or
probe). New entries land when the design reaches `main`; entries are never
deleted — superseded designs get a closing note.

---

## D-1: Global run queue (single GRQ)

**Design.** One global run queue serves all CPUs. The mainstream approach is
per-CPU run-queues plus a cross-CPU load balancer; Rux has no per-CPU
run-queue and no load balancer — the pick path is a single queue with
per-priority-class ordering (RT/CFS bands), with the idle path per-CPU.

**Advantage.**
- Single source of truth for scheduling order: no migration decisions, no
  balancer heuristics to tune, no fairness bugs from imperfect balancing.
- O(1)-class pick with a tiny state space — the entire wake/pick/strand
  forensics stack (W3 census, TERM-DESCHED, WAKE-REFUSED probes) reasons
  over one structure, which is how the scheduler bug families of the OH
  campaign were actually found and fixed.
- Measured hold cost is flat and small: GRQ average hold 5.1–6.1 µs across
  loads (volume-coupled, not contention-bound at current scale).

**Compatibility evidence.** LTP sched-family tests green; 4-CPU SMP boots
Ubuntu 22.04 to a desktop and OpenHarmony to its full ~80-service set with
launcher execution in-sandbox.

**Known cost.** A single queue is a contention ceiling at high CPU counts
and cache-locality loss versus per-CPU queues. Acceptable at MAX_CPUS ≤ 4
(current config); revisit if core counts grow.

**Guard.** Perf-gate scheduling domain: GRQ average hold band (5.1–6.1 µs
±20%), census in band (TERM 8±2 / REFUSED 32±4), pbench P2 context-switch
median ±20% — any change touching `sched/` reports these.

---

## D-2: Packed kernel-stack slab (x86_64)

**Design.** Kernel stacks are carved three-per-256KB order-6 block (80KB
slots) by a dedicated slab with a side-table slot map and empty-block
reclaim, instead of the mainstream buddy allocator's
one-power-of-two-block-per-request (a bare 80/96KB request burns a whole
128KB order-5 block). riscv64 and unmeasured arches compile the slab out
entirely and keep plain 128KB allocation — packing buys nothing at
power-of-two sizes, and the 80KB margin is an x86_64-measured figure
(the ungated first attempt corrupted adjacent heap data on the deeper-
nesting riscv64 test kernel; the arch gate is itself regression-tested).

**Advantage.** Per-stack footprint 85.3KB vs 128KB (~-115MB at the
launcher-floor population where stacks were 42% of the 904MB floor);
99.9% slot packing live (sc237: kb=981/ks=2941).

**Compatibility evidence.** No external interface touched; riscv64 suite
1096/1096 (the ungated variant read 1094/1096 — the gate caught it);
x86_64 full-image boot to deepest-ever population (3445 live tasks) with
zero canary hits; census exact hold across the gate boots.

**Known cost.** A leaf side-table lock on the alloc fast path; 6.7%
per-block tail waste; the ~15KB depth margin is arch-specific and thin.

**Guard.** R18-2 canary at every slot bottom; KSTACK-HW depth probe
(dfx=schb,ksprobe); SC-ANAT kb/ks packing columns; perf-gate allocator
domain (Tier 1 census/ticks, Tier 2 fork rows).

---

<!-- Pending entries (register when they land on main):
     - packed kernel-stack slab (order-6 blocks carved into 80KB slots,
       vs plain buddy order-power allocation) — on the launcher lane branch,
       arch-gated, register at merge with its sc228-class evidence. -->
