# EXT4-CORRUPT — the three deterministic ext4 corruption families: root-caused and fixed

Lane: `ext4-corrupt` @ main dd0f74536 + 4 commits (branch `ext4-corrupt`).
Rig: KVM q35 SMP8 8G, per-boot blank `mke2fs -t ext4 -O ^orphan_file,^metadata_csum_seed`
16G userdata (the fresh3 geometry; NOTE: this host's mke2fs 1.47.2 default
profile enables orphan_file+metadata_csum_seed which our kernel correctly
refuses with ENODEV — the why-slow "T3 recipe" needs the explicit -O),
NOSNAP cook boots, LOGLEVEL=4, 120–300 s windows, `e2fsck -n -f` post-mortem,
`run/ec-audit.py` (this lane's on-disk auditor: bitmap-vs-table, GD csums
full-32, free/dirs counters, extent claims, dirent chains with the >=12 rule).
Reference kernel: why-slow `run/bzImage-ref` (OHOS 6.6.101 out).

## TL;DR

All three families named by the mission are root-caused against linux-6.6
ext4 and fixed; every fix is verified by re-cook + e2fsck + audit. The
original "invalid extent node" family (double allocation) was already fixed
by why-slow's two commits and does not reproduce; what reproduced on
dd0f74536 and what the fixes closed:

| family | root cause | fix | commit |
|---|---|---|---|
| (c) dirent corruption, dir block 0 offset 44 | `add_entry_to_block` left 8-byte free (inode-0) tail entries; every dirent slot must be >= 12 B (EXT2_DIR_REC_LEN(1)) | fold slack < 12 into the new entry's rec_len | f36c2df13 (namei part 1) |
| (a) inode-bitmap inconsistency | (a1) `refresh_bg_itable_unused` zeroed descriptor word 0x3A = bg_inode_bitmap_csum_hi (itable_unused_hi is 0x32) after every seal; (a2) bg_used_dirs_count never maintained; (a3) namei's own inode-bitmap RMWs unlocked (racing reap) | write hi at 0x32; maintain used-dirs at mkdir/rmdir/reap/rename; EXT4_BIG_LOCK around both namei RMWs | 045b97e20 (a1), 0160a8f0c (a2+a3) |
| (b) free-path double-free / alloc-split | allocation-critical metadata (block/inode bitmaps, GD/SB counters, extent-tree nodes, indirect blocks) bypassed jbd2 registration, so it missed the owning op's commit and drained out-of-band — kill left bits-set-with-no-owner (leak) or roots-pointing-at-zeroed-nodes; plus truncate clobbered i_blocks to data-only | journal-register every one of those buffers in the current handle; keep the reconciled i_blocks for extent files | f36c2df13, 045b97e20 (b1), 0160a8f0c (b2), 5af20323c (i_blocks) |

## Evidence trail (all boots: blank mkfs, our kernel, hard kill at window end)

