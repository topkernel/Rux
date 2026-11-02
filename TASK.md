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
