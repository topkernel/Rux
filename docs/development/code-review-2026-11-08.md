# Rux Kernel Code Review — Round 4 (2026-11-08)

> Scope: full-tree, five parallel review agents, zero-coverage-gap partition.
> 458 files / 330K lines in `kernel/src` reviewed across arch (riscv64 + x86_64 + aarch64), fs, mm/process/sched/sync, syscall/ipc/net/io_uring, and drivers + debug/trace/perf + top-level + build/test infrastructure.
> Detailed reports: [review4/arch.md](review4/arch.md), [review4/fs.md](review4/fs.md), [review4/mm-proc.md](review4/mm-proc.md), [review4/syscall.md](review4/syscall.md), [review4/drivers-misc.md](review4/drivers-misc.md) — each with a per-file coverage appendix.
> Dedup: findings were checked against `review-coverage.md`'s fixed-defect table and the 2026-09-12 review; three Sept-round fixes were found **still live in the tree** and re-filed (see S-13 below).
> Reference: Linux at `/home/william/refer/linux/` (6.x), virtio 1.1 spec, ARM DDI 0487, RFC 793.

## Totals

| Severity | Count | Notes |
|---|---|---|
| **Critical** | 2 | dentry-drop-under-lock regression; IPI csd container_of (code-exec) — both ✅ FIXED |
| **High** | 11 | all ✅ FIXED (incl. DRM prime mmap = arbitrary phys R/W; binder BR stream desync; ext4 rename-over-open-file frees blocks) |
| **Medium** | 42 | 39 ✅ FIXED; 3 open (getrandom entropy source, binder silent-txn retirement — deliberate OH-leak-mitigation divergence, ext4 async read gate — bio completion attribution) |
| **Low** | 67 | 64 ✅ FIXED; 2 recorded deliberate divergences (binder handle-0 8s park, c.ecall +4 reference-parity); 1 attempted-blocked (fixmap Svpbmt IO bit — needs an Svpbmt probe; QEMU default rv64 treats bit 62 as reserved) |
| **Info** | 40 | design observations (a handful addressed with the Lows they annotated; remainder tracked in the per-area reports) |
| **Total** | **162 findings** (+8 retracted/merged sections = 170 `###` blocks) | |

Per area: arch 33 · fs 30 · mm-proc 14 · syscall+ipc+net 61 · drivers+misc 32.

## Critical

- **C-1 [fs/dentry.rs:295]** ✅ FIXED (`124045df`) Regression of the f62a4df6 fix: the 3f377d13 reader-bank re-land restored dropping retired dentry/inode Arcs *while holding* `DENTRY_SLOW_READ.lock_irqsave()`. The drop cascade runs `Inode::drop → ext4_destroy_inode → read_inode → bio::bread` — synchronous virtio I/O with IRQs+preemption off, reachable from every last-reference unlink/rmdir and the common path_memo retire path. Ancestry verified (`git merge-base`): the fix is an ancestor of the re-land that reverted it. Fix: take the vecs under the lock, drop outside (mechanical).
- **C-2 [riscv64/ipi.rs:331 + aarch64/ipi.rs:298]** ✅ FIXED (`124045df`) `csd_flush_queue` derives `CallSingleData` from the list-node pointer **without subtracting `offset_of!(list)`** — `(*csd).func` reads the static queue head and jumps to it: executing list-head bytes as code on the target hart. Reachable via `sys_membarrier(GLOBAL/GLOBAL_EXPEDITED)` on any SMP boot. The x86_64 twin fixed exactly this (ipi.rs:364-371, with comment); the port never happened.

## High (11)

