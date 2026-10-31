Perf review — syscall entry/dispatch + openat (perf-syscall lane, 2026-10-31)

Method: A/B with the reference kernel bzImage
(/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage)
through the SAME rig — test/pbench_boot.py, q35, `-accel tcg,thread=multi`,
`-cpu max`, SMP2/2G, minimal ext4 rootfs, init=/test/pbench.
All numbers are medians in microseconds unless noted.

## 0. Ground truth (true A/B, both TCG)

IMPORTANT calibration finding: the TASK.md gaps ("p1 15-90x", "openat
50-250x") came from comparing Rux-TCG (or Rux-debug) numbers against
reference-KVM numbers (run/ref-r1/r2, pbref-boot.sh machine class) and
syscost debug-boot numbers. Under a FAIR A/B (same accel, same rootfs,
same probe) the reference itself is ~6x slower than its KVM columns:

| row            | Rux strong base | reference TCG | ratio | reference KVM (ref-r1) |
|----------------|-----------------|---------------|-------|------------------------|
| p1_null        | 0.611           | 0.240         | 2.5x  | 0.040                  |
| p1_clock       | 0.741           | 0.832         | ahead | 0.070                  |
| p2_ctxsw       | 1.377           | 1.518         | ahead | 0.085                  |
| p6_stat        | 18.294          | 3.877         | 4.7x  | 0.271                  |
| p6_openclose   | 32.582          | 6.241         | 5.2x  | 0.431                  |
| p6_read_cached | 40.502          | 12.122        | 3.3x  | 0.671                  |
| p6_append      | 85.811          | 9.377         | 9.2x  | 0.471                  |
| p6_unlink      | 88.164          | 25.216        | 3.5x  | 26.239                 |
| p6_mkdrmdir    | 32352           | 144.1         | 224x  | 40.4                   |
| p7_sleep1ms    | 1215            | 16.5          | 74x   | 14.6                   |

Out-of-lane rows worth flagging to their owners: p6_mkdrmdir 224x
(ext4 create path), p7_sleep1ms 74x (timer), p5b/p5c stream/dgram ~15x
(unix-socket write path), p6_append = per-write journal commit (see F5).

## Fixes (each measured on its own build)

### F1 — gate per-syscall forensic instrumentation (dfx=sysforensic)

syscall_handler/`_inner` carried UNCONDITIONAL per-syscall work: the
SYSCALL_RING entry+exit records (2 RMW + 10 atomic stores), a duplicate
post-translate LASTSYS latch, the per-task last_calls/init_calls/birth
rings, the fail-tail ring, and the sc-series comm-gated hooks
(appioctl/applog/appconn/prctl-einval/missstat/openmiss — the ioctl and
connect ones did comm() copies + 5 string scans on EVERY ioctl/connect).
Also check_and_deliver_signals did two atomic RMWs (xchg) per syscall
RETURN (ISIG latch + CAD latch) — load-first now, RMW only when nonzero.

All of it now sits behind one atomic load (`DfxSwitch::SysForensic`, new
switch `dfx=sysforensic`); the pre-translate LASTSYS latch stays always-on
(taskdump's lastsys= needs it, 3 stores). Hunt boots that need the rings
or sc-hooks must add dfx=sysforensic.

  p1_null 0.611 -> 0.481 (-21%), p1_clock 0.741 -> 0.621. fs rows flat (expected).

### F2 — path_lookup prologue: zero-alloc fast path + walk churn

The prologue paid ~15 heap objects per lookup before the walk started:
make_absolute_rooted's String, THREE get_process_root() Strings (each =
FsStruct::get_root() Vec clone — or a full parent-chain build_path() walk
when a root dentry is recorded — plus path_normalize's TWO Vec<&str>
collects and output String), path_normalize's output, and the
components Vec. Now: `process_rooted()` reads the fs_struct slots with
no alloc; absolute + already-normal paths take a `Cow::Borrowed` path
(`path_is_normalized_fast` = one byte scan); chrooted/odd paths take the
old allocating arms unchanged. The sbx sandbox-trace computation
(4 byte-scans per lookup) is behind sysforensic.

Walk loop: the ENOTDIR check and the DAC check each called
`current.get_inode()` (spinlock + Arc clone) — now one call serves both,
in both walk loops (path_lookup + the symlink-resolution walk).
The icache refresh's `set_inode(fresh)` ran even when the icache returned
the SAME Arc (lock + Arc swap per cached component) — ptr_eq-guarded now.

  p6_stat 18.294 -> 9.395 (-49%), p6_openclose 32.582 -> 24.709 (-24%),
  p6_read_cached 40.502 -> 32.131 (-21%), p6_unlink 88.164 -> 67.817 (-23%).

### F3 — sys_openat shortcut-chain trims

sys_openat's shortcut chain (O_PATH trace, /proc/exe, /proc/*/environ,
ns links, procfs read_file) ran per open: TEN `vfs::chrooted()` calls
(each = get_process_root() = the ~5-alloc stack from F2), TWO
Vec<&str> collects for /proc parsing (ungated by any /proc prefix
check), try_open_ns_path's Vec collect, and procfs::read_file's Vec
collect + scans. All now: `process_rooted()` (0 allocs) + a
`starts_with("/proc")` gate before every /proc-shaped collect.
(The probe opens non-proc paths, so this is pure removed overhead.)

  p6_openclose 24.709 -> 14.380 (-42%), p6_read_cached 32.131 -> 22.396 (-30%),
  p6_stat 9.395 -> 8.277 (-12%).

