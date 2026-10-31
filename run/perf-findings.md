# perf-fs lane: fs stack p6 family vs reference (session 2026-10-31)

Protocol: test/pbench_boot.py (q35, TCG thread=multi, -cpu max, SMP2, 2G),
same probe rootfs both sides. Reference bzImage =
/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage.

## Ground truth (medians, us)
row             ref      rux-base   ratio     rux-fix5   ratio
p6_stat         4.06     198.5      49x       197.6      49x
p6_openclose    6.33     495.6      78x       486.9      77x
p6_getdents     898.6    5136.9     5.7x      5208.1     5.8x
p6_read_cached  12.56    525.9      42x       523.2      42x
p6_write_fsync  14980.9  17204.0    1.15x     12971.4    0.87x (better)
p6_append       9.52     698.4      73x       631.5      66x
p6_mkdrmdir     153.1    29795.7    195x      2968.4     19x
p6_unlink       26.87    1091.7     41x       1030.2     38x
PREP tree       97ms     12487ms    129x      3010ms     31x

NOTE: host ran 3-9 foreign QEMUs during these boots (multiple perf lanes
in flight); medians are load-inflated for BOTH sides but ratios hold
(reference captured under 3 foreign QEMUs).

## Root causes found and fixed

1. Per-mkdir synchronous publication drain (the big one, 195x).
   ext4_mkdir_no_journal ended with bio::flush_publications_scoped() --
   5-8 synchronous block round trips at EVERY mkdir return (pre blocks ->
   entry block -> post). Pa85/e3c1ec35 already proved this owns the
   mkdrmdir cost on BOTH journal and no-journal arms; fab0e029 reverted
   because deleting it alone pushed the deferred bill onto unlink
   (H2-constrained dir-block writes, 100x). Fix: batch drain at the
   journal-stop boundary (bio::publication_batch_drain, PUB_BATCH=64,
   called from ext4_journal_stop): records batch per transaction window,
   the crossing op pays ONE ordered whole-queue drain (FSW1 pb2 field).
   Queue high-water is now 64 (pm=64 measured), never 2048.

