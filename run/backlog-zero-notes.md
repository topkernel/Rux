# backlog-zero lane notes — the buffer-cache zeroing defect

Branch feature/backlog-zero (off main 21505704). Mission: root-cause the
deep buffer-cache zeroing that forced the lock-free dentry walk revert
(724867a0), fix it, and re-land the walk with pathprobe as the gate.

## 1. Reproduction

- Rig: r3a's pathprobe artifacts + own reflink of the shared lmbench
  image (run/img/lmbench.img, probes injected via debugfs: diag3 ->
  ino 3553, pathprobe -> 3554, block 280 = inodes 3537-3552 unchanged).
- Boot shape: q35 KVM, -cpu host, 2G, SMP2, init=/test/diag3, snapshot.
- r3a's walk kernel (review-r3a debug bzImage): block 280 ZEROED
  (mode=0 across inos 3537-3552) on boot 1; reproduces under TCG
  thread=multi SMP2 as well (that mattered: TCG gdbstub watchpoints
  see device DMA, KVM DR registers do not).
- My own semantic merge of the walk onto current main: CLEAN (the merge
  changes heap layout; consistent with "the walk only shifts heap/
  timing" — the trigger is layout, not the walk logic).

## 2. Root cause (attributed live)

kernel/src/process/kthread.rs, kernel_thread() step 4:

    core::ptr::write_bytes(
        pt_regs_ptr, 0u8,
        core::mem::size_of::<crate::arch::pt_regs::PtRegs>(),
    );

write_bytes' third argument counts ELEMENTS of the pointee type. Through
the *mut PtRegs this zeroed size_of^2 bytes: 168*168 = 28,224
(x86_64), 288*288 = 82,944 (riscv64), 320*320 = 102,400 (aarch64) —
i.e. ~27-100 KB past the 168-byte frame, off the top of every new
kthread's stack slot into whatever follows the stack block.

Attribution chain (gdbstub, TCG, SMP2):
- hw-breakpoint at persistent_log::flush_once: block 280's correct
  content ALREADY absent from the whole heap (superblock + root-inode
  blocks still cached nearby at 0x7c406400/0x7c415100).
- hw-breakpoint at bio::bread with $rsi==280, return-site break, find
  the fresh buffer, hw-watchpoint its first qword.
- HIT: memset() from kernel_thread+265 with rdx=0x6e40 (28,224);
  disassembly shows `xor esi,esi; mov $0x6e40,%edx; call *GOT(memset)`
  right between Task::pt_regs() and mark_kernel_frame — exactly the
  write_bytes. 168^2 = 28,224 = 0x6e40.
- The klogd stack slot top was at 0x7c4bc000; memset span
  [0x7c4bbf58, 0x7c4c2d98) covered the block-280 b_data page and the
  BufferHead's page neighborhood.

Why the A/B matrix behaved as it did (r3a's evidence):
- every kernel_thread() call stamps the 28 KB window with zeros;
  whether a LIVE buffer-cache page sits in the window is pure heap
  layout. walk+SMP2 deterministically placed the klogd stack under the
  freshly-read kmsg inode block's buffer (resolve_file's bread(280)
  happens immediately before kthread_run(klogd) in persistent_log
  init). SMP1 / no-walk / kmsg-off shift the layout away.
- "during the first kmsg WRITE cycle" was an observation-timing
  artifact: the zeroing fires at klogd CREATION (between resolve_file
  and flush_once), the corruption is first READ during the write cycle.

## 3. Controlled A/B (byte-identical binary, only the store length)

Patched the r3a bzImage at the `mov $0x6e40,%edx` immediate
(bzImage off 0x4b4c29: 40 6e 00 00 -> a8 00 00 00):
- unfixed: 6/6 boots ZEROED (ino=3537 mode=0 size=0)
- patched: 6/6 boots OK (mode=100775)
Same layout, same timing — only the memset length differs.

## 4. Fix

d047afad fix(kthread): cast pt_regs_ptr to *mut u8 so the byte count
reads as bytes (behavior: zero exactly the 168/288/320-byte frame).
Audited every other write_bytes site in tree: signal.rs set_fpsimd and
the riscv64/aarch64 mm paths already go through *mut u8 — correct.

## 5. Walk re-land + gates

- 3f377d13 re-lands the walk (cherry-pick of 692e7623, semantically
  merged with the 21st-successor negmemo + chroot-fast-base work that
  landed after the revert):
  - dentry.rs: both field sets kept; remove_child/clear_children carry
    neg bookkeeping AND retire/drain; memo_negative bumps children_ver
    on insert and on its eviction-remove (map mutations must retire
    walk memo slots).
  - vfs.rs: walk start keeps fast_base + claim/current_raw; the dcache
    hit path keeps the stale-negmemo normalization (parent gen from
    cur_inode_raw or get_inode_fast under the claim) before falling to
    the fs-lookup arm; note_neghit parity kept.
- Gates (all green):
  - x86_64 build-release-opt rc=0; riscv64 build rc=0; aarch64 rc=0.
  - make test riscv64: 1297/1297.
  - riscv64 scripted regression boot: cd /proc + ls + echo OK, zero
    panics (TIMER-SOFTIRQ-STALL/RCU warnings are the known TCG-under-
    host-load class; host loadavg ~9-10 during the run).
  - pathprobe (x86_64 KVM SMP2, lmbench image): 18/18 PASS, 0 failures.
  - own-kernel stability: SMP2 x4 + SMP1 diag3 boots all clean.
  - stat/open spots (lmbench, strong profile, load 6.5-9.6): stat
    0.3909/0.3926 us, open 0.8371/0.8547 us — the walk's 0.42/0.89 us
    class is back (slightly better: the negmemo + chroot fast base
    landed since).

## 6. Side observations (filed, not fixed here)

- "PARKWALK-ARMED\n" prints UNCONDITIONALLY once per boot from the
  sched-slot park walk (sched.rs ~5499) via putchar_no_lock — it
  character-interleaves with userspace console writes on SMP boots
  (the mysterious "P A R K W A L K" corruption in diag3's output
  lines). Pre-existing on main; cosmetic, but it makes serial logs
  look corrupt. Candidate: gate behind a dfx switch or drop.
- The r3a "wildwrite" suspicions (virtio DMA mistarget, pcp pageset
  D-14, buffer-head eviction race) are ALL exonerated for this defect:
  the writer was a plain CPU memset. The wildwrite-hunt line's residual
  engine (hpet statics, .bss victims) remains a different beast — this
  fix only closes the heap-overrun family.
- r3a's review-findings §1 should be updated by the fs lane: the FILED
  item is resolved by d047afad; the walk can be re-merged to main.

## 7. Artifacts (worktree only, not committed)

run/img/lmbench.img (reflink + injected probes), run/kern-fixed-bz,
run/kern-r3a-fixed-bz (binary-patched A/B kernel), run/logs/*,
run/{diagboot,abmatrix,hunt,hunt2}.sh|py, run/logs/hunt2-47133*.gdb.