1. **[drm/mod.rs:2479]** ✅ FIXED (`3a17c555`) Prime-fd mmap offset unbounded → **arbitrary physical read/write**: `gem_mmap_resolve`'s prime branch returns `gem.phys + offset` with no `offset < gem.size` check; `/dev/dri` is 0666 and any process can mint a prime fd (CREATE_DUMB + PRIME_HANDLE_TO_FD). Linux `dma_buf_mmap` bounds it. One-line fix.
2. **[binder.rs read path]** ✅ FIXED (`d5fa104e`) 512-byte BR staging buffer silently drops bytes (`Staged::extend`) when full while the drain loop bounds only against user read_size — command header fits, ptr/cookie payload does not → BR stream desynchronizes mid-payload for every binder process under small-work bursts.
3. **[binder.rs handle-0]** ✅ FIXED (`d5fa104e`) BC_INCREFS/ACQUIRE on handle 0 find-or-create by node, BC_RELEASE/DECREFS look up `desc == 0` which never matches the created ref (`next_desc`) — every BpBinder(0) cycle permanently leaks one strong ref + one ref-table entry on the context manager.
4. **[readv/writev]** ✅ FIXED (`3a17c555`) iovec double-fetch TOCTOU: validate pass, then consumer loops re-read each `Iovec` from user memory and copy to the fresh `iov_base` unvalidated — same class fixed for recvmsg in 32fd359f; racing CLONE_VM thread redirects kernel copies to unvalidated addresses.
5. **[ext4/namei.rs:2693]** ✅ FIXED (`d5fa104e`) rename-replace of an open file frees blocks eagerly (unlink got deferred deletion; the rename arm did not) — running-binary upgrade can serve another file's data through the live mapper after block reuse.
6. **[jbd2]** ✅ FIXED (`8952fd96`) commit/revoke j_head disagreement: revoke blocks written at pre-commit j_head clobber the descriptor block (mixed commits) or are clobbered by the commit block (revoke-only commits = every lazily-batched pure unlink/rmdir). On-disk log of any revoke-carrying commit is malformed; write-through hides it from our own recovery.
7. **[sync/futex.rs]** ✅ FIXED (`d5fa104e`) signal-return race with a concurrent waker leaks a waiter-pool slot permanently (remove_waiter miss path returns without freeing; pool exhaustion degrades all futex waits to the 1-jiffy backoff loop). Linux `futex_unqueue()` is leak-free by design (on-stack waiter).
8. **[sync/futex_pi.rs]** ✅ FIXED (`d5fa104e`) PI sleeper-side cleanup (`futex_lock_pi` post-schedule, `futex_pi_exit_cleanup` part 1) unlinks/frees without the PI bucket lock, racing `futex_unlock_pi`'s handoff — `handed` can land on a freed/reallocated slot, breaking PI mutual exclusion. `pi_requeue_detach` in the same file is the correct pattern.
9. **[mm/kswapd.rs]** ✅ FIXED (`3a17c555`) lost-wakeup race permanently parks kswapd (clear-token → check → set-state → schedule, no post-state recheck; a dropped wake strands the token so every later `wakeup_kswapd` early-returns).
10. **[virtio/queue.rs:875]** ✅ FIXED (`3a17c555`) `wait_for_desc_interruptible` has no wake-source deadline — used by MMIO sync disk I/O paths; one lost IRQ strands the waiter forever. Its sibling fixed this class (10s `add_timer_wakeup` + kick/walk recovery, documented there as mandatory); port never happened.
11. **[riscv64/mm/memory_layout.rs:548]** ✅ FIXED (`3a17c555`) `virt_to_phys` kernel-image window still caps at link+16MB while the image grew to 24MB — .bss/.data VAs above the cap return a virtual address as physical (one pr_warn); consumer: static virtio queue buffers fed to DMA.

## Notable Medium (selection)

