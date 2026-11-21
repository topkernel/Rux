# The crate plan — Layer 1's end state

Status: DESIGN (2026-11-21). Pilot verdict ADOPT (syscall-crate-pilot.md);
the flip is executing. This document is the architecture the ladder builds.

## 1. Target end state

The workspace becomes a framework hub plus service-leaf crates:

```
rux/                      (the framework crate — "kernel/src" minus services)
  ├── arch/{x86_64,riscv64,aarch64}/   entry, pt-tables, uaccess, traps
  ├── sched/  mm/  sync/  timer/  interrupt/     the machine core
  ├── fs/  (framework side: VFS core, bio, page cache, journal)
  ├── process/  ipc/  dfx/  net/ (core)  drivers/ (framework side)
  ├── safety/              the typed primitives (KPtr, MmioRegion, ...)
  └── */edges.rs, */unsafe_edges.rs    the audited unsafe zones (INV contracts)

rux-syscall/              service crate — #![forbid(unsafe_code)] at crate level
rux-net-svc/              service crate — same
rux-fs-svc/               service crate — same
rux-drivers-svc/          service crate — same
rux-ext4/                 service crate — same (last rung; the largest)
```

Every service crate carries a crate-level `#![forbid(unsafe_code)]`: the
compiler then guarantees the WHOLE crate unsafe-free — a new file added
tomorrow cannot reintroduce unsafe without removing the crate attribute,
which the unsafe_gate forbid-ratchet makes a visible, reviewable act.

## 2. The invariant

**Framework hub, service leaves, one-way edges.** Service crates may
depend on the framework crate and on LOWER service crates (syscall is the
top of the service stack: it calls fs/net/process); nothing depends
upward. Cargo refuses cycles, so the invariant is machine-checked once
the flips land — the same class of guarantee `forbid` gives per crate.

