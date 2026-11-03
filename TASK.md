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


### Session 2 (2026-11-03, successor: resumed from quota-dead predecessor)

**Inheritance + commit.** The predecessor died mid-migration with 5
dirty files + oneshot.rs; its own make-test fix2 run was already
green. Session 2 verified the intent (typed caller-side guards + the
wheel's session-2 typestate), committed it as a045a9ba (wheel
typestate: clock-typed `Now`/`Time<C>`, split `OneshotAction`/
`PeriodicAction` arm classes, shared static test wheel) and 2122be22
(`timer/oneshot.rs` WakeArmed/WakeSpent guards + futex/nanosleep
migration), merged main 02d9cbde (59cc5074; only TASK.md conflicted —
no incoming commit touched the timer files; the sched IRQ fix
d3700224 came in through it).

**Suite flake adjudicated.** First post-merge `make test` failed ONE
case (`wake_up running task`): call 1 of Task::wake_up enqueues a
boot-stack Task and rescheds an idle CPU; when that CPU picks it
before call 2, the task reads RUNNING-unlinked and call 2's R46 heal
re-enqueues → returns true. Window widened by main's d3700224 (idle
CPUs now act on rescheds promptly) under host load. Rerun ×2 green
(1306/1306); pre-merge fix2 + merge-base logs both passed the same
case. Main-inherited test race, not timer code — no change made;
left for the sched lane if it recurs.

**Gates.** x86_64 + aarch64 + riscv64 builds rc=0; make test green
(1306/1306); riscv64 minimal-rootfs scripted serial boot green
(cd /proc + pwd verified, poweroff clean, 0 panics;
run/arch-s5-s2/rv64-regression.log); pushed origin/arch-s5.

**Session-2 typestate (what shipped).**
- `Time<C>`: the now-side twin of `Deadline<C>` — `is_due` only
  type-checks when BOTH sides carry the same clock domain. The
  hres-vs-jiffy mixed compare (lost-wakeup early-fire class) is now
  a compile error on both sides.
- Arming split: `arm_oneshot_jiffy/hres` (no interval parameter) vs
  `arm_periodic_jiffy/hres` (`NonZeroU64` interval required,
  `PeriodicAction::Wake` absent — the periodic-wake and interval-0
  classes are unconstructible).
- `WakeArmed`/`WakeSpent` (kernel/src/timer/oneshot.rs): scoped
  one-shot wake guard — owns the id (not Copy/Clone), `cancel`
  consumes into a tombstone, `rearm_if_spent` consumes the stale
  guard and returns the fresh one, Drop cancels on early return,
  hres arms bake in the hart re-program. Stored-id families
  (itimer/POSIX/timerfd) deliberately keep the raw-id API.
- Callers migrated: futex `pool_full_backoff` + `futex_wait_timeout`,
  nanosleep (caller-side jiffy bookkeeping deleted — the wheel
  derives the routing horizon from the precise deadline). ~27 more
  raw-facade files share the scoped shape; left unmigrated (pilot
  scope + conflict discipline), the pattern is established.

**Type-error catch evidence** (rustc on the real tree,
run/arch-s5-s2/type-catches.txt, 6 classes):
1. E0308 `Time<JiffyClock>` vs `Time<HresClock>` in `is_due` — the
   mixed-clock compare cannot compile (the historical lost-wakeup
   bug shape).
2. E0599 `PeriodicAction::Wake` — no such variant (periodic wake).
3. E0308 `NonZeroU64` required — interval-0 periodic (busy-loop).
4. E0061 `arm_oneshot_jiffy` takes 3 args — one-shot + interval
   combination has no call signature.
5. E0308 `WakeSpent` expected, `WakeArmed` found — a spent guard
   cannot flow where a live one is required.
6. E0382 use-after-move — double `cancel`/`rearm_if_spent` on one
   guard (double-del / stale-id-del class).
