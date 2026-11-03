# arch-s5 lane: S-5 typed timer wheel pilot

## Charter (from the survey shortlist, docs/architecture/os-kernel-survey.md S-5; user go 2026-11-03)
Pilot, 1-2 sessions: a typed timer-wheel design for the internal timer
subsystem — timer classes enforced by TYPES (one-shot vs periodic,
IRQ-context vs task-context callbacks, restart semantics) instead of
runtime flags. External ABI unchanged (timer syscalls keep reference
semantics); this is an internal divergence under the three-gate rule
(compat intact + LTP passes + no upper-layer breakage).

## Steps
1. Map the current timer implementation (kernel/src/drivers/timer.rs
   and users): representation, wheel/bucket shape (if any), callback
   context rules, the one-shot wakeup-timer discipline the DFX lanes
   rely on (nanosleep one-shot pattern).
2. Design the typed wheel: typestate for armed/pending/fired; class
   types for callback context (irq-safe fn vs task-safe fn); wheel
   levels per reference-kernel shape (cascading or hashed — pick with
   evidence from the survey + our timer-volume census).
3. Prototype behind the existing API; boot + pbench P2 + timer-heavy
   syscost rows (nanosleep/clock class) within baseline band.
4. Report: complexity delta, perf delta, type-error catch examples
   (what the types would have prevented), and a keep/adopt verdict
   for the divergence register (docs/architecture/design-divergences.md
   — entry only on evidence).

## Session log (append per session)

### Session 1 (2026-11-03)

**Map (step 1, done).** Core subsystem is `kernel/src/timer.rs` (the
drivers/timer/ dir is arch clock-event drivers). Old shape: fixed
1024-slot flat table + occupancy bitmap, ONE spinlock, full-scan
expiry check every softirq pass (R37 design: no allocation under the
lock — the R36 OOM-deadlock family). Classes were runtime flags on
one slot struct: `expires_time != 0` (hres), `interval_jiffies > 0`
(periodic), delivery kind = which of wake_pid/tfd_addr/pid+signo was
non-zero (silent else-if priority). ~45 call-site files use 4 arm
APIs + del/pending/state; `mod_timer` had ZERO callers (removed).
Historical flag-confusion bugs: hres early-fire via jiffy fallback
(lost-wakeup root cause), one-shot action leak (GNOME-oom), periodic
re-arm-without-delivery (R37), action-field double-set silent drop.

**Design (step 2, done).** `kernel/src/timer/wheel.rs`: cascading
wheel, reference-kernel class — 64 buckets x 3 levels (granularity
1/64/4096 jiffies, span 262144 jiffies ≈ 43.7 min @ HZ=100) + FAR
list rescanned every level-2 cascade (41 s; 64 rescans of headroom —
routing never delays a fire) + DUE carry list. Census evidence for
the shape: syscost rest-sites on the old table (baseline boot
s5-s5base: timer.rs lock opens 310k/720 s, hold avg 0.9-1.3 µs) and
the code's horizon census (poll slices 1-2 jiffies, futex/poll ms,
itimer/timerfd 10 ms-min, nanosleep hres sub-jiffy) — spread across
all three levels; occupancy is low (pool 1024, typical wheel census
tens — wwcensus heartbeat carries len). Types: `Deadline<JiffyClock>`
/ `Deadline<HresClock>` newtypes with per-domain `is_due` ONLY (the
mixed-compare early-fire cannot be written); `Action` enum
(Wake/Signal/TimerFd — combos unrepresentable); interval
`Option<NonZeroU64>`; `FiredToken` by-value lifecycle (one-shot fire
frees slot + moves action in one step; periodic re-arm + deliver is
one step); `SoftirqCtx` capability token for delivery context;
routing computed only in `route_for`. hres routing horizon is
grid-derived via new `drivers::timer::jiffy_floor_of_time` (all three
arches) — caller jiffy params stay diagnostics-only (their old
comment already promised that). Slot 56→64 B (pool 66 KB .bss).
Facade `timer.rs` rewritten behind the unchanged public API; softirq
flow, WW forensics, heartbeat, DFX hooks structurally identical.

**Prototype + tests (step 3, done).** Shape-test group `timer-wheel`
(9 cases: slot round-trip, wrap/cascade exact-jiffy, hres
precise-gate pinned regression, periodic delivery pin, budget carry,
catch-up advance, remove semantics, FAR round-trip, pool boundary).
Two real catches during bring-up:
1. `link_tail` linked after the HEAD not the TAIL — every bucket
   deeper than 2 entries lost its middle (lost wakeups; wedged the
   suite's scheduler group). Caught by the budget-carry shape test.
2. Suite-tail hang: the first test version used Vec/format! at the
   suite tail where the heap is exhausted (syscall:time SKIPs on the
   same run class) — alloc_error_handler panic = parked CPU, clean
   freeze. Tests rewritten allocation-free. Host-side soak (200
   rounds x 2000 interleaved arm/advance/collect/remove steps,
   per-step invariant checks: tag match, cycle detection,
   linked==popcount) passes — wheel list mechanics sound.

