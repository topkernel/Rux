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

## SMAP landed (2026-10-31 evening) — with a QEMU finding

**CR4.SMAP is now ON for x86_64** (arch_init → cpu::enable_smap), with
the usercopy primitives bracketing their copies with EFLAGS.AC
set/clear sequences.

Key discovery: the STAC/CLAC encodings (0F 01 CA/CB) take #UD under
QEMU TCG regardless of CR4.SMAP — bisection proved it (SMAP=0 with the
instructions still crashed at the first uaccess; main without them
never crashed; every crash RIP was garbage because the #UD site sits
outside __ex_table). Replaced with pushfq/orq/popfq AC set-clear
sequences — architecturally equivalent, works everywhere.

The first-round "signal path raw user-pointer dereference" conclusion
is WITHDRAWN: it was a STAC #UD artifact, not a real dereference. The
1194-test suite plus the strong pbench boot run with zero SMAP-VIOL
beacons — the syscall layer was already fully on uaccess (the audit's
static finding stands).

Fault-path fix that made the mode sane: do_page_fault's
kernel_data_fill branch now refuses to demand-fill when CR4.SMAP is on
and EFLAGS.AC is clear (a raw kernel dereference of a user page used
to be demand-filled, then the re-executed instruction SMAP-faulted
again — an unbounded loop that ended in the binder stall-report panic).
Raw dereferences now route to the exception table: uaccess sites get
EFAULT, anything else gets a named SMAP-VIOL beacon and an oops.

Verification: x86_64 make test 1193/1194 (only the known
identity-window prlimit64 row), strong pbench full board DONE rc=0
with all rows in band (p4_fork_exit 456.6 us, best seen) — SMAP costs
nothing measurable under TCG.

Remaining: prlimit64 EFAULT needs the PML4[0] identity window out of
the kernel CR3 (KPTI-shaped work); SMAP makes no difference to it (the
window is U=0 and STAC windows still read it — only removing the link
fixes it).

## Identity-window drop attempt (2026-10-31 late) — parked, evidence banked

Unlinking PML4[0] (bootstrap identity 0..2GB) from the kernel CR3 at
boot_stack_continuation worked mechanically (build clean, tests start)
and immediately EXPOSED a real consumer: virtio pci_rescan_block_hotplug
reads device cfg at physical 0x101000+4K through the identity mapping
(kernel paging request at 0x101004). The bar_va() x86 path falls through
to the bare physical address for BARs outside both static device-alias
windows (a firmware-placed low MMIO BAR at 0xFF000/0x101000), and the
added ioremap_bar_kernel fallback did not take effect — the assigned
BAR base_addr resolution (ECAM path) needs the virtio/pci lane to
verify. Parked until that lane owns it; main stays with the identity
window and the documented prlimit64 EFAULT gap.

Also confirmed this round: STAC/CLAC #UD under QEMU TCG is CR4-invariant,
and the syscall layer has ZERO raw user-pointer dereferences (SMAP-VIOL
beacons silent across the full x86_64 suite with CR4.SMAP on).

## Round 3 (2026-11-01): syscall entry/exit overhead — queue item 1

Audit of the x86_64 round trip (trap.S syscall_entry/trap_exit,
trap.rs handle_syscall + trap_handler tail, dispatch.rs wrapper,
signal.rs check_and_deliver_signals):

Always-on per-syscall costs found (everything else verified lazy):
- ENTRY+EXIT: the forensic SYSCALL_RING was recorded TWICE per call
  (entry sentinel + exit record): 2 global fetch_adds, 10 atomic
  stores into a rotating 64 KiB buffer, 2 current_pid() calls.
- EXIT tail: check_and_deliver_signals paid TWO locked RMWs on every
  user return — PENDING_ISIG_SIGNO.swap(0, AcqRel) and
  PENDING_CAD.swap(false, AcqRel), both almost always on the empty
  path.
- Verified cheap (one load each on the disabled path): syscost
  enter/exit, trace_sys_enter/exit (armed() inside), the seccomp gate
  (one Acquire load), rcu_note_context_switch (one Relaxed load),
  leakhunt check_user_return, steptrace gate, x86_compat::translate
  (dense match -> jump table).

Commits (feature/review-perf2):
- ff791c2b load-first ISIG/CAD pending checks (claim semantics kept:
  only the CPU whose swap sees the pending value delivers it)
- aefbf2db gate the forensic SYSCALL_RING behind dfx=sysring
  (default off; per-task LASTSYS latch stays unconditional; readers
  say the ring is off instead of replaying a stale one)

A/B (lmbench lat_syscall, KVM strong profile, one boot per case,
interleaved base->fix1->fix12 per sweep, 5 sweeps total across two
runs; med-of-runs; rig: run/sysab-abc.sh, banks
run/sysab-abc{,2}.txt; host load 3.2-10.4, foreign_qemus 0-14 —
columns interleaved at equal load per the round-2 protocol; the
KVM numbers proved load-insensitive across that band: base null
0.0901-0.0980 with no load correlation):

  case           base     fix1     fix12    fix12 vs base
  null(getppid)  0.0929   0.0803   0.0787   -15.3%
  read           0.2239   0.2058   0.2071   -7.5%
  write          0.1965   0.1833   0.1811   -7.8%
  fstat          0.1755   0.1677   0.1654   -5.8%
  open/close     5.658    5.591    5.580    -1.4%

Attribution: fix1 (the two locked xchg) carries most of the gain —
consistent with ~2 locked RMW ~= 10-15 ns under KVM; fix12's sysring
gate is small/within-noise single-threaded (the 64 KiB ring rotates
inside L2 on this host) but removes a globally-contended fetch_add
pair from every syscall under SMP and matches the mm-forensics
default-off policy. open/close is path-lookup dominated (queue item
3).

vs the native reference column: null 0.0909 -> 0.0787 us; ratio vs
reference 0.0398 improves 2.28x -> 1.98x. Remaining delta
attribution for the next pass: getppid's body walks the pid hash per
call (the reference reads a cached field) — a per-syscall-body cost,
not entry/exit; candidate for a follow-up micro-lever (cache ppid,
invalidate on reparent — bug-dense, needs LTP fork-family gate).

Gates: x86_64 + riscv64 builds rc=0 (strong profile x86); make test
1204/1204 ALL PASSED on the fix1+2 tree; every lmbench boot is a
full PID-1 functional boot (ext4, virtio, timer) — all clean.
Worktree note: toolchain/riscv64-rux-linux-musl was a dangling
symlink to the deleted ltp-r20 lane; rebuilt locally (make rootfs
failed until then).
