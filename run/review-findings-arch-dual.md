# arch-dual review findings (rv-arch-dual lane)

Scope: kernel/src/arch/x86_64/** + kernel/src/arch/riscv64/**, twins
reviewed against each other. Date: 2026-10-30. Branch: rv-arch-dual.

Verification per fix batch: `make build`, `make build PLATFORM=x86_64`,
`make build PLATFORM=aarch64` all rc=0 (run/build-{rv,x86,aa64}-fix1.log,
run/build-{rv,x86,aa64}-fix2.log). One intermediate build break (doubled
semicolon in an edit) caught by the gate and fixed before commit.

## FIXED

1. **x86_64 exception.rs: stack-overflow panic ran BEFORE the
   exception-table fixup** (the riscv64 twin's R78 conviction had not
   been ported). A fixup-window uaccess fault at a wild address landing
   in the task's kernel-stack range (or on a cross-stack rsp) killed the
   whole machine and carried held locks (EXT4_BIG_LOCK family) into the
   grave instead of returning a short copy. Reordered: demand-fill ->
   fixup -> stack-overflow panic -> forensics, matching the twin.
   Commit 40d3b046.

2. **x86_64 trap.S `.Lrestore_kernel_tail`: hardcoded 16MB kernel-rip
   window.** The image links at 0xffffffff80200000 and the bzImage
   kernel already measures 16.6MB (text ends 0xffffffff80805000); the
   [0xffffffff80000000, 0xffffffff81000000) bound only cleared text by
   ~8MB of growth and would misclassify legitimate kernel returns as
   bad frames the day .text outgrows it. Now derived from
   `__stext`/`__etext` (R57 lesson; verified in the built ELF: `lea ...
   # ffffffff80805000 <__etext>`). Commit ca2bb689.

3. **x86_64 trap.rs BusError arm: missing `force_unblock_current` when
   a SIGBUS handler exists.** The riscv64 twin force-unblocks; the x86
   arm left the pended-but-blocked SIGBUS undelivered, so the task
   returns to the faulting rip and refaults past EOF forever (the same
   livelock class the SIGSEGV arms guard against). Commit 20b99b54.

4. **x86_64 uaccess copy_from_user: no zero-fill on failure.** The
   riscv64 twin zeroes the uncopied tail and the whole buffer on the
   access_ok failure ("review SEC": callers that proceed with a short
   copy must never see stale kernel heap contents — information leak);
   the x86 port returned leaving stale bytes in both failure modes.
   Ported. Also put_user now routes through copy_to_user so a fault on
   a valid-but-COW user page gets the resolve-and-retry instead of a
   bare EFAULT (riscv64 twin shape). Commit b304e53c.

5. **riscv64 uaccess clear_user: missing fault_in_write retry.** Its
   own copy_to_user and the x86 twin's clear_user have it; a clear onto
   a post-fork COW page returned uncleared -> spurious EFAULT. Commit
   7b06902c.

6. **riscv64 trap.rs: default syscall return stored +38 while its own
   comment, the x86 twin and the ABI say -ENOSYS.** Dead code today
   (dispatch overwrites every returning path — verified in
   syscall/dispatch.rs:750) but a trap for future early-returns.
   Commit dc1ba218.

7. **riscv64 process.rs: dead arch copy_thread carried a stale
   unconditional `epc + 4`** and no module doc. The live fork path is
   the generic copy_thread in process/fork.rs (documented in the x86_64
   and aarch64 twins); handle_syscall already advances the frame epc,
   so a revived copy would skip one extra instruction (and mis-skip
   compressed ecalls). Fixed to verbatim copy + doc. Commit 0564975b.

8. **riscv64 mm_ops.rs: MM_CTX doc claimed "generation counter"** —
   the x86 twin defines it as an advisory map_page caller-context
   marker (values 1..9, read by the PTE-OVERWRITE-LIVE tripwire); no
   riscv64 reader exists. Doc corrected. Commit 49a7d5dc.

9. **x86_64 context.rs: packed ownership-qword store had no layout
   pin.** The riscv64 twin asserts (R65) that ti_on_cpu is 8-byte
   aligned and running_on_cpu sits at +4 — the single `movabs
   $0xffffffff00000000` store in __switch_to corrupts a neighbor field
   if the Task layout ever reorders. Same const-assert added; also
   removed a stray `/// Read CR3` doc line attached to read_cr0.
   Commit a6eda03e.

## FILED (not fixed — documented divergence or low-severity)

F1. **x86_64 file faults use an explicit 16-page fault-around batch;
    riscv64 relies on the ext4-side window fill** (D-12) and maps the
    pinned cache frame per-fault. Both documented in-tree (session 14 /
    R-SC-RV64); no contract divergence — LTP mmap/mincore families green
    on both. No action.

F2. **PTE lock mode: x86_64 per-mm (pte_lock_for), riscv64 global
    (PTE_MODIFY_LOCK, GLOBAL mode)** — explicitly documented in
    riscv64 mm_ops ("riscv64 follows once its own site audit is done").
    No action.

F3. **x86_64 sys_arch_prctl: ARCH_SET_GS has no canonical-address
    validation** (ARCH_SET_FS checks `addr >= 1<<47` and returns
    EFAULT; the reference returns EPERM for both). Worst case is
    user self-harm via a non-canonical GS base (GS is the swapgs
    shadow, not the kernel per-CPU pointer). Errno value (EFAULT vs
    EPERM) unverified against the man page contract — left alone to
    avoid churning an errno nothing tests.

F4. **riscv64 exception.rs NOVMA-EXEC forensic replay** (~250 lines,
    includes fixed VAs for one rootfs: base 0x3fffffe_a80, GPROBE
    offsets) has no x86 twin (x86 prints a one-line witness). One-shot
    diagnostic path, exec-faults only. No action.

F5. **riscv64 handle_breakpoint/handle_syscall keep gdm-era
    "temporary, REMOVE before merge" prints** (UBRK/SSBRK pr_info).
    Diagnostic noise on a rare path; flagged for the coordinator, not
    removed (not a defect).

## FALSE-POSITIVES RETRACTED

R1. "riscv64 trap.S R85 `bltz` gates inverted" — re-derived: bltz
branches on bit63 SET (kernel-half = good) and the repair runs on the
fall-through (user-half = wild sp). Correct as written.

R2. "riscv64 SIGBUS kernel-mode BusError missing" — kernel-mode
BusError falls to the `_ => {}` arm and returns to the faulting rip.
BusError is only produced for user-mode file faults (the SIGBUS gate
sits inside `if regs.user_mode()` in both twins' mm layers), so the
kernel arm is unreachable for it. No defect.

R3. "x86_64 trap_exit user loop doesn't re-check signals after
asm_schedule (riscv64 does)" — the window is the pre-iretq loop only;
a signal pended in it is delivered at the next trap entry (≤ one tick
at 100 Hz). POSIX-permissible latency, both twins consistent in
practice. No action.

R4. "riscv64 fork heap pt_regs + ret_from_fork_user_asm 32-byte
underflow write" — the arch copy_thread that allocates a heap pt_regs
is dead code (finding 7); the live generic path puts the frame at the
child's kernel-stack top. The underflow cannot execute.

R5. "riscv64 strncpy_from_user returns non-NUL-terminated success at
    the limit" — identical in both twins (x86 doc: "ported verbatim");
    callers treat the missing NUL as the too-long condition. Shared
    contract, not a divergence.

## Builds

- baseline (pre-change): all three rc=0
- fix batch 1 (exception order, trap.S window, BusError, ENOSYS,
  process.rs, MM_CTX doc, layout assert): all three rc=0
- fix batch 2 (uaccess zero-fill/put_user/clear_user, cpu doc): all
  three rc=0

---

# Follow-up (2026-10-31): FD-1 close-out — per-arch leaf primitives

The mm lane's filed FD-1 (run/review-findings-mm.md on main): shared
mm code (rmap.rs try_to_unmap_inner, compact.rs remap_page) carried
Sv39-only manual page-table descents — root indexed with bits 38:30 —
so on the 4-level x86_64/aarch64 trees the walk read the wrong root
slot and swap-out unmap / compaction migration silently no-oped
(ENABLE_SWAP defaults true: anonymous reclaim reclaimed nothing on
those arches).

## FIXED (commits f2cc5e06 + 936e3c8d)

- `arch/{riscv64,x86_64,aarch64}/mm/mm_ops.rs`: new leaf primitives —
  `leaf_pte_replace(mm, root, va, expect_pfn, new_bits)` (PTE lock,
  R7-C3 expect-pfn re-validation, raw entry write: 0 = unmap, swap
  entry, migration marker), `leaf_pte_install(mm, root, va, pfn,
  flags)` (rebuild a present leaf from saved flags), `pte_flag_bits`
  + `PTE_FLAGS_ANON_DEFAULT`. Each arch owns its walk shape and PTE
  encoding. riscv64 keeps the Sv39 encoding (walks gain huge-leaf
  guards the old shared code lacked); x86_64's flag extraction also
  carries NX (the old shared `& 0x3FF` stripped it — a migrated NX
  anon mapping would have come back executable) and its non-present
  leaf legally holds the raw swap entry (software bits when P=0);
  aarch64 flags are the descriptor minus the output-address field
  (attribute/AP/UXN preserved verbatim).
- `mm/rmap.rs` try_to_unmap_inner and `mm/compact.rs`
  remap_page/migrate_page now call the primitives; the Sv39 descents
  are deleted. riscv64 behavior unchanged (same encoding).

## Verification

- Builds: all three platforms rc=0 (run/build-{rv,x86,aa64}-fd1.log).
- x86_64 end-to-end probe (new, committed: test/swap-x86-probe.sh +
  test/swap_x86probe.c — the cpuinfo-x86-probe recipe, 128 MB ext4
  head in a 512 MB disk so boot swap_init carves the 256 MB tail,
  2 GB RAM, KVM):
  - FIXED kernel: SwapTotal 262144 kB active; arena 1114788 kB
    (MemFree 983716 + 128 MB); **SwapFree dropped 133380 kB** —
    try_to_unmap_with_swap actually replaced PTEs and vmscan moved
    ~130 MB to swap (the arena excess over RAM); verify pass
    **0/278697 pages corrupted** (right leaf slot written; swap-in
    restores data); clean serial; PROBE-RESULT: PASS.
  - PRE-FIX baseline kernel (built by stashing the fix): the probe's
    touch loop dies at the pagefault-OOM gate — OOM-DEEP-PASS
    `freed=0` across passes (nothing reclaimable: unmap no-ops),
    zone free 0, init killed; no PASS line. Differential confirms
    the probe is not vacuous and the fix is what changed the
    outcome.
- riscv64 in-guest suite (`make test`): all tests passed
  (run/maketest-fd1.log) — the shared mm rewiring is
  regression-free on the arch whose encoding did not change.

Filed residue: none — aarch64's swap-entry signature bits (62/61)
collide with its PCACHE/COW software bits on PRESENT leaves, but
non-present leaves never carry those bits (every PCACHE install sets
V), so the primitives are unaffected; noted here for the mm lane.
