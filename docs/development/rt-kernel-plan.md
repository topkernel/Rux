# RT kernel plan — Rux-native real-time, staged

User directive 2026-11-17: build a real-time kernel (the PREEMPT_RT class of
behavior) on Rux's own structures. This plan is the working order; each stage
names the mechanism, the code surface, the acceptance anchor, and the measured
starting point. Benchmarks below are the 2026-11-17 pair battery
(docs/test/pair-battery-2026-11-17.md): same-image same-binary, Rux main
debug vs the reference kernel, q35/KVM smp2.

## Starting point (measured 2026-11-17)

| Metric | Rux | Reference | Gap |
|---|---|---|---|
| cyclictest SCHED_OTHER avg | 107 µs | 63 µs | 1.7× |
| cyclictest SCHED_FIFO p98 avg | 104 µs | 12 µs | 8.7× |
| The tell | FIFO ≈ OTHER (104 vs 107) | RT works (63 → 12) | priority not delivered |
| cyclictest FIFO max (isolated host, r3) | 396 µs | 887 µs (loaded win) | tail is environmental |
| max closed IF=0 segment (gate band) | 6–9 ms | — | delays the timer fire itself |
| pi_stress | no data (PI-mutex UAPI chain fails) | >4.8M inversions | glibc robust+PI chain |

Structural facts the plan builds on:
- Kernel preemption defaults OFF; the irq-exit preemption hook exists behind
  `kpreempt=` (kernel/src/arch/*/trap.rs; sched.rs). Explicit `cond_resched()`
  points exist on long paths.
- Timer expiry delivery is pinned to softirq context (D-19's SoftirqCtx
  delivery guard, kernel/src/timer/wheel.rs + oneshot.rs) — the reference's
  hrtimer delivers in hardirq.
- The D-14 measured-wait wakeup gate applies to all classes today; the
  reference preempts unconditionally for RT classes.
- SCHED_FIFO/RR + DL (EDF+CBS) exist; the GRQ is one global queue (D-1) — a
  single priority-ordered pick, no cross-CPU balancing to fight.
- PI is a nice-floor (kernel/src/sync/futex_pi.rs), not rt_mutex chains.
- IRQ actions run in hardirq context (no threaded IRQs).

## What we do NOT import from PREEMPT_RT

Full-tree spinlock→rt_mutex conversion. Rux's measured critical sections are
small (GRQ actual hold ~102 ns; switch machinery ~620 ns) and nesting is a
compile fact (D-11 typed lock keys). We shrink and bound critical sections
instead of making them sleep, and convert only the locks that need PI
semantics. This is the Rust-native divergence; record it in
design-divergences.md when stage R6 lands.

## Stages (in order; R1–R3 are the latency win, R4 bounds the tail,
## R5–R6 are structural completeness, R7 makes it permanent)

### R1 — Preemption on by default
- Mechanism: flip `kpreempt` default to armed (irq-exit preemption hook);
  audit the preempt-count census (phantom-bits family already at MISS=0) for
  real brackets.
- Surface: arch trap irq-exit hooks, sched.rs preempt_count paths.
- Acceptance: cyclictest OTHER avg 107 → the 63–70 µs class; make test +
  LTP sched family green; Tier-1 census bands hold.

### R2 — Timer delivery out of softirq (hrtimer-shaped)
- Mechanism: add a hardirq delivery arm for wake-type timers — the D-19
  Action::Wake class gets delivered in the timer hardirq (wake + set
  need_resched), while clock-reading stays softirq-safe. The typed wheel
  already separates these; this is a delivery-context split, not a redesign.
- Surface: kernel/src/timer/wheel.rs (routing), oneshot.rs, arch timer irq
  paths; NOHZ one-shot re-arm discipline stays.
- Acceptance: cyclictest FIFO avg 104 → ≤30 µs; p7_sleep1ms overshoot
  15.8 → ≤10 µs; nanosleep census unchanged.

### R3 — RT wake preempts unconditionally
- Mechanism: the D-14 measured-wait gate is a fair-class policy; for
  SCHED_FIFO/RR/DL wakes, set need_resched at the wake point (reference
  semantics — check_preempt_curr for RT compares priorities, no latency
  window). With R1+R2 in, the woken RT task starts immediately.
- Surface: kernel/src/process/task.rs dispatch ladder, sched.rs
  preempt_check_wake.
- Acceptance: the FIFO/OTHER gap opens (FIFO avg ≤ 1/3 of OTHER); hackbench
  forms improve (they stress wake round trips); LTP sched/rt family green.

### R4 — Bound the IF=0 tail
- Mechanism: continue the phantom-bits direction (defer softirq out of
  irq-exit windows) and take the syscost REST-site table's top holders down
  below 1 ms (audit the 6–9 ms class segments individually).
- Surface: whatever the REST table names; gate band tightens after each.
- Acceptance: max closed cli ≤ 1 ms on standard boots (gate band update);
  cyclictest max (quiet host) 396 µs holds, loaded max < 5 ms.

### R5 — Threaded IRQs
- Mechanism: forced threading in the reference's shape — irq actions run in
  per-IRQ SCHED_FIFO kthreads; the raw hardirq top-half stays only for the
  timer/clock source. irq_desc's action chain already funnels everything,
  so this is a dispatch-context change at one seam.
- Surface: kernel/src/interrupt/ (irq_desc dispatch), ksoftirqd pattern
  reused for per-IRQ threads; virtio IRQ becomes preemptible.
- Acceptance: cyclictest FIFO under fio load stays within band (the pair
  battery becomes the RT-under-load gate); no throughput regression past the
  fs/IPC gate rows.

### R6 — rt_mutex and real PI chains
- Mechanism: SleepMutex gains rt_mutex semantics (priority chains, death
  notification); the PI futex moves from nice-floor to numeric priority
  propagation along the chain; wire the glibc PTHREAD_PRIO_INHERIT UAPI
  chain (robust registration) so pi_stress runs.
- Surface: kernel/src/sync/mutex.rs (+ the loom/Kani models extend with the
  new invariant table — the S-1 harness is standing), futex_pi.rs, robust
  list handling in exit.
- Acceptance: pi_stress completes inversions (dual-column row gets numbers);
  LTP rt-mutex/futex-pi families green; the mutex loom model still passes
  INV-M1..M6.

### R7 — Gate and register
- Mechanism: add the RT domain to the perf gate — cyclictest dual-column
  bands (FIFO avg/max, OTHER avg), p7 overshoot band; the pair battery
  script becomes the standing RT measurement. Record the R6 divergence
  (no full spinlock conversion — bounded sections + typed keys instead)
  in design-divergences.md with numbers.
- Acceptance: perf-gate-baseline.md carries the RT rows; rust-native
  register entry with the same-rig A/B.

## Sequencing note

R1–R3 together are the 12 µs-class path for the wake latency (days of work,
not weeks — each is a small, measured change on existing seams). R4 runs in
parallel whenever the REST table names a holder. R5–R6 are the structural
completeness stages and can proceed independently once R2's delivery split
is stable. The pair battery rig (same image, same binaries, both kernels) is
the acceptance instrument throughout.
