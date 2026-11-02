# Review findings — r3b session (functional-correctness round, 2026-11-02)

Scope (coordinator assignment): kernel/src/fs/ (priority: vfs.rs — the
lock-free dentry walk from merge b28884e7 / commit 692e7623, writer-side
retirement audit; ext4/mod.rs, ext4/namei.rs, ext4/file.rs — tail_reserve
+ D-13 write contracts; bio.rs; procfs/mod.rs) + kernel/src/drivers/
(virtio/mod.rs, drm/mod.rs). Method: line-by-line against the man-page /
reference contracts and the in-repo R-numbered comments; every candidate
re-derived before claiming; fixes gated (builds + suite + host mirror).

## 1. Lock-free dentry walk (692e7623) — writer-side audit verdict

**Protocol SOUND; one writer-side bug found and fixed (f62a4df6).**

Verified line-by-line in dentry.rs + the path_lookup walk in vfs.rs:

- **Reader banks**: SeqCst claim RMW (fetch_add) before any pointer
  load; Release decrement on Drop; drain's SeqCst fence + Acquire bank
  scan closes the store-load hole. I re-derived the C++20 memory-model
  argument: a fresh reader that misses the scan (claim RMW after the
  drain's fence in S) cannot load a pre-mutation version — deriving
  both Mb <S F_W (via the reader's stale ver read coherence-ordering
  Wv) and F_W <S Mb (via the scan's 0-read of the bank) yields the
  contradiction. The SeqCst RMW + SeqCst fence pairing is exactly what
  FdTable's get_file/drain_retired argument requires — generalized
  correctly.
- **Strong-count discipline**: every raw→owned promotion
  (dentry_arc_from_raw / inode_arc_from_raw, 8 sites in vfs.rs) runs
  while the claim is held; claim dropped only after; arms that sleep
  (fs lookup miss, symlink follow) materialize owned Arcs first and
  null the carried inode pointer. Early returns drop the claim via Drop.
- **Writer coverage**: every Dentry children/inode mutation goes through
  add_child/remove_child/clear_children/rehome_children_into/
  set_inode/clear_inode (grep-audited: the sysfs/tmpfs/rootfs/procfs/
  debugfs `children` hits are their own node types; vfs.rs:5869 is a
  read). All bump children_ver and retire displaced Arcs.
- **rename/unlink/rmdir publication** (vfs_rename_locked,
  vfs_rmdir/vfs_unlink): replaced-entry kill = set_negative →
  clear_inode (nulls mirror, retires) → remove_child (bump+retire);
  the moved dentry re-publishes via add_child (bump). Racy memo hits
  serve either the pre- or post-state — both legal linearizations
  (memory-safety via the retire/claim handshake). In-place name change
  is under the name spinlock; the old parent's memo dies with the ver
  bump. Correct.
- **Drain internals**: lock order SLOW_READ → RET_DENTRY → RET_INODE
  consistent; take-guards released before the drops (no re-entry
  hazard); DENTRY_DRAIN_DEFERS bounded, deferral is safety-neutral.
- **The bug (fixed)**: the drain dropped the retired vecs while holding
  DENTRY_SLOW_READ under lock_irqsave. The drop cascade frees whole
  dentry subtrees and runs Inode::drop → ext4_destroy_inode →
  inode::read_inode → bio::bread — synchronous disk I/O with IRQs and
  preemption off: virtio's wait degrades to poll-only (1e6-iteration
  MMIO spin + read_block's 5e7-iteration late scan), a cli segment
  bounded only by device latency (IRQ gate class is 3-7 ms), tick/IPI
  loss, watchdog bait. Reachable from every unlink/rmdir that makes the
  retired inode the last reference (vfs_rmdir/vfs_unlink call
  icache_remove BEFORE clear_inode) and from walk-side icache-refresh
  set_inode retires. Fix: take the vecs under the slow lock (serializing
  with slow-path readers), drop AFTER the lock scope — the quiescence
  proof (banks zero at scan + fresh readers can't reach the Arcs) never
  needed the lock held during the drop. Suite 1291/1291.
- **Pre-existing, filed (see finding F-r3b-3)**: the same I/O-under-
  spinlocks shape exists in the OLD contexts too (icache eviction,
  dcache bucket drops, the `*inode.lock() = None` era) — those hold
  preempt off but NOT irqs; the drain was the first IRQs-off instance.

## 2. ext4 write-path contracts — verdict

- **tail_reserve (12-byte metadata_csum dir tail)**: find_entry_space /
  add_entry_to_block / create_initial_entry threading re-verified. The
  deleted-slot arm's `offset + required_len <= block_size - tail_reserve`
  and the split arm's `new_offset + new_rec_len > usable_end` bound
  (line ~940) close the tail-overlap window even for legacy-geometry
  blocks whose chains end past block_size-12 (falls through to the next
  block on Err; the mutation happens on the CLONE). dir_block_seal
  re-derives the tail after every hosting write. PASS.
- **D-13 batched publication**: PENDING_PUBLICATIONS is crash-ordering
  machinery only — live reads go through the buffer cache, which holds
  current state regardless of queue depth. ENOSPC mid-batch leaves
  ordered pending records + a crash window, exactly the registered
  D-13 stance (POSIX never promised per-op dir durability). PUB_BATCH
  drain runs lock-free at journal-stop. No functional issue.
- **BUG (fixed, 6a83a090)**: ext4_file_write returned Err(ENOSPC) from
  the per-block loop even after earlier blocks of the SAME request had
  landed — write(2) and the reference kernel report the partial count
  when some bytes landed (only a request that wrote nothing fails).
  Reachable shape: write into an interior HOLE of a sparse file on a
  full disk (appends can't hit it: the pre-allocation head covers past-
  EOF blocks and errors before any data). All mid-loop failure arms
  (hole/block alloc, extent materialize, buffer fetch) now degrade to
  partial success once total_written > 0; the size tail is
  partial-safe. A/B: host-mirror proptest (kernel/verify
  fs::ext4::write_partial_test, POSIX oracle over random layouts) —
  old body fails 3/4 tests, fixed body 4/4 (verify 1118/1118); guest
  rig blocked by pre-existing wedge F-r3b-1 (identical before/after =
  no-regression signal only). Suite 1291/1291.

## 3. Filed (not fixed this session)

- **F-r3b-1 (pre-existing on main 819a1be2, HIGH for ENOSPC workloads):
  write-to-ENOSPC wedges the kernel.** Guest-side filling of a full ext4
  (fresh tiny image, init-probe boot) wedges: the writer task found
  holding preempt_count 0x10002 (SOFTIRQ_OFFSET + 2 spinlock guards) for
  1-18 s repeatedly; PREEMPT-HEAL resets it, the guards then underflow
  (unpaired decrement clamped, spinlock.rs:0x1b8 = the lock_irqsave
  drop), loops forever; used ring caught up (device idle, no VW lag) so
  the waiters are not stuck on the device. Reproduces with a long fill
  AND with only 108 free blocks (≈25 syscalls) — it is the ENOSPC path
  itself, not volume. Identical on main 819a1be2 and this branch (both
  logs preserved). Rig: run/r3b_enospc.c + run/r3b_enospc_boot.py;
  serial log run/r3b_enospc_serial.log; image run/r3b_enospc.img.
  Suspect family: a spin section in the allocation-failure / dirty-
  eviction arm that cannot complete with IRQs off (same class as the
  drain bug fixed above, different site — preempt 0x10002 points at
  bh_disable + 2 locks). Needs its own hunt lane.
- **F-r3b-2 (pre-existing): guest-fill dirty storms wedge the box** —
  the documented WEDGE family (bio.rs Phase 1.5 comment) still bites on
  sustained create/write-to-full under TCG; subsumed by F-r3b-1's
  evidence (the 24M-image run wedged mid-fill before any ENOSPC).
- **F-r3b-3 (pre-existing, low): fs destroy hooks run I/O under
  preempt-off in older drop contexts** — icache eviction, dcache bucket
  replacement and the pre-692e7623 `*inode.lock() = None` writes ran
  ext4_destroy_inode's read_inode with preempt disabled (virtio
  poll-only wait). The drain fix removes the only IRQs-off instance;
  the preempt-off instances remain (bounded by the 1e6 poll budget).
- **F-r3b-4 (noted): ext4_destroy_inode's comment claims "queueing
  avoids I/O in this Drop context" but read_inode IS a disk read in
  that Drop context** — only the reap is deferred. Comment/code drift;
  harmless once the drop contexts stay preemptible-on (F-r3b-1's fix
  direction).

## 4. drm/mod.rs

GEM lifecycle re-read (the cr2 lane's coverage extended):
- **BUG (fixed, 2b2a4a52)**: gem_close returned -EINVAL when the handle
  EXISTS but the object is still shared (Arc::strong_count > 1) — the
  None sentinel conflated "keep for other holders" with "not found".
  The reference's per-file handle tables make a close never fail
  because another file shares the OBJECT; Mesa's GEM_CLOSE on shared
  buffers (the oh-glm5 EGLImage shape the shared-keep proxy was built
  for) must see 0. Outcome split into Removed/SharedKeep/NotFound; the
  object drop still runs outside the state lock (RESOURCE_UNREF polls
  for ms). Contract-gated (builds x2 + suite; the DRM probe boot is the
  drivers lane's rig, not rerun here).

## 5. Other files this session (verdicts)

- **virtio/mod.rs**: sync read/write paths (R8-M2 status, R22-6
  dealloc, the R8-M2 late-completion bounded spin), async completion
  walker (positional fast path + device-truthed fallback, tombstones,
  ABANDON-vs-DELIVERY, lossless dispatch re-raise, VIRTIO-WQ-1
  collect/deliver split), vw lag watchdog — all honor their R
  contracts. No defect.
- **bio.rs**: bread/get (R9 dead-bh guard, failed-async residue,
  evicting-skip R7-C1), getblk_overwrite/drop_freed paths, publication
  queue + PUB_BATCH/PUB_HIGH_WATER drains — re-read in the D-13
  context; consistent with the cr3 verdicts.
- **procfs/mod.rs**: ino scheme (100..2524 ranges collision-free vs
  FD_DIR 200 / fd links 300..1324 / ns 410..416 / attr 510..516 /
  task 620 / fdinfo 1400/1500..2524), node types, root pid-dir
  fabrication, refcount get/put — no defect in the reviewed surface
  (init trees + generators not re-derived this session).

## Gates summary

- make build (riscv64) rc=0, make build-x86 rc=0 — per fix and at tip.
- riscv64 suite: 1291/1291 at tip (run/r3b-maketest-final.log).
- make verify: 1118/1118 (4 new partial-write mirror tests).
- Commits: f62a4df6, 6a83a090, 2b2a4a52 on feature/review-r3b.
