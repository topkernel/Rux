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