What is "framework" is decided by the existing classifier convention
(kernel-layers.md): the machine core, the audited unsafe zones, and any
plumbing that MORE THAN ONE service consumes (poll/epoll notify, errno,
exec helpers — the pilot's relocations were exactly this rule applied).

## 3. The flip recipe (per crate, five steps — proven in the pilot)

1. **Measure first** — count the reverse surface (framework → candidate),
   decompose by class; a rung only proceeds priced.
2. **Sever/relocate** — framework-shaped utilities move DOWN into the
   framework crate (one family per commit, mechanically verifiable by
   grep + build).
3. **The entry seam** — the one place framework calls INTO the service
   (syscall's dispatch, net's rx from drivers): resolved as a
   kernel-owned registration table or a kernel-side shim; decided with
   the owning lane present, never by fiat.
4. **The flip** — new workspace member; `#[path]`-include the existing
   files initially (ZERO file moves, zero behavior change); crate-level
   forbid; the crate's edges.rs STAYS in the framework crate as the
   audited zone.
5. **Physical moves later** (optional, cosmetic) once the crate has
   proven stable through a full merge cycle.

Every step is independently mergeable and gate-green; no step changes
behavior — relocations and attributes only.

## 4. The ladder (order = least-coupled first, each rung measured)

| rung | crate | why this order |
|---|---|---|
| 1 | rux-syscall | pilot DONE: reverse surface 88 sites post-errno, all four families decomposed; 12/12 files already zero-unsafe |
| 2 | rux-net-svc | measurement queued; net's service files are flat consumers of skbuf/driver framework |
| 3 | rux-fs-svc | the VFS framework stays hub-side; the leaf file systems move |
| 4 | rux-drivers-svc | largest; depends on the safety primitives' MMIO/DMA surface being the only bridge |
| 5 | rux-ext4 | deepest fs consumer; moves last with the most measurement |

A rung whose measurement shows an un-decomposable reverse surface is
SHELVED with the numbers — the ladder does not force a bad seam.

## 5. What stays in the framework crate forever

arch entry/trap/uaccess/pt-walks, context switch, the scheduler and mm
core, sync primitives, the poll/epoll notify core, journal/bio/page-cache
core, binder (its block-view edges are framework-shaped), dfx instruments,
`safety/`, and every `edges.rs`/`unsafe_edges.rs`. These ARE the TCB; the
crate plan does not shrink them, it makes everything ELSE structurally
outside.

## 6. The payoff (metrics)

- The census splits into framework vs service-CRATE columns; the service
  columns are structurally zero (compiler-guaranteed, not grep-argued).
- The human-audited surface = the framework crate alone.
- Layer 1's "every service crate forbidden" becomes a workspace fact,
  completing the plan's Part II Layer-1 row.

## 7. Gates per rung

Triple-arch build, full suite, unsafe_gate (forbid ratchet + ledger paths
if files move), print ratchet, smoke — plus the rung's own reverse-surface
grep coming back empty at the seam only. No behavior change anywhere in
the ladder: a rung that needs one is redesigned, not argued.


## Step 4 — THE SEAM DECISION (recorded 2026-11-21, steps 2-3 complete)

**Decision: the dispatch table + syscall_handler stay KERNEL-SIDE as
the shim; the per-call implementation fns are the service surface.**

Rationale: the trap path (arch ×3 -> syscall_handler -> dispatch ->
sys_*) is the machine's hottest call sequence after the timer — the
registration-callback alternative (the kernel exposes a table the
service fills at init) puts one indirect call plus a bounds-checked
table load on EVERY syscall entry to buy a dependency-direction
technicality, and the table's 500-slot static must then live
framework-side anyway — the code does not move, only the indirection
is added. The shim shape keeps the entry sequence a direct call chain
(registered in the binary crate's composition, where main.rs already
names both sides — the crate-plan's hub-and-leaf keeps framework
below, services above, and the BINARY is the composition point that
may depend on both). Cost: dispatch.rs + syscall_handler remain
kernel files (they contain no unsafe beyond edges delegation — the
tree forbid covers them today; at flip time they stay outside the
service crate's forbid scope by construction, which is CORRECT: they
are the seam, the one place the two worlds meet).

Seam inventory (the post-flip reverse surface, measured after steps
2-3): syscall_handler + dispatch (10 sites, the seam proper);
seccomp_syscall_gate (3 arch-trap sites — the syscall FILTER is
entry-seam by nature, stays with it); syscall/edges re-exports (8
sites — edges stays framework-side per this plan, so these vanish at
flip); errno re-export imports (5 alias sites — mechanically
severable to crate::errno::constants); misc residue (param_stall_dump
×2, kernel_random_bytes ×2, timerfd ×2, pid ×1 — per-owner one-line
relocations); io/process/memory stragglers (~11, same class).
Everything else from the pilot's 130-site surface is severed.

## Flip readiness (the checklist for the flip commit)

1. lib/bin split: kernel crate grows src/lib.rs (the framework lib:
   every current main.rs `mod` becomes `pub mod`; rust_main becomes a
   pub fn); main.rs becomes the thin composition bin depending on the
   lib. No behavior change; the binary crate is the composition point.
2. rux-syscall workspace member: lib.rs with `#![forbid(unsafe_code)]`
   + `pub use rux::{arch, fs, ...}` root re-exports (so the service
   files' crate::X paths resolve) + `#[path]`-includes of the 12
   service files (zero moves). edges.rs STAYS in the framework crate.
3. dispatch/syscall_handler stay in the binary crate (the seam, per
   the decision above); the seccomp gate moves WITH the service crate
   and the traps call through the binary's seam shim.
4. The battery: triple-arch, make test, KVM smoke, unsafe_gate, and
   the NEGATIVE PROBE (an unsafe block anywhere in the service crate
   must fail the build).


## Flip milestone ledger

### MILESTONE 1 LANDED (2026-11-22): the lib/bin split
kernel/src/lib.rs = the rux framework LIB (34 pub mods, entry fns,
crate attrs); kernel/src/main.rs = the composition BIN. rust_main
stays no_mangle IN THE LIB (all three arches' entry asm resolves it
there — x86 boot.S's compile-time `sym crate::rust_main` REQUIRES the
symbol in the lib crate); its body calls seam_register_default() (the
stable no-op hook) then rust_main_impl. Triple-arch rc=0, make test
1,598 PASS. Zero behavior change.

### The remaining knot, resolved by measurement: the entry symbol vs
### the service crate

The service crate (rux-syscall) must be called by SOMETHING at
registration time, and the only composition point that can name both
crates is the BIN — but the entry symbol lives in the LIB (boot.S's
`sym` reference). Two resolutions, decided:

A) ENTRY-TO-BIN (chosen): move the three arch entry pairs
   (boot.rs/boot.S, entry.S) into the BIN. The sym reference then
   resolves in the bin where rust_main + service registration both
   live; the LIB keeps everything else. Cost: three file moves +
   build.rs link-arg verification per arch (the .incbin DTB and -T
   scripts must follow).

B) Table-with-lib-default (rejected): the lib's dispatch table
   defaults to the current match, replaced at registration — this
   compiles the service files in BOTH crates until registration
   lands, defeating the forbid guarantee.

### The dispatch conversion inventory (measured, mechanical)
The generic match is 381 single-call arms — 322 `NR => mod::fn(args)`
plus 59 fully-qualified `NR => crate::x::y::fn(args)` — and ONE
regs-passing variant (rt_sigreturn, NR 139). The default arm's
ENOSYS-attribution block (37 lines) stays verbatim in the dispatch
skeleton. Conversion shape: TWO static tables in the lib
(OnceSlot<[Option<fn(SyscallArgs)->i64>; 1200]> + a one-entry
regs-table), filled by rux-syscall::register_all at bin boot; the
dispatch skeleton does the x86 preamble/translate, the table lookup,
and the ENOSYS tail.
