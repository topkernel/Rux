# Crate-flip ladder — rung 2 measurement: net/ (one page, no flip)

Method: the syscall pilot's (docs/development/syscall-crate-pilot.md).

## Forward surface (net → kernel; becomes the crate dependency)

~520 non-self refs: drivers 42 (virtio-net), sched 32, config 23,
security 20, sync 14, safety 11, arch 11, process 10 + macros/dfx.
PLUS 18 refs into fs/process/ipc (socket files touch fs/pipe-like and
process credential surfaces) and 5 into syscall (mostly errno — the
pilot's unification already gave net the framework errno path).
Cost: nothing — same direction as syscall's forward surface.

## Reverse surface (kernel → net; the cycle blocker)

~35 sites, dominated by ONE class:

| class | sites | disposition |
|---|---|---|
| syscall/network.rs → net::* (the socket syscalls) | 223 | NOT a blocker in the flipped world: after the syscall flip, syscall is a sibling service crate; the socket syscall layer moves WITH the net crate or stays in syscall depending on the seam chosen at flip time (the syscall pilot's step-5 decision gains a net rider: `sys_socket` family placement) |
| procfs/net.rs → net::generate_* (the /proc/net tables) | 10 | the generate_* table builders are framework-shaped — relocate to net/ as a `pub(crate) mod procfs_tables` OR procfs consumes a registration; single family, mechanical |
| futex/futex_pi → net (wake helpers?) | 10 | inspect: likely socket-owner wake hooks — relocate or invert |
| main.rs net init calls | 5 | entry seam (same class as syscall dispatch): net_init stays a kernel-side call into the crate — resolved by the same registration shape |
| dfx/sysfs stragglers | 7 | per-owner relocations |

## Verdict for rung 2

CHEAPER than syscall: the reverse surface is ~35 sites (syscall's
was 88 post-pilot) and the intimidating 223-site syscall/network
number is a FORWARD edge in the flipped topology (syscall→net), not a
blocker. Prerequisites: the syscall flip lands first (its seam
decision absorbs the socket-syscall placement); the procfs table
builders + futex hooks relocate (one mechanical family each).
Estimated: two focused lane-days after the syscall flip.

## The ladder state

1. syscall — pilot complete, errno class severed, tree-level forbid
   COMPILER-ENFORCED (mod.rs inner attr; edges.rs escapes via #[path]
   at the crate root; negative-probe verified). Two-crate form waits
   on the framework-crate extraction (documented).
2. net — this page: measured, priced, sequenced after 1.
3. fs service files, drivers — same method, measure when their turn
   comes (fs's forbid coverage already strong from L4's rounds).

## Closure (2026-11-22)

RUNG 2 CLOSED. The estimate held: the reverse surface resolved exactly
as priced (procfs rows + dfx/futex diagnostics + the entry/init seam ->
one 26-row hook table; the futex "wake helpers" were diagnostic
consumers, not hooks). The syscall/network 223-site class stayed
forward as predicted. One unpriced cost: the substrate extraction
(24 unsafe constructs -> framework edges) had to land BEFORE the move —
the crate forbid admits no migration allows.