- **S-13: "fixed" Sept-round defects still live** — `get_mempolicy` nodemask OOB write (kernel writes `nwords*8`, access_ok checked `(maxnode+7)/8`), `shmdt` accepting interior addresses, `sem_undo` exit without [0, SEMVMX] clamp: the fixes are documented in review-coverage/2026-09-12 but the tree still carries the defect forms. Fix-forward required plus a process note: fixed-in-branch ≠ merged-to-main.
- **[arch, all three]** non-canonical-VA guard tests the VirtAddr-masked address (half the class escapes; x86 does it right on raw CR2) ✅ FIXED (`724e40be`, raw stval/FAR); `flush_tlb_others`/membarrier targets CPUs `[0, count)` instead of the started set.
- **[arch/riscv64]** exception.rs carries ~250 lines of ungated hunt instrumentation firing ~40 pr_err per user exec fault; `map_page` does a full `sfence.vma` per demand fault, defeating the ASID layer.
- **[fs]** icache hash-collision overwrite drops still-referenced inodes (dual-instance class); tmpfs hard links don't share size; MNT_NOEXEC/NOSUID parsed but never enforced; devfs rename has no cycle guard ✅ FIXED (`f88ff011`); jbd2 has no time-based commit (unbounded metadata crash window) ✅ FIXED (`00a52820`, kjournald thread + t_expires + commit gate).
- **[syscall]** execve truncates argv ✅ FIXED (`8952fd96`) at 8KB and drops the rest of the array on mid-read fault (should be E2BIG/EFAULT); clone3 CLONE_PIDFD probes the word but never writes the fd ✅ FIXED (`5b93ce8e`); epoll dead-interest purge keys on fd number ✅ FIXED (`8952fd96`) (dup2+close removes a live watch — Linux keys on the file description's last ref); IPv6 connect ignores nonblocking ✅ FIXED (`00a52820`).
- **[drivers]** virtio_pci Phase-3 drain counts any chain's used-ring entry as completion (UAF window on 64-byte io_buf, R21-N2 class — should match head_desc like the MMIO twin); virtio_net xmit spins for TX completion under `tx_queue.lock_irqsave` per packet; printk /dev/kmsg heap-allocates under the ring irqsave lock; TSC resync's >25s rebase arm unreachable (else-if shadowing); DRM bring-up trace families ungated on every boot ✅ FIXED (`f88ff011`, six `drm:` tracepoints + debug=ohhunt gating).
- **[mm-proc]** rcu.rs lost waiter-announcement race strands call_rcu callbacks; seqlock missing reader-side LoadLoad fence (TCG hides it); fork RLIMIT_NPROC compiled out (`if false`).

## Clean passes (no new defects)

interrupt/ (irqdesc/softirq/tasklet), intc cores (APIC/GICv3/PLIC), timer drivers + wheel, trace (61-event table), perf, security/module, pstore/oops, initrd (DEFLATE path verified), console, debug arsenal gating discipline (the one always-on regression, mmprof, is already fixed in-tree), RVV vector encodings in vext.rs (checked against `riscv64-linux-gnu-as`), binder reference-parity commits (deferred tcomplete, send_failed_reply routing, silence sweeps — all check out vs 6.6).

## Fix plan (waves)

- **Wave C (immediate, ≤1 day each)**: C-1 dentry drop-outside-lock (mechanical); C-2 csd container_of port from x86_64 twin; DRM prime mmap bound; kswapd state-first recheck; virtio queue deadline port; virt_to_phys 24MB cap; readv/writev snapshot-after-validate; the three S-13 still-live fixes.
- **Wave H-1 (this week)**: binder BR staging buffer (grow buffer / bound by user read_size + return -EINTR short-read semantics), binder handle-0 desc bookkeeping, futex waiter-pool miss-path free, futex_pi bucket-lock on cleanup paths, ext4 rename-replace deferred deletion.
- **Wave H-2**: jbd2 revoke/commit j_head redesign; execve E2BIG/EFAULT; epoll interest lifetime on file description; icache dual-instance guard; MNT_NOEXEC/NOSUID enforcement points.
- **Wave M**: per-area Medium/Low items, batched by file to minimize churn; arch instrumentation gating (rv64 exception.rs hunt block → debug= switch); per-fault sfence → targeted sfence.vma vaddr.
- **Process actions**: (1) re-audit every "fixed" entry in review-coverage.md against main (the S-13 lesson: three documented fixes were never in the tree); (2) add dup2/close-vs-epoll and membarrier to smoke tests; (3) update review-coverage.md marks from the five coverage appendices.

## Coverage

All 458 `kernel/src` files enumerated; the five appendices record per-file status (✅ end-to-end / 🔶 hotspot-verified with prior-round full read / quality-pass for infra). Gaps honestly marked: aarch64 6 mm files (arch), mm page_desc/vma prior-round-verified regions (mm-proc). Coverage counts by the appendices: ~93% end-to-end this round, remainder covered by prior-round reads re-verified at their highest-risk regions.

## Post-merge gate verification (2026-11-09, main session)

The round (87ed94dc..f25124e3) was re-reviewed domain-by-domain and run
through the full merge gate after landing. Every P0/P1 finding below was
re-verified in code by the main session before any fix was written.

### Gate table (first pass, tree as landed + aarch64 build fix 8ec8c473)

| Gate | Result |
|---|---|
| Triple-arch build | x86_64 OK · riscv64 OK · aarch64 FAILED at f25124e3 (config.toml target-section rename; fixed 8ec8c473) |
| riscv64 regression boot | PASS (cd /proc rc=0, zero panics) |
| make test riscv64 | 1348/1348 |
| Tier-1 syscost (780 s) | census/tick/clock/GRQ/small_alloc PASS; nanosleep avg FAIL — adjudicated SEED-vehicle population artifact (n=5499, max 295 s; band applies to tok-class only, baseline L55/L370); p7_sleep1ms 15.98 µs = family reference |
| Tier-2 pbench | first attempt eaten by the boot-mount race (below); retry PASS attempt 4, but p6_stat SKIP (tree-build errno) — race-contaminated, re-run on fixed tree |
| LTP full scan #1 | PASS 585/1869 — INVALID as a round verdict: the pre-existing used.idx race turned every tst_tmpdir setup into EIO (505 PASS→FAIL) |

### The boot race (NOT a review4 regression — pre-existing, root-caused and fixed)

`wait_for_used_interruptible` read used.idx ONCE right after the response
byte was observed done; the device publishes resp-byte → used-entry →
used.idx → IRQ, and the pause-poll routinely lands in the gap, so
completed requests were classified "VirtIO request timeout" → EIO.
Faces: ~50% fresh-image mount [fail]; the LTP#1 mass mkdtemp EIO;
pbench p6 tree-build errno. Two noise-blind bisects (blaming 2ceb0fe7,
then a docs-only commit) are RETRACTED; repeated-boot statistics showed
the anchor 87ed94dc failing at the same rate. Fix dbbed246: bounded
grace read of used.idx at the five resp-done exits (hunt verification
18/18 + 6/6 here, vs 15ok/10fail unfixed).

### Re-audit fixes (three commits, all main-session verified)

- 8ec8c473 — aarch64 cargo target-section restore (P0 build break).
- 8ec51f23 — WTRACE gating hoist; io_uring offset-path RW_CHUNK staging
  (2 GB heap panic vector); mbind/set_mempolicy get_user (raw 8-byte
  user read → kernel fault); O_CREAT|O_EXCL answered before the
  dangling-symlink recursion; MNT_NOEXEC consults the numeric
  VfsMountInternal flags (the landed string check never fired).
- 0e48201f — ext4 rename-reap leak (d5fa104e stamped dtime=1 which the
  reap itself treats as already-done → permanent inode+blocks leak);
  tmpfs st_size inflation reverted; devfs cycle guard re-anchored on the
  moved child's subtree; ARCH_SET_FS/GS EPERM; faccessat2 AT_EACCESS;
  fchmodat2 AT_EMPTY_PATH; legacy-clone CLONE_PIDFD; epoll moved-fd
  refresh; jbd2 yield-under-commit-gate removed; binder staging
  head-budget bound (break-after-pop dropped whole work items); POSIX
  timers record tgid + deliver to any live member.
- dbbed246 — the used.idx grace fix above.

Marks in review4/*.md corrected to point at the real fixes (rename H-5,
MNT_NOEXEC, tmpfs, O_CREAT recursion, devfs cycle, binder staging H-2,
POSIX-timer scoping). Ledger defects found but not edited: summary
claims 11 High/42 Medium vs the area files' 10/43 (virt_to_phys
double-classified); per-area files carry FIXED marks for only 1 of 10
High blocks and neither Critical.

### LTP full scan #2 (dbbed246 = round + re-audit fixes, race fixed)

**PASS 1007/1869** (FAIL 347, TCONF 425, CRASH 59, EXECFAIL 17,
TIMEOUT 11, POISON 2) in 2441 s, zero boot stalls vs scan #1's seven
wedge-retry chunks. vs r27 (1084): **−77 net** (−86/+9), two open
clusters, both confirmed present on PURE f25124e3 (not caused by the
re-audit fixes):

1. **loop/mkfs family (~20 tests, deterministic)**: mke2fs on /dev/loop0
   cannot get device geometry, then fails EINVAL writing out —
   suspects ae7a0f7a (loop-0 canonical GenDisk registration) /
   2ceb0fe7 (sizelimit transfer cap). Hunt in progress; rename01/
   utime01/msync01 fail solo on f25124e3 and PASS solo on the anchor.
2. **SysV msg family (13+ tests, CRASH 254 with
   "ListHead::for_each: Too many iterations")**: msg tests PASS solo;
   the crash needs the same-boot context after msgstress01 (167
   children released to init) — a 1000-cap list walker breaking on a
   legitimately long list (fragility, not per-test semantics).

+9 gains include getdents×2, epoll, faccessat, ptrace, sched, timer,
futex — the round's real fixes at work. Scan #1's 585 is retained only
as the race's evidence artifact.

### Final gate re-run on dbbed246 (round + all re-audit fixes)

| Gate | Result |
|---|---|
| Triple-arch build | x86_64 / riscv64 / aarch64 all rc=0 |
| make test riscv64 | 1348/1348 (each fix batch) |
| Tier-1 syscost (780 s, KVM) | ALL PASS — panics 0, OOM 0, census 8/32 dead-center, ticks 125/s, clock_gettime 3 µs, **nanosleep avg 102.7 ms (n=40,083, tok-class — the enforceable population)**, GRQ hold 4.54 µs in band, small_alloc REST 0.3% |
| Tier-2 pbench (KVM, first-attempt DONE) | **8/8 PASS** — p1_null +0.4%, p1_clock +2.9%, p2_ctxsw +10.6%, p3b_cow +2.6%, p6_write_fsync +11.9% (in band); **p6_mkdrmdir −21.0%, p6_unlink −28.3%, p6_append −21.6% (improved)**; p7_sleep1ms 15.38 µs = family reference |
| LTP | 1007/1869 (above) — two open clusters hunting |

Perf-gate artifacts: run/perf-gate-r4fix-parse.txt,
work/pbench-r4fix-pb1-serial.log, run/r4review-ltp2-results.txt.
