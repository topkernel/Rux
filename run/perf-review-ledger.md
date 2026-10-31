# Performance review ledger — 2026-10-31, target: 10x on memory paths

Working tree: /home/william/rux-agents/code-review-perf (branch
feature/code-review-perf). Baseline for all A/B: main 40e925fd,
pbench x86_64 DEBUG profile, pa-series machine class, QUIET host
(foreign_qemus<=1). Noise discipline: any pbench run taken at
load>=4 is void for comparison (qs1 voided this way).

## Findings (lock/critical-section survey, ranked)

F1. No per-CPU pageset; every order-0 alloc/free takes the global
    (single-zone) zone lock with descriptor init inside.
    -> FIXED 45ccb66d (pcp, INV-PCP-1..3). p3b_fault_cow med
       24.614 -> 20.693/21.263 us (-14%), quiet runs.

F2. RCU QS was reported ONLY on __schedule + idle. Return-to-user
    (syscall+IRQ+exception exit) never reported, so a CPU in a
    syscall-heavy loop held RCU_QS_GEN stale and every
    synchronize_rcu() waiter (MmStruct::drop on EVERY process exit)
    stalled ~a full scheduler tick (~10 ms). Matches the measured
    p4_fork_exit gap vs the reference column (perf-report 1129:
    38.6x class; 8324 us vs 420 us class).
    -> FIX IN FLIGHT: rcu_note_context_switch() wired into both
       arches' common user-return tails (next to the leakorigin
       check), gated by a new RCU_WAITERS counter so the fast path
       is one relaxed load when nobody waits (protects p1/p2 rows).

F3. mprotect holds per-mm vma_write across the whole syscall with a
    Vec collect + sort under the lock (memory.rs ~1622-1646). The
    atomicity is deliberate (fixed a real PTE/VMA split bug); the
    collect+sort can become a BTreeMap range walk. PENDING (low
    frequency syscall; do after F2/F4 measured).

F4. Page cache: global wlock serializes all insert/evict, and
    insert_batch holds wlock + per-inode lock across up to 64x4KiB
    memcpy; per-inode lock is exclusive even for read hits.
    PENDING — expected to move p3c_fault_file and p6_read_cached.

F5. riscv64 PTE modify path degrades the per-mm PteLock to the
    GLOBAL write lock (mm_struct.rs:114-117); x86_64 uses the
    epoch-read + per-mm-spin split. PENDING — affects riscv64 SMP
    fault throughput; not visible to the x86 pbench rig, needs a
    riscv64 measurement harness first.

F6. MmStruct::drop runs synchronize_rcu outside the vma_write lock
    (the survey's "under the lock" was wrong — re-read confirmed
    the wait is outside) but still SYNCHRONOUSLY on every exit;
    F2's fix attacks the wait length itself. If after F2 p4 is
    still >> reference, revisit async (call_rcu-style) teardown.

## Measurement protocol

- pbench_boot.py, DEBUG bzImage, --tag <name>; med-of-runs for
  verdicts; p99 is host-noise-dominated (never a verdict input).
- pre-fix quiet bank: pb1 (main 40e925fd = pre-pcp), pcp1/pcp2
  (pcp only). Every post-fix run records foreign_qemus from the
  boot line; runs at load>=4 are void (qs1 voided).

## Round 2 results (2026-10-31 afternoon)

Commits on feature/code-review-perf:
- 45ccb66d pcp pagesets (p3b dbg -14%; strong: p2/p5 rows slightly better)
- c312944d RCU QS on return-to-user (p2 wall half-rt 4.37 -> 3.08 us
  quiet; med -4.7% back-to-back at foreign=3). Attribution correction:
  the earlier "grace = 10 ms tick" theory was WRONG — RCU_GEN never
  advances without call_rcu users, so grace was already cheap; the p2
  gain comes from the QS path shortening stall windows, mechanism
  verified by A/B only.
- 11517d2a(+amend) forkbench probe in the packed set
- (leaf fast path commit) fork COW-exempt scan: leaf-table granularity;
  no pbench-visible delta (pbench's exempt list is short) — kept for
  the OH many-VMA shape.
- 49f40d2d page_cache fill/describe outside the locks; single-threaded
  pbench unchanged (expected: readers use the lock-free claim path);
  concurrency is the beneficiary.

## Where the 10x stands (strong profile, TCG, vs reference TCG column)

- p2 ctxsw: 3.08 us vs 15.4 us -> 5x AHEAD
- p5a pipe: 2.11 vs 21.5 -> 10x AHEAD
- p5b dgram: 40 vs 462 -> 11.6x AHEAD
- p5c stream: 36.4 vs 568 -> 15.6x AHEAD
- p3 fault family: at or ahead of the reference (p3b 2.9 us vs the
  A-line 3.4 us; p3a/p3c marked "ahead" in the reference table)
- p4 fork+exit: 472 us — the LAST lagging row. forkbench shows a small
  mm forks in 75 us; the pbench probe carries p3's 64 MB map (16K PTEs
  of COW marking + teardown) and the TCG per-PTE atomic tax dominates.
  Next levers, in order: batched COW write-protect with per-table TLB
  flush instead of per-PTE atomics; exit teardown chunking. Risk:
  fork/exit correctness paths are the most bug-dense in the tree —
  gate every step with make test + the full LTP fork family.
- dbg-profile rows are 12-100x the strong rows by COMPILE-PROFILE tax
  alone (p1 6.9 -> 0.55; p2 wall 437 -> 4.37); the perf gate anchors
  dbg runs against dbg columns only (perf-report 8.10.1) — never
  compare across profiles.

Noise protocol (validated): back-to-back A/B at equal foreign_qemus;
med-of-runs for verdicts; void any run at host load >= 4 for
absolute claims.
