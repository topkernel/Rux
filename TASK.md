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

### Session 1 (2026-11-03) — round 1 research + survey doc

- Read the in-house anchors: `docs/architecture/rust-design-headroom.md`
  (eight levers; note: it lives under docs/architecture/, not
  docs/development/ as the charter guessed) and
  `docs/architecture/design-divergences.md` — the register has grown to
  D-1..D-18; levers §1/§3/§4/§5 already landed as D-6/D-7+D-11/
  D-4+D-15/D-10. Open levers: §2 (ownership), §6 (binder FSM),
  §7 (timer wheel), §8 (driver proofs).
- Grounding greps (no edits): x86_64 ships NO vDSO (exec.rs keeps
  AT_SYSINFO_EHDR absent) — recorded as a parity gap for the perf lane,
  not a divergence; binder.rs = 9,170 lines behind the WORLD lock;
  timer.rs = 1,309 lines; io_uring module exists.
- Web research done on: Redox schemes, Theseus (OSDI '20, not ASPLOS),
  RustyHermit/OSv, seL4 (8.7kLoC / ~170k lines proof / ~20 py; CertiKOS),
  Zircon (170+ syscalls, vDSO-only entry, per-process handle rights,
  Nest Hub), Barrelfish (SOSP '09) + Barrelfish/DC, flat combining
  (SPAA '10), FlexSC (OSDI '10, Soares & Stumm — not ATC '10),
  io_uring (5.1+; ~20% over epoll; within ~10% of SPDK per SOSP '22),
  EEVDF (v6.6) + ghOSt (SOSP '21) + sched_ext (v6.12), userfaultfd
  (4.3/4.11/5.13), NOVA/PMFS + Optane EOL 2022, Capsicum (USEC '10,
  FreeBSD 9), Plan 9 namespaces (Pike), RUMP anykernel (Kantee),
  Kani/loom. Sources recorded inline in the doc.
- Wrote `docs/architecture/industry-design-survey.md` (~730 lines):
  18 designs, verdicts (ADOPT: intralingual continuation, verification
  method; PROTOTYPE: Zircon internal rights, internal writeback
  completion-ring, RUMP-style in-host fs tests; REJECT with reasons:
  11 more incl. FlexSC/unikernel/Barrelfish/PM/Plan9/flat-combining —
  flat combining rejected on OUR GRQ volume-coupled data, EEVDF
  reclassified as parity-not-divergence). §8 cross-reference: levers
  §4 and §7 have NO industry precedent (in-house validated / internal
  risk-free); ranked top-5 shortlist with per-lane cost/exit criteria:
  S-1 model-checking pilot, S-2 binder typed FSM, S-3 anykernel test
  slice, S-4 writeback completion-ring, S-5 typed timer wheel.
- No kernel code changes; no register entries this round (per charter —
  register only on prototype evidence).
- Discipline: worked only in this worktree; no QEMUs started.
- Next round (suggestions): coordinator picks shortlist lanes; S-1 and
  S-5 are the cheapest; S-2 needs OH-lane coordination on binder.rs;
  S-4 depends on S-3's replay scenarios.
