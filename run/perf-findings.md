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
