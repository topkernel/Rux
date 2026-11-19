# Formal verification report

**Standing (2026-11-01)**

| Layer | Scale | Status |
|---|---|---|
| Kani symbolic proofs | **157 harnesses** (core unsafe code, all-inputs) | green on the mirror tree; nightly CI |
| proptest (host) | **1104 tests** (`make verify` leg 2) | green on every push (smoke gate) |
| SPIN models | **4 models** (kernel/verify/spin) | green; nightly CI |
| Miri UB sweep | verify crate under `-Zmiri-disable-isolation` | nightly CI |
| Mirror-sync discipline | `scripts/verify_sync_check.py` | binding: a kernel change without its mirror copy updated fails `make verify` and the CI gate |

The verification crate (`kernel/verify/`) holds hand-mirrored copies of
the kernel algorithms it proves; the sync checker enforces that a
mirrored function's body stays byte-equivalent (modulo platform
normalization) to its kernel original — the proofs are pinned to the
code they cover.

## Cadence

- Per push (fast, binding): mirror sync + the full proptest suite —
  `.github/workflows/smoke.yml`.
- Nightly (heavy): Kani over all harnesses, Miri, SPIN —
  `.github/workflows/{kani,miri,spin}.yml` (moved off the per-push path
  2026-11-01; they cannot fit a per-commit budget and used to paint
  every kernel commit red).
- Manual: `make verify && make kani` from a worktree before declaring a
  proof-bearing change done.

## Known scope boundary

Kani proves the mirrored algorithms, not the whole kernel; the
admission rule is that core unsafe primitives (page flags, refcount,
VMA manager, spinlock, futex keys, ext4 namei, ...) carry the harnesses
and everything else builds on them through safe interfaces
(docs/development/rust-kernel-best-practices.md tracks the TCB ledger).