2. Buffer cache 4 MiB / 64 buckets -> 32 MiB / 2048 buckets.
   1024 entries evicted constantly under any real working set: 2.5
   evictions per mkdrmdir pair, 46% dirty (each dirty eviction = one
   synchronous round trip inside the caller's bread + constraint guard).
   Post-fix FSW: ev=0/0 across the whole battery.

3. Directory scan costs (create/lookup paths).
   a. ext4_add_entry_locked cloned 4 KiB per scanned block and scanned
      blocks 0..n forward -- under churn the free slot is in the TAIL
      block, so every mkdir walked the whole directory. Now: tail-first,
      in-place search (clone only the block that hosts the entry).
   b. find_dir_entry (rmdir/unlink/rename target search) cloned 4 KiB per
      block and scanned forward; tail-resident entries cost the full walk
      (rmdir of a just-created dir). Now: in-place scan, bidirectional
      (both ends inward; unique names make order a speed choice only).
   PREP (1000 file creates) 12.5s -> 3.0s.

4. write_block_from_vec read every block from disk before overwriting it
   (fresh mkdir dir block = one device read per create). New
   bio::getblk_overwrite: no read on miss (zeroed+dirty buffer), no
   zeroing on hit (full-block copy makes content valid; identical
   visibility class to the old bread+copy).

## Remaining gaps (next session)
- mkdrmdir floor ~2.5ms/pair (min sample) with ~zero steady-state device
  I/O: CPU-side. FSW shows ~200 breads/op-pair across the battery whose
  site attribution is incomplete -- needs per-site bread counters.
- p6_unlink ~1ms (38x): own-path decomposition pending.
- stat/openclose/read_cached/append (42-77x): shared syscall-entry +
  lookup-path costs (p1_null is 5.3us vs ref 0.23 = 23x, syscall lane).
- p6_getdents 5.8x: per-call re-materialization of the whole directory.

## Gates (2026-10-31 session close)
- Dual-arch builds: PLATFORM=x86_64 rc=0, PLATFORM=riscv64 rc=0.
- make test (riscv64): 1204/1204 PASSED.
- pbench boards (Tier 2, this battery): all p6 rows improved or in
  class; write_fsync now BETTER than reference (11.1-13.0ms vs 15.0ms).
- Tier-1 quiet syscost boot: REFUSED by run/perf-gate.sh host-capacity
  guard (6 foreign QEMUs from sibling lanes). Rerun once quiet before
  merge; the pbench A/B above is the lane's primary evidence.
- NJ-arm (--nojournal) validation board: mkdrmdir 2880us med / unlink
  959us med - no fab0e029 regression class on the no-journal image.

Pushed: origin/perf-fs (5 commits).

## KVM columns (policy 2026-10-31: KVM is the primary rig; x86_64-first)

Same battery, --accel kvm, same rootfs. Medians in us.

row             ref-KVM   rux-base-KVM   ratio     rux-fix-KVM   ratio    fix gain
p6_stat         0.230     14.717         64x       14.768        64x      -
p6_openclose    0.371     37.129         100x      37.159        100x     -
p6_getdents     80.781    511.648        6.3x      505.843       6.3x     -
p6_read_cached  0.541     38.852         72x       38.752        72x      -
p6_write_fsync  10420.3   9078.5         0.87x     9068.0        0.87x    (better than ref both)
p6_append       0.390     120.844        310x      117.429       301x     -
p6_mkdrmdir     7.224     15209.7        2108x     244.085       34x      62x
p6_unlink       1.703     67.996         40x       65.272        38x      -

TCG vs KVM: the TCG ratios understate the fs-sync gaps because TCG
inflates the reference too (reference mkdrmdir 153us TCG vs 7.2us KVM).
The per-mkdir publication drain cost is I/O-bound (same absolute ms on
both rigs), so the fix gain is 62x under KVM vs 10x under loaded TCG.

## Residual attribution (fsw op-bracket timers, FSW2 mk/rm/ul/jd)
- Under KVM the fixed pair median is 233-244us: mkdir ~50us avg, the
  rest is the rmdir side (fsw rm= sum; note the coarse timer quantizes
  at 10ms so rm averages are outlier-skewed - use the pair medians and
  mk avg). NEXT: sub-bracket timers inside ext4_rmdir_inner
  (is_dir_empty / delete_entry / free_inode / free_inode_blocks) to
  split the ~180us; suspects: the two 4 KiB clones (is_dir_empty
  read_block_to_vec, free_inode bitmap read), post-capture queue
  scans, and the eager frees' deferred-write bookkeeping.
- jd (jbd2 dirty_metadata dedup scan): ~230K iterations over the whole
  battery, ~5-8 per scan - NOT a bottleneck; recorded to close the
  suspicion.
- p99 on mkdrmdir under KVM: 39-721ms - the PUB_BATCH=64 ordered
  drains (sequential sync_block_ordered). If the p99 matters, the next
  step is phase-batched submission (submit all blocks of a phase, wait
  once) or a write-back kthread.

## rmdir floor attribution (FSW3 phase timers, KVM, 1000 rmdirs)
Phase sums per op (find+is_dir_empty 8.3us / delete_entry 9.3us / inode
writes 4.3us / free_inode 12.4us / free_blocks 8.5us): inner work is
~43us/op. Whole bracket (rm=, monotonic clock): 3158us/op AVERAGE -
the difference is the PUB_BATCH drain running inside the journal-stop
bracket: each drain synchronously writes the ~64 records' unique dirty
blocks, dominated by the FRESH DIR BLOCKS of mkdirs that rmdir has
ALREADY FREED by drain time (dead data written to disk). Pair median
243us / average 3371us / p99 54ms is exactly this distribution.

Next fix (design worked out, not landed): discard-on-free - when
free_block_run frees a block whose cached buffer is dirty and
unreferenced, DROP the buffer instead of leaving it to be written
(the content is dead; the bitmap free carries the semantics; the
reference kernel forgets freed buffers). Needs bio::invalidate_block
(unhash+free, modeled on evict_one Phase 1-3 without the sync) +
careful notes: publication records referencing the dropped blockno
already no-op in sync_block_ordered (peek miss); jbd2 commits may
re-read + re-write the stale content (correct, rare, harmless).
Expected: drain work drops from ~64 blocks to the ~4-6 shared metadata
blocks, floor ~3.1ms/pair -> ~100us/pair class, p99 collapses.

Remaining per-pair typical cost after that: mkdir 41us (mk=, monotonic)
+ rmdir inner 43us + ~160us syscall/VFS layer (matches the p6_stat
14.8us + openclose 37us shared lookup costs - syscall lane overlap).