Plus the two live catches during bring-up (session 1/2): link_tail
(budget-carry shape test) and the stack-wheel scribble (scheduler
KERNPANIC family — tests now share a .bss wheel).

**Measurement (s5-boot rig, OH image, KVM q35 smp2/4G, syscost=1,
780 s; A = s5base session-1 boot on the old flat-table kernel, B =
s5s2 this branch)** — run/s5-s5s2-serial.log vs
run/s5-s5base-serial.log:

| row (t=720s) | A old table | B typed wheel | read |
|---|---|---|---|
| timer arm lock (timer.rs site) | n=310051 avg=924ns | n=308780 avg=697ns | -25%/open, same volume |
| timer_pending | n=166285 avg=1288ns | n=166707 avg=868ns | -33%/open, same volume |
| del_timer remove | n=149497 avg=1291ns | n=149871 avg=862ns | -33%/open, same volume |
| nanosleep avg | 772.6ms (n=1732) | 662.7ms (n=2028) | -13%, improvement side |
| clock_gettime avg | 3us | 3us (n=33.9M, 18x deeper boot) | same class |
| TERM-DESCHED census | 8 | 8 | in band both |
| WAKE-REFUSED census | 15 | 12 | same below-pa-band class (rig property; pa-lineage band 28-36 is a different image/lineage) |
| tick rate /CPU min | 99.33 FAIL | 102.9 PASS | main's d3700224 fix (not ours) |
| GRQ avg hold | 4845ns | 2464ns | in band both |
| panics / OOM | 0/0 | 0/0 | clean |

Tier-1 band notes: syscall volume and the pa-lineage nanosleep band
are rig-incomparable on the OH image (A=4.0M vs B=67.9M — boot-depth
spread from main's wake-path fixes; the 126M anchor is the pa
userdata lineage); the A/B above is the same-rig comparison that
isolates this branch's effect. Timer-volume classes are tick-driven
(n matched to ~0.1% across A/B) — the per-open hold drop is the
wheel's indexed remove/pending against the old 1024-slot scan.

pbench P2 (scheduling domain): full battery DONE rc=0
(work/pbench-s5s2pb-serial.log, pb1 debug anchor, medians µs):
p2_ctxsw 12.198 vs anchor 15.400 (-20.8%, improvement side);
p1_null 2.958 (-35%), p1_clock 3.730 (-25%), p6_write_fsync 13346.8
(-26%), p6_append 82.1 (-84%), p6_unlink 797.7 (-98%) — all anchored
cells PASS, none regressed. (Whole-tree numbers — they carry main's
recent perf arc; the lane-relevant read is P2 in gate with the timer
paths migrated.)

**Verdict: KEEP + ADOPT (typed timer wheel + typed one-shot guards).**
Entry drafted for docs/architecture/design-divergences.md (D-19;
lands when this branch reaches main):
- Design: internal timer subsystem enforces timer classes by type —
  clock-typed deadlines/now-reading (no cross-clock compare), split
  one-shot/periodic arming (bad classes unconstructible), caller-side
  armed/spent wake guards (double-del/stale-id unrepresentable).
  External ABI unchanged; the facade keeps the legacy raw-id API for
  stored-id families.
- Advantage: three historical bug families (hres early-fire
  mixed-compare, one-shot leak/wedge, periodic mis-re-arm) are now
  compile errors; measured per-open lock hold down 25-33% on all
  three timer-site classes at matched volumes.
- Compatibility: make test 1306/1306, riscv64 scripted boot green,
  OH boot 0 panics, census in class, clock/nanosleep same-class or
  better on the same rig.
- Known cost: slot 56→64 B (pool 66 KB .bss); the guard's Drop adds
  one no-op find on fired exits (same cost as the old explicit del).
- Guard: Tier-1 quiet boot census + clock_gettime/nanosleep rows;
  pbench P2 (scheduling domain); the timer-wheel shape-test group in
  the suite.