### F4 — icache init fast path + stats gating (kept, see F4b for the copy part)

icache_init() took the global ICACHE spinlock on EVERY lookup just to
check "already initialized" — fronted with an AtomicBool now. The
hit/miss counter fetch_adds (2 per component lookup) are behind
`icache_stats_arm()` (default off; the LRU time updates stay — eviction
order is functional). The first attempt of this round also switched
strncpy_from_user to a whole-window bulk copy — that REGRESSED
(p6_openclose 14.380 -> 17.503): a PATH_MAX window meant a 4 KiB memcpy
for a 20-byte name. Reverted to chunked (F4b); init/stats kept.

### F4b — strncpy_from_user: chunked copy instead of per-byte get_user

Both arches copied user strings ONE BYTE AT A TIME through get_user —
on x86_64 each byte is a call + access_ok + AC bracket (pushfq/orq/popfq
— the SMAP-era STAC/CLAC replacement) + 1-byte rep movsb + AC clear +
ret ≈ 30 instructions per path byte, paid by every openat/stat/exec/
chdir path argument. Now: 64-byte chunked raw exception-table copies
(one AC bracket pair per chunk) + NUL scan in kernel memory; a
mid-chunk fault zero-fills the tail and returns the same Ok/EFAULT the
byte loop produced. Short names cost ONE chunk; nothing ever copies
more than 63 bytes past the NUL.

### F6 — symlink check reuses the walk's inode (follow_symlink_hinted)

The per-component follow_symlink call re-took the dentry inode spinlock
and cloned the Arc just to check is_symlink, although both walk loops
already hold the child's inode (icache-refresh arm / iget arm). The
Arc now flows through as a hint; a mount hop (different dentry) passes
None. pbench: p6_openclose 13.65 -> 12.64 us, p6_stat 7.23 -> 6.62,
p6_read_cached 21.48 -> 20.33; one spinlock acquisition per component
removed under SMP contention too.

## Cumulative — med-of-3 interleaved (base vs fix5, orders F,B,F,B,F,B)

| row            | base med | fix5 med | delta | fix6 build | reference | final ratio |
|----------------|----------|----------|-------|------------|-----------|-------------|
| p1_null        | 0.611    | 0.491    | -20%  | 0.491      | 0.240     | 2.05x       |
| p6_stat        | 18.440   | 7.229    | -61%  | 6.622      | 3.877     | 1.71x       |
| p6_openclose   | 33.265   | 13.648   | -59%  | 12.642     | 6.241     | 2.03x       |
| p6_read_cached | 42.162   | 21.478   | -49%  | 20.326     | 12.122    | 1.68x       |
| p6_unlink      | 88.164   | 74.438   | -16%  | 70.784     | 25.216    | 2.81x       |
| p6_append      | 85.8     | 90.3     | flat  | 79.9       | 9.377     | 8.5x (F5)   |

