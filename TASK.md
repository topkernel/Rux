# arch-research lane: industry OS design survey — what the reference kernel lacks, what Rux can adopt

## Charter (user directive 2026-11-03)
Survey industry OS architectures for designs the mainstream reference
kernel does NOT have; for each, judge whether Rux can adopt it
INTERNALLY under the house rule: external POSIX/ABI compatibility
intact + LTP passes the affected families + no upper-layer software
breaks (advantageous internal divergences are KEPT and registered —
see docs/architecture/design-divergences.md; D-1 GRQ and D-2 compact
stacks are the existing entries).

## Candidate map (starting list — extend freely; use web research)
- Rust-native kernels: Redox (scheme/URL namespace),
  Theseus (intralingual design — types carry invariants),
  Hermit (unikernel no-syscall fast paths)
- Capability/object kernels: seL4 (formal verification, capabilities),
  Fuchsia/Zircon (handle table, vDSO-only syscall surface)
- Multikernel/scalability: Barrelfish (per-core replicated state,
  cross-core messaging instead of shared locks)
- Async/batch syscall interfaces: io_uring-style pervasive async,
  FlexSC (detached system-call batching — syscall as data)
- Scheduler designs beyond CFS/EEVDF
- Lock-free / flat-combining structures, MSG-passing IPC vs shared mem
- Memory: folios→larger, persistent-memory-first designs, userspace
  page-fault handling (uffd-style delegation)
- Build on the in-house eight-lever list:
  docs/development/rust-design-headroom.md (capability traits, typed
  per-CPU, procfile macros...) — evaluate each lever against industry
  precedent and evidence.

## Deliverable (docs/architecture/industry-design-survey.md)
Per design: what it is + where it ships (papers/projects/numbers);
what the reference kernel lacks; Rux adoption verdict — ADOPT /
PROTOTYPE / REJECT (with the three-gate risk note); cost estimate;
dependencies. End with a ranked adoption shortlist (top 5) with
prototype plans sized for a worktree lane each. Register any accepted
divergence in docs/architecture/design-divergences.md per template.

## Method
- Web research is the core tool (papers, project docs, LWN, talks);
  record sources inline. Numbers over adjectives.
- No kernel code changes this round (research + docs + the register);
  prototype spikes are follow-up lanes from the shortlist.
- x86_64+KVM is the evaluation rig for any numbers you produce.

## Session log (append per session)