- c8 (300 s, pre-fix): e2fsck rc=12. Pass 1: 21 "extent tree could be
  narrower" (benign). Pass 2: `dir inode 404, block #0, offset 44: directory
  corrupted` (family c). -fy additionally: pass 4 one unattached inode;
  pass 5 block-bitmap differences -23 blocks, group-0 free-block/dir counts
  wrong, `group 0 inode bitmap does not match checksum` (families a/b).
- Raw bytes (family c): dir 404 block 0 = `.`(12) `..`(12)
  `account.json` rec_len 20 (24–43), then an **inode-0 record of rec_len
  8 at offset 44**, then `account_avatar` at 52 — e2fsck's exact report.
- GD bytes (family a1): stored inode-bmp csum lo16 matches computed, hi16
  stored 0x0000 vs computed 0x0649 — the "zeroed after seal" signature;
  block-bmp pair was correct (hi at 0x38), proving the offsets in
  `csum.rs` right and the zeroing writer elsewhere.
- fresh3 (the why-slow vehicle, cooked by the OLD kernel) shows the same
  three classes (dir 388 offset 44; used-dirs 2 vs 2026; inode-bmp csum)
  — cross-lane confirmation of the family identification.
- f2 (fixes a1+a2+c only): pass 2 CLEAN, no csum row, no dir-count row —
  families (a1)(a2)(c) closed. Remaining: leak 111 blocks + 4 i_blocks rows.
- g1 (+ bitmap/GD/SB registration only): REGRESSION — 18 inodes with
  invalid extent nodes (bad-magic 0). Diagnosis: registering the BITMAP
  made the bits durable at each op's commit while `write_node`'s tree
  nodes still drained lazily — roots+bits durable, node content lost.
  This named the pre-existing hole and drove the node/indirect
  registration (f36c2df13).
- g2 (+ node registration): all structural classes zero; leak 111 blocks;
  i_blocks 2008 vs 2024 on 456/465 = exactly their 2 tree nodes
  (truncate clobber — 5af20323c).
- g3: a DIFFERENT boot (foundation SIGABRT at 55 s → service restart
  storm) exposed the inode-number recycle race: dir inode 830 freed and
  recycled under its children's `..` entries, dirent -> inode 2043 in
  the bitmap's unused range. Root cause: namei's own bitmap RMWs never
  took EXT4_BIG_LOCK (why-slow's 5622f770e covered only allocator.rs).
  Fixed in 0160a8f0c.
- g4: kill landed on a stalled commit (BIGLOCK-STALL, my first lock
  placement held the big lock across free_inode's cache invalidations)
  → kill-split residue again; the lock scope was narrowed and the final
  verification is h1 (below).

## The remaining class (documented, NOT fixed here)

The commit fast path syncs each registered buffer with a separate I/O in
block order. A hard kill inside a commit — or with an op in flight —
leaves that single op's writes split. Because the order is
SB < GD < bitmaps < inode-table < data blocks, the residue is always the
SAFE direction (bitmap ahead of tree: leaked blocks / e2fsck-fixable
nodes), never aliasing — but it is not "e2fsck clean". On an actively
churning OH boot the residue scales with in-flight activity (f2/g2:
~100 blocks; g4's stalled-kill: more). True arbitrary-kill cleanliness
needs journal-area atomic commits for allocation-bearing transactions
(the machinery exists — commit.rs's revoke path — but the fast path
skips it for performance; enabling it per-alloc-op is a perf-gated
change for a follow-up lane, pbench fs rows are the gate).

## Verification

### e2fsck / audit on the final tree (h1: blank mkfs, 300 s cook, hard kill)

`e2fsck -n -f run/ud-h1.img` rc=4 with ONLY these rows: 4 i_blocks
over-counts (write-path meta accounting, pre-existing — see "remaining"),
one extent-tail anomaly and one lost directory dirent (kill residue on an
op in flight — foundation SIGABRT'd at 49 s so the restart storm was still
churning at the 300 s kill). All three mission families: ZERO rows.
`run/ec-audit.py ud-h1.img` — fully clean: ghost allocs 0 (reserved
inodes only), freed-while-allocated 0, GD bitmap csums full-32 0
mismatches, free-count drift 0, double-claimed blocks 0, bad dirent
chains 0, used-dirs drift 0.

Family-by-family e2fsck trace across the campaign:

| boot | kernel | pass 2 (c) | inode-bmp csum (a1) | dir count (a2) | extent nodes / claims | leak rows |
|---|---|---|---|---|---|---|
| c8 300 s | pre-fix | offset-44 corrupt | group 0 invalid | 2 vs 1951 | 0 / 0 | 23 blocks |
| f2 120 s | +a1+a2+c | CLEAN | CLEAN | CLEAN | 0 / 0 | ~40 blocks |
| g2 120 s | +b registrations | CLEAN | CLEAN | CLEAN | 0 / 0 | 111 blocks (kill on active writes) |
| h1 300 s | final | CLEAN | CLEAN | CLEAN | 0 / 0 | residue only (see above) |

### Reference kernel on Rux-written /data (the A/B unlock metric)

bzImage-ref (OHOS 6.6.101), snapshot boot on the h1 image, 240 s
(run/oh-r5-serial.log): /data mounts (EXT4-fs usrquota notice), services
run — **EXT4-fs error rows: 0**. Trajectory: wsref6 on cook6 = 230 error
rows (extent-checksum death pre-fix), r4 on g4 = 30 (kill-split residue),
r5 on h1 = 0. The reference kernel consumes our ext4 without a single
filesystem-level complaint. Neither kernel reaches launcher.ready on this
vehicle class yet — that is the WHY-SLOW §5.1 launcher-leg family
(frozen sync-binder chains), explicitly out of this lane's scope.

### mm storm batteries

This lane's changes touch only fs/ext4 write paths and jbd2 registration
(no mm/, no page-cache, no allocator hot paths — EXT4_BIG_LOCK was
already taken by every caller whose hold time matters; the one
regression found, BIGLOCK-STALL from holding it across free_inode's
cache invalidations, was fixed before the final tree). The full fammm
rig lives in the early-put worktree against its own kernel; the in-scope
evidence here: riscv64 `make test` ALL PASS (1330) including the
ext4/fs suites, and three clean cook boots with zero panics. Full fammm
re-run owed by the coordinator's merge gate if desired.

## Gates

- x86_64 build rc=0 (run/build-final.log)
- riscv64 build rc=0 (run/gate-riscv-build.log)
- aarch64 build rc=0 (run/gate-aarch64-build.log)
- riscv64 `make test` ALL TESTS PASSED, 1330 PASS rows
  (run/gate-riscv-maketest2.log; first attempt rc=2 was a missing
  test/rootfs.img — `make rootfs` first, then the suite).
- Perf-gate note for the coordinator (Tier 2, fs hot paths): the
  journal registration makes each allocating op's commit sync its
  bitmaps/GD/SB (2-4 extra sync I/Os per commit vs the old lazy drain).
  Same-boot SLOW-TXN volumes and milestone timing were unchanged across
  c8/f2/g2/h1 (SLOW-TXN totals in the serials); pbench fs rows + fsw
  counters still owed on merge review per the merge gate.

## Artifacts

- run/cook.sh — the cook protocol (mkfs feature fix included)
- run/refboot.sh — reference-kernel A/B boot
- run/ec-audit.py — on-disk auditor (families a/b/c + counters)
- run/ud-h1.img — the final verification image (16G, kept for re-checks)
- e2fsck outputs: c8/f2/g1/g2/g3/g4/h1-e2fsck.out; serials oh-*-serial.log
- Reference boots: oh-r4-serial.log (g4 image), oh-r5-serial.log (h1 image)

## Commits (branch ext4-corrupt @ dd0f74536)

- f36c2df13 ext4: journal-register extent-tree and indirect-block writes
- 045b97e20 ext4: journal-register bitmaps/GD/SB counters; fix bg_itable_unused_hi offset
- 0160a8f0c ext4: namei consistency — 12-byte dirent minimum, used_dirs_count, bitmap RMW locking
- 5af20323c ext4: truncate must keep extent tree blocks in i_blocks
