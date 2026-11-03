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

### Session 2 (2026-11-03) — round 2: complete the Rust-native register

Mission (user directive 2026-11-03, record-everything): make
docs/architecture/rust-native-register.md COMPLETE and unassailably
evidenced. Merged main (4fecef2d register skeleton) — one TASK.md
conflict resolved in favor of this lane's charter (lane-local file).

- Mined every lane's evidence: TASK.md records across the 63 lane
  worktrees, run/ artifacts (stress-port/run/stress-runs/findings.md,
  arch-s5/run/arch-s5-s2/type-catches.txt, perf-audit/run/
  pa68-d7-rootcause.txt, dfx-trace TASK round-1 verification), the
  main-tree perf-gate-baseline.md and docs/test/lmbench-report.md,
  and git log on main (~30 candidate commits hash-verified as
  ancestors of main before citing).
- Register rewritten (skeleton 91 lines -> ~260): thesis paragraph up
  front (reference-compatible kernel, Rust-native internals, measured
  ledger, 30-second read); evidence conventions (numbers inlined,
  source pointer per entry; reference-parity fixes filed under C, not
  claimed as originals).
- New entries vs the skeleton: D-11 lock keys (with the 2b092c49
  conviction -> #[must_use] story), D-7 CpuLocal (with the .rodata
  incident), WalkClaim reader banks (3f377d13, incl. the honest
  revert/re-land history), D-2 packed stacks (numbers inlined), the
  reference-parity receipt list in C (12 commits: ETXTBSY, SysV
  namespaces, futex physical keys, tmpfs MAP_SHARED, binder poll,
  dead-tick with the 5.42M-ops number, execveat, O_DIRECT, poll
  RLIMIT, timerfd deadlines, SYSRET TF|RF), section D now carries the
  generated TCB-ledger numbers (3158/1340 unsafe tokens, 47
  forbid(unsafe_code) files, 64% SAFETY coverage, 157 Kani harnesses,
  1104 proptests), E gains the bio write-plan closure (lb229 decode,
  allocation-free fix, verification state kept honest), the x86_64
  ctx-switch levers (9d6980b1: lat_ctx -58%/-59%, PCID gating
  honesty), p5/fs re-bank families; F gains the census+heal machinery
  (miss=0, IF=0 25.7->20.4%, cli 594-599ms->6-9ms, strands 233->31)
  and the allocwatch/lostpark exonerate-instruments entry (the mirage
  verdict recorded as evidence discipline).
- Fixes to skeleton claims: lmbench pointer corrected to
  docs/test/lmbench-report.md (the standing table lives there, not in
  perf-gate-baseline.md); "ledger retires 59 switches" corrected to
  what the ledger actually says (59-entry table mapped, waves ported
  with census A/B identical); dead-tick moved from E (redesign) to C
  (reference-parity receipt) since the fix adopts the reference's
  switch-boundary rule.
- Landed D-19 (typed timer wheel) in design-divergences.md from the
  arch-s5 session-2 draft — the branch reached main (0b396384) but the
  D-entry never landed; also removed the stale "pending entries"
  comment block (D-2 landed long ago).
- Section G stays placeholder-honest (S-2/S-3/S-4 + the standing
  rule); no kernel code changes; no QEMUs started.

### Session 3 (2026-11-03) — round 3: safety-and-performance ledger v1 (taxonomy + gaps)

Mission (coordinator): land the two decisive artifacts of
docs/architecture/safety-and-performance.md — the §2 bug taxonomy and
the §5 honest gaps register — plus §3 pairing polish. Synced
origin/main first (picked up dced3cb7, the seeded ledger; fdreap RCU
retire; ltp_family_boot).

§2 taxonomy v1 (the decisive statistic):
- Sweep: 3,358 deduped non-merge commit subjects (cherry-picks
  deduped by subject), 2,702 kernel-code subjects after excluding
  docs/session-logs/LTP-scans/test-drivers; 169 corruption/race/panic
  vocabulary candidates individually adjudicated; cross-checked the
  lane hunt ledgers (wildwrite-hunt.md 3,284 lines, r74-scribble-hunt,
  review-coverage.md defect table, security-posture.md).
- 33 memory-corruption-class bugs adjudicated to root cause, each
  fix-diff inspected for defect residency: 23 inside explicit unsafe
  blocks / arch asm / unsafe interiors; 10 in SAFE code — all ten
  ownership/refcount/lock-protocol/stack-depth decisions through
  trusted unsafe boundaries (vmscan <=0 free, page_cache stale put,
  exec guard drop, Arc-swap race, 2 stack overruns, ti_cpu TOCTOU,
  GRQ guard discard, recvmsg re-fetch, PCACHE bit-10). Zero
  pointer-expression classes (indexing overrun, iterator
  invalidation, format-string) in safe code; the one safe-code
  would-be OOB write (pcp refill race) panicked at the bounds check.
- The draft claim "zero memory-corruption bugs in safe code" was
  FALSIFIED in strict form and replaced by the 23/10 residency split
  with per-row citations — the honest version is stronger: the
  language killed the C expression-level classes everywhere; the
  safe-code residue is exactly the manual-ownership surface §5
  targets (typed Refcount, lockdep-class coverage).
- 4 boundary rows (user-memory/on-disk corruption via bookkeeping:
  mremap expand, R71 pos race, VMA surgery, ext4 extent/bitmap
  aliasing) counted separately, not in the 33.
- Class (c) receipts: 15 panic-surface rows listed with commits
  (pcp refill bounds catch, UTF-8 printer family, negstale, mmprof
  slots, SWITCH_COUNT, memwatch PFN, R81/R73 prints, taskdump scan,
  ext4 empty-index, aarch64 regs[31], binder expect, timer
  reentrancy, Wave-1 batch).
- Honesty notes kept: PTGUARD/FREEGUARD misdiagnosis revert, PT_LEDGER
  hash-collision false reports, the 2b092c49 mechanism correction.

§5 gaps register v1: ordered by-construction (4 rows: bulk memory
safety, bounds checks, data-race freedom, iterator/format classes)
→ have-with-artifacts (13 rows: SMAP+AC with G1 fixed 763e9f3c,
  CR0.WP, no-identity-window D-17, user W^X, seccomp+nnp, 41-cap
  model, kstack canary/quarantine/audit, deferred-free quarantine,
  oops/pstore, tracefs, Kani/loom rig)
→ partial (7: lockdep-class coverage, refcount saturation — tied to
  §2 rows 24-25, release overflow-checks, KASAN-breadth, LSM,
  namespaces, cap breadth)
→ absent-plainly (8: KASLR, kernel W^X tightening, compiler stack
  protector, hardened usercopy, module signing — moot while
  registration-only, KCSAN-class checker, eBPF channel,
  kallsyms masking). Each absent names the closing shape.

§3 pairing: every perf row now carries its safety-pairing clause
(lmbench standing table numbers from docs/test/lmbench-report.md;
ctx-lever -58% with the FP-free-binary/af464fb9 pairing; p5/fs
re-banks from perf-gate-baseline.md board; Geekbench on D-1 with the
census bands). Fixed the stale tcb.md pointer →
docs/development/tcb-ledger.md; capability count corrected to 41
(code truth, CAP_LAST_CAP=40).

No kernel code changes; no QEMUs started. Pushed branch
arch-research.
