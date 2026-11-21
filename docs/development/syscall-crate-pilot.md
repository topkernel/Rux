# Layer-1 crate-extract pilot: syscall/ as the first service crate

Status: PILOT COMPLETE (2026-11-21). Method proven on the largest
reverse-edge class; the full crate flip is priced below as a
multi-lane campaign with a concrete order of operations.

## The candidate (why syscall/)

12/13 files forbid-checked at zero unsafe (`edges.rs` is the only
unsafe home, already the audited framework zone); 41,377 lines; the
service layer's natural top of the stack.

## The dependency measurement (the pilot's core data)

FORWARD surface (syscall → kernel; fine, services depend on
framework): ~2,700 references — arch 711, fs 698, process 410,
sched 310, net 279, dfx 261, security 144, mm 128, drivers 94...

REVERSE surface (kernel → syscall; the cycle blocker), measured
before the pilot: ~130 sites, decomposed:

| class | sites | disposition |
|---|---|---|
| `syscall::errno::` constants | 46 | **SEVERED IN THIS PILOT** — the table was a duplicate of errno.rs::constants (no value conflicts); unified into the framework table (91 extended consts moved), syscall::errno now re-exports, kernel-side sites re-pointed. Also killed a 200-line duplication. |
| `syscall::misc::poll_events` + epoll/signalfd notify | 45 | poll/epoll notification is FILE-PLUMBING framework — relocate the notify core to fs/ (or a fs/poll.rs); the syscall layer keeps the poll/epoll SYSCALL shapes |
| `syscall::process::` exec/reboot/seccomp helpers | 13 | process-lifecycle framework utilities mislocated in the syscall layer — relocate to process/ |
| `syscall::io::` tty/open helpers | 10 | same: framework utilities — relocate to fs/ |
| `syscall::time::` | 8 | timer framework utilities — relocate to timer/ |
| `syscall::dispatch` + `syscall_handler` (trap entry) | 10 | THE ENTRY SEAM: stays kernel-side; the flipped shape is a kernel-owned dispatch TABLE registered with the syscall crate's per-call fns (or dispatch stays as the kernel-side shim and the per-call modules move) |
| misc small (random_bytes, param dumps) | ~8 | relocate per owner |

## The cycle analysis

Cargo cannot compile kernel→syscall AND syscall→kernel. The flip
therefore requires the reverse surface to reach ~(only) the entry
seam. After this pilot's errno severance it is 88 sites, all of which
are relocations of framework-shaped utilities OUT of syscall plus the
entry seam — no architectural inversion needed, no trait surface
needed. The forward surface needs nothing (it becomes the crate dep).

## The decision (verdict)

**Adopt the path; the full flip is a priced follow-up campaign, not a
single-lane change.** Order of operations, each step independently
mergeable and gate-green:

1. (DONE here) errno unification + severance — 46 sites, zero risk,
   duplication removed.
2. poll/epoll notify core → fs/ (45 sites, one family, mechanically
   verifiable by grep + build).
3. process/io/time helper relocations (31 sites across three owners).
4. The entry seam: dispatch table registration shape (10 sites) —
   decide table-in-kernel vs registration-callback with the trap
   owner present.
5. THE FLIP: `kernel-syscall` workspace member, crate-level
   `#![forbid(unsafe_code)]`, `#[path]`-including the 12 safe files
   initially (zero file moves), edges.rs staying in the kernel crate
   as the framework zone, kernel/src/syscall/ reduced to the entry
   shim + edges. Then net/, then fs service files, then drivers —
   the same ladder, each rung with the reverse surface measured
   first (this pilot's method).

## Coupling-cost summary (what the pilot proved)

- The blocker was NEVER the forward surface (services-depends-on-
  framework is the desired end state and costs nothing).
- The blocker is a thin, decomposable reverse surface; its largest
  class (errno, 46 sites) severs mechanically with a table
  unification as the bonus.
- Estimated remaining effort: steps 2-4 are ~90 call-site
  relocations in 4 cohesive families plus one interface decision —
  sized as one focused lane-week, not a rewrite.

Gates for this pilot commit: riscv64 build rc=0; unsafe_gate OK;
print ratchet OK. (Triple-arch + make test re-run at the branch tip
with the tail-completion commit.)
