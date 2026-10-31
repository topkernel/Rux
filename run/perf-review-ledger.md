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