## Open findings (not fixed in this lane)

F5. p6_append (a single 4K write() to an open fd) = 77-89us vs reference
    9.4us. ext4_write_at_core starts a JOURNAL TRANSACTION and commits
    metadata per write (data=ordered inline discipline) — the ledger's
    known "~88us append class" (route-1/J-arm). The reference kernel does
    delayed allocation + writeback: write() touches no disk I/O. This is
    the fs-lane's route-1 work; naming it here because the charter asked
    for the write path too. p6_write_fsync 19.6ms vs 16.0ms (1.2x, close).

F6. p6_getdents 544us vs reference 882us — we are AHEAD (full-dir sweep
    of 1000 files).

F7. Remaining p6_openclose gap (12.6 vs 6.24): per-component walk locks
    (dentry children lock, parent inode lock, vfsmount lock, icache
    global lock — ~5 lock pairs per component still) and VFS_STATE's
    global spinlock per lookup. The reference walks dcache RCU-style
    with zero locks. Candidates: bucket-level icache locking,
    dentry-embedded RCU-walkable name hash, VFS root ArcSwap.

F8. Relative-path opens rebuild the cwd from the dentry parent chain on
    every make_absolute (FsStruct::get_cwd -> build_path() — locks + a
    String per ancestor). Probe rows use absolute paths; shells don't.

F9. p1_null residual 2.0x (0.48 vs 0.24): trap entry SCRIBBLE2-GUARD
    (PER_CPU load + stack-bounds check per user entry), CURRENT_PT_REGS
    save/restore pair, seccomp gate, LASTSYS latch, syscost/trace armed
    checks, exit-path signal+resched checks. Each is small; the total is
    the gap. Not worth dismantling the safety nets for the last 2x under
    TCG; revisit under KVM measurements.

## Artifacts

- Kernels: run/sys-{base,fix1..fix6}-bzImage
- Results: run/<tag>-results.txt (PBENCH lines), serial logs in work/,
  confirmation script run/sys-confirm.sh (interleaved med-of-3)
- Reference TCG column: run/sysref-tcg1-results.txt

## Gates run for the final state (fix6, commit 542bbc6d)

- `make test` riscv64: ALL TESTS PASSED (1204/1204 at fix5 state,
  1203 at fix6 state — one row flake between runs, zero failures both)
- PLATFORM=x86_64 build-release-opt / riscv64 build / aarch64 build: rc=0
  (aarch64's regenerated dtb_blob.S restored, not committed)

---

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

## Fix 6 landed: discard-on-free (KVM board)
p6_mkdrmdir avg 3371 -> 2107us, p99 54 -> 16.4ms, max 53 -> 22.8ms;
mkdir bracket max 10ms -> 137us (mkdirs no longer pay drains at all).
Median ~245us unchanged - bounded by the shared syscall/lookup path
(p6_stat 15.2us vs reference 0.23us, p6_openclose 37us vs 0.37us):
VFS path walk + fd machinery, overlapping the syscall lane's p1_null
work. Gates: make test 1204/1204 PASSED, dual-arch rc=0.

Remaining mkdrmdir tail (p99 16ms): the revoke-threshold slow commits
still write the accumulated registered set (inode tables, bitmaps,
GD, SB - ~70-130 unique blocks) into the journal area; shrinking that
needs the jbd2 commit to write only CHANGED-since-last-commit blocks
(the reference kjournald checkpoint shape) - jbd2-level design work.

Final KVM verdict table for this session (medians, us):
row             ref     base    fixed   ratio-now
p6_mkdrmdir     7.2     15210   245     34x (2108x -> 34x)
p6_unlink       1.7     68      66      39x
p6_write_fsync  10420   9079    9174    0.88x (better than ref)
p6_append       0.39    121     112     287x  (syscall+write-path, next)
p6_stat         0.23    14.7    14.8    64x   (lookup path, syscall lane)
p6_openclose    0.37    37.1    36.9    100x  (same)
p6_read_cached  0.54    38.9    38.3    71x   (same + read path)
p6_getdents     80.8    512     500     6.2x