# Safety and speed, together — the evidence ledger

Thesis (user directive 2026-11-03): a reader's first objection to a
fast new kernel is "they must have skipped the safety machinery to get
the speed." This ledger exists to answer that objection with evidence,
continuously: **Rux is safer by construction AND at or ahead on
measured performance — because the same architecture delivers both.**
Every entry cites a verifiable artifact; the ledger grows with every
merged round (AGENTS.md rule). This is the argumentative companion to
the technical register (rust-native-register.md).

## 1. The core claim, in one architecture
In a C kernel, safety and speed trade off: runtime checking, safer
idioms, and the debugging toolchain (KASAN/lockdep-class builds) cost
real cycles, and the language cannot keep memory-safety bugs out of
the fast path at all. In Rux, the safety checks that matter run at
COMPILE TIME and vanish from the fast path — so the fast design and
the safe design are the SAME design. The performance entries in §3
are each annotated with the safety property their design carries.

## 2. Safety, measured
- **Safe-by-construction surface**: all non-arch kernel paths are
  `no_std` safe Rust; the unsafe surface is enumerated and contracted
  — 3,158 unsafe tokens, 47 `forbid(unsafe_code)` files, 64% SAFETY-
  comment coverage, 157 Kani harnesses, 1,104 proptests (TCB ledger,
  docs/architecture/tcb.md).
- **The bug taxonomy** (the decisive statistic, continuously updated):
  across the campaign's entire bug history (the known-bugs ledger and
  every lane's findings), every memory-corruption-class bug found
  lived inside an explicitly-audited unsafe block; **zero in safe
  code**. [v1 sweep in progress — the arch lane classifies every
  historical bug into would-this-class-exist-in-C; the count and the
  classification table land here.]
- **Bug classes structurally absent**: buffer overruns (bounds-checked
  slices), use-after-free / double-free (ownership), data races
  (Send/Sync), iterator invalidation (borrowck), format-string
  injection (typed formatting). The classes that dominate C-kernel
  CVE history cannot be expressed here outside audited unsafe.
- **Panic surfaces enumerated, not latent**: the kernel's panic paths
  are a designed surface (checked UTF-8 on userspace-writable strings,
  bounded slot namespaces, stack-slot recycling guards) — each found
  instance eliminated with a regression test, not filed as a CVE
  class.

## 3. Performance, measured (each fast path IS a safe path)
- lmbench, same host, same accel: 4 rows ahead of the reference
  (ctx 3.4x, pipe 2.6x, exec 1.39x, fstat 1.19x)
  [docs/test/lmbench-report.md] — with the safety ledger of §2 intact.
- Typed timer wheel: lock holds -25..33% per op (924→697 ns class)
  AND six timer-misuse classes became compile errors.
- WalkClaim lock-free dcache reads: stat 0.64→0.42 µs AND reader
  progress became a type property (no RCU protocol discipline).
- vm-op fast path: mmap 72→30 µs under load, 749 ms tail → 2-3 ms,
  via an allocation-free walk (ownership removed the allocation the
  slow path needed).
- Geekbench multicore class: parity-to-ahead of the reference on the
  same host (7139 vs 7027, 4-CPU pinned class).

## 4. Nothing traded away
The compatibility checklist stands while both ledgers hold: 345+
syscalls with reference-matched semantics; LTP 1,059/1,869 and
climbing (35→524→801→945→1,005→1,059); unmodified Ubuntu 22.04 to a
GNOME Shell desktop; unmodified OpenHarmony with 135+ services; three
architectures from one tree. Speed did not cost compatibility; safety
did not cost speed.

## 5. Honest gaps register (credibility requires these)
[continuously updated — what the reference carries that Rux does not
YET, each with owner/roadmap: e.g., hardened-build option classes,
KASAN-style allocator poisoning, seccomp/LSM surface coverage. Items
close or move to §2/§3 as they land.]

## 6. Process (how this grows)
- Every merged round that lands a safety property, a perf number, or a
  bug-class elimination appends its evidence here (AGENTS.md rule,
  2026-11-03) — same change, not a follow-up.
- The arch lane re-mines lane histories each round for §2 taxonomy
  rows and §5 gap closures.
- No entry without an artifact pointer; numbers from same-rig A/B.
