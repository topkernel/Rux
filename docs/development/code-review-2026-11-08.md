# Rux Kernel Code Review — Round 4 (2026-11-08)

> Scope: full-tree, five parallel review agents, zero-coverage-gap partition.
> 458 files / 330K lines in `kernel/src` reviewed across arch (riscv64 + x86_64 + aarch64), fs, mm/process/sched/sync, syscall/ipc/net/io_uring, and drivers + debug/trace/perf + top-level + build/test infrastructure.
> Detailed reports: [review4/arch.md](review4/arch.md), [review4/fs.md](review4/fs.md), [review4/mm-proc.md](review4/mm-proc.md), [review4/syscall.md](review4/syscall.md), [review4/drivers-misc.md](review4/drivers-misc.md) — each with a per-file coverage appendix.
> Dedup: findings were checked against `review-coverage.md`'s fixed-defect table and the 2026-09-12 review; three Sept-round fixes were found **still live in the tree** and re-filed (see S-13 below).
> Reference: Linux at `/home/william/refer/linux/` (6.x), virtio 1.1 spec, ARM DDI 0487, RFC 793.

## Totals

| Severity | Count | Notes |
|---|---|---|
| **Critical** | 2 | dentry-drop-under-lock regression; IPI csd container_of (code-exec) |
| **High** | 11 | incl. DRM prime mmap = arbitrary phys R/W; binder BR stream desync; ext4 rename-over-open-file frees blocks |
| **Medium** | 42 | |
| **Low** | 67 | |
| **Info** | 40 | design observations |
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
- **[arch, all three]** non-canonical-VA guard tests the VirtAddr-masked address (half the class escapes; x86 does it right on raw CR2); `flush_tlb_others`/membarrier targets CPUs `[0, count)` instead of the started set.
- **[arch/riscv64]** exception.rs carries ~250 lines of ungated hunt instrumentation firing ~40 pr_err per user exec fault; `map_page` does a full `sfence.vma` per demand fault, defeating the ASID layer.
- **[fs]** icache hash-collision overwrite drops still-referenced inodes (dual-instance class); tmpfs hard links don't share size; MNT_NOEXEC/NOSUID parsed but never enforced; devfs rename has no cycle guard; jbd2 has no time-based commit (unbounded metadata crash window).
- **[syscall]** execve truncates argv ✅ FIXED (`8952fd96`) at 8KB and drops the rest of the array on mid-read fault (should be E2BIG/EFAULT); clone3 CLONE_PIDFD probes the word but never writes the fd; epoll dead-interest purge keys on fd number ✅ FIXED (`8952fd96`) (dup2+close removes a live watch — Linux keys on the file description's last ref).
- **[drivers]** virtio_pci Phase-3 drain counts any chain's used-ring entry as completion (UAF window on 64-byte io_buf, R21-N2 class — should match head_desc like the MMIO twin); virtio_net xmit spins for TX completion under `tx_queue.lock_irqsave` per packet; printk /dev/kmsg heap-allocates under the ring irqsave lock; TSC resync's >25s rebase arm unreachable (else-if shadowing).
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
