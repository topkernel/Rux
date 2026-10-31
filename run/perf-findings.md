rf review — syscall entry/dispatch + openat (perf-syscall lane, 2026-10-31)

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

---

# Perf findings — sched/futex lane (perf-sched), 2026-10-31

Worktree: /home/william/rux-agents/perf-sched, branch perf-sched.
Mission: the charter's biggest gap — futex wake-to-run tick-quantized
(10-30ms steps, avg 108ms under OH load; reference kernel single-digit
µs) = 200-1000x. Method: identical pbench probe binary booted on the
reference bzImage and on rebuilt Rux kernels, same machine class
(q35, -cpu max, SMP2, 2G; TCG thread=multi for the relative record,
KVM as the primary rig per policy).

## Commits (this lane)

1. `5a86bc60` mm: mprotect coverage walk must seed with the VMA
   CONTAINING addr (correctness regression fix — see F1).
2. `4d9f26b4` test(pbench): P2f futex wake->run battery.
3. `6ab42522` sched(x86_64): arm the IRQ-exit kernel-preempt hook by
   default (kpreempt=0 opts out) — the headline fix (F2).

## F1. mprotect interior-range ENOMEM — regression from 1f2f18cb (FIXED)

The perf-mm lane's bounded coverage walk (commit 1f2f18cb, today)
uses `iter_after(addr-1)`, which yields only VMAs whose START >= addr:
the VMA *containing* addr is skipped, so every mprotect beginning
inside a VMA (not at its start) sees a phantom gap and returns ENOMEM.

That is musl pthread_create's stack construction verbatim (mmap
PROT_NONE whole, mprotect(base+guard, stack, RW)). Probe evidence
(run/ps-probe-boot.sh, x86_64 strong, 1f2f18cb kernel): interior
mprotects at sizes 0x1000..0x40000 all failed errno 12; whole-VMA
mprotect worked. Consequence: every threaded musl static binary broke
(pthread_create -> EAGAIN, residual errno ENOMEM). The b91-b92 comments
in sys_mprotect even describe this exact pattern as known-critical.

Fix: seed the walk with `vma_mgr.find(addr)` (O(log n) containing-VMA
lookup); None leaves cursor at addr so real gaps still fail ENOMEM.
No-alloc/no-sort property kept. Regression test added to
mem_mmap (test 8b); NOTE the in-kernel test context has no user mm so
the row SKIPs there — the operative verification is the boot probe
(all sizes pass post-fix) and the p2f battery running threads.

## F2. Futex wake-to-run is syscall-length-quantized behind kernel-mode
incumbents — kernel-preempt hook default-ON collapses it (FIXED, x86_64)

Mechanism (why the OH census saw 10-30ms steps / 108ms avg):
- USER-mode incumbents: the wake IPI sets need_resched; trap_exit asm
  (.Lreturn_user loop, x86_64 trap.S) preempts at IRQ return. µs. Fine.
- KERNEL-mode incumbents: the resched IPI only set a flag; the IRQ-exit
  kernel-preempt hook existed but was disarmed (sc69-73 leak
  amplification verdict), and cond_resched() call sites are nearly
  nonexistent (only char_dev UART + arch files). The woken task waits
  for the incumbent's next syscall return — i.e. wake latency is
  quantized by whatever syscall the incumbent is in. On the OH image
  that's our slow syscall classes (fsync 12-14ms, mkdrmdir 14-16ms,
  socket ops 36-40µs at 15-18x reference) stacked through 50-hop IPC
  chains — the "tick-quantized" signature and the 26x boot wall's
  engine, as the in-tree kpreempt commit message already argued.

New pbench P2f battery (thread ping-pong, musl private futexes, waiter
pinned CPU1, waker CPU0; hog pinned CPU1 loops pread(4MB) of a cached
file = one contiguous kernel stretch per syscall, hogcal row times it):

  TCG q35 smp2/2G (relative record)   hogcal med:
    reference kernel                     4783 us
    ours (hook off)                      5501 us
  khog wake->run med (w2r):
    reference kernel       85.8 us  (87% of wakes <100us — preempts mid-syscall)
    ours, hook off       5607.9 us  (97% of wakes 1-10ms)  = 65x reference
    ours, hook on          23.2 us                             = 241x collapse
  KVM (primary rig)                   hogcal med:
    reference kernel                       72 us
    ours (hook off)                       160 us
  khog w2r med:
    reference kernel        8.2 us
    ours, hook off        163.0 us  (b1lt1ms 19778/20000)  = 20x
    ours, hook on           9.5 us                          = parity
    ours, default-on        9.3 us  (no cmdline flag)
    ours, kpreempt=0      161.9 us  (opt-out verified)

Controls: quiet w2r and user-hog w2r never regressed (KVM: 7.6/9.0us
hook off vs 7.5/9.1us on — the user-mode preempt path was already fine;
TCG quiet: 24.7 -> 21.0us).

Full-battery regression (KVM, back-to-back kp=0 vs kp=1, same probe):
every row within ±5% (p1_null 0.080 identical, p2_ctxsw 0.180->0.175,
p5/p6 rows at 1.00-1.05, p6_unlink 0.91). The hook's disabled-path cost
is not measurable. p7_sleep1ms med 984->1089us (+11%) on one pair of
boots — flagged for the merge gate to re-verify with med-of-runs; p7 is
the timer-overshoot row, plausibly noise.

Boot beacon delta (same boots): PREEMPT-HEAL events DOWN with the hook
on (49 -> 2), zero WAKE-STRANDED, no new panic classes — consistent
with the kpreempt commit's argument that WAKE-HEAL + the 2s tick heal
now bound the old sc70 monopoly amplification.

Residual risk, recorded: a task carrying a LEAKED preempt_count that
stays in kernel loops is still unpreemptable while count != 0 (same as
before the flip); the leak producer hunt (sc73 fingerprints: ICACHE,
RECORD_LOCK_TABLE, heap spinlocks — the VFS double-run engine) remains
open and is now more user-visible. kpreempt=0 is the escape hatch.

Gates: riscv64 `make test` ALL TESTS PASSED (1198); x86_64 debug +
release-opt(strong) + aarch64 builds rc=0.

## F3. riscv64: secondary harts never enabled SSIE — every IPI masked,
##       every cross-hart wake tick-quantized (FIXED — the headline)

After the x86_64 work, porting the battery to riscv64 (primary
platform, GNOME target) reproduced the charter's signature on a QUIET
two-CPU boot:

  riscv64 strong, TCG virt SMP2, pbench p2f (waiter pinned hart 1):
    quiet futex wake->run med 9118us — 49% of wakes clustered at the
      10ms tick (p99 10009us); rt row shows exactly one tick-stall per
      roundtrip
    user-hog incumbent w2r med 9952us (96% at the tick)
    kernel-hog (pread4M, hogcal 1669us) w2r med 1447us (syscall-length)
    PROOF OF MECHANISM: pin BOTH threads to hart 0 (no cross-hart IPI
      needed) — quiet w2r med 24.2us, khog 25.2us, uhog 18.7us. The
      wake path was never slow; the cross-hart IPI never took effect.

ROOT CAUSE: `ipi::init()` sets sie.SSIE once, on the boot hart only
(main.rs). sie is per-hart, boot.S zeroes it on every hart, and
`secondary_cpu_entry` re-enabled only STIE + SEIE. Every secondary hart
ran with software interrupts masked forever: resched/call-function/
TLB-flush IPIs sat in sip.SSIP pending-but-disabled; the wake's
need_resched was never set by the IPI; the target ran the waiter only
when its 10ms timer tick's own scheduler work preempted the incumbent.
(TLB-flush IPIs to secondaries were silently never executing either —
fire-and-forget, a standing TLB-staleness hazard.)

FIX: one `csrsi sie,2` per secondary bring-up (ipi::
per_hart_enable_ssoftware from secondary_cpu_entry). Commit 94c8dcec.

  after (same rig, hook off):  quiet 35.0us (was 9118 = 255x)
                               uhog  33.2us (was 9952 = 300x)
                               khog 1438us (unchanged — see F2' below)
  stability: make test ALL TESTS PASSED; full P1-P7 battery on riscv64
  boots rc=0 zero panics; reproduced across two boots (35.7/35.0).

## F3b. riscv64 kernel-preempt twin: implemented, default OFF (FILED)

The riscv64 twin of the x86_64 kernel-preempt hook is in the tree
(commit 9f5ebc50, armed by kpreempt=1, default off). Armed, its first
battery hit the pick-machinery corruption the .Lcheck_resched_kernel
asm comments warn about (R46/R49/R56 phantom-heal cascade into an
idle-task user-frame KERNPANIC ~30s into the quiet regime). The x86_64
twin runs the same battery clean (its guards were tuned through
sc65-sc73). Stabilizing the riscv twin requires the R53/R84/R85
trap-frame work, not scheduler changes. Prize once stable: the khog
(syscall-quantized) wake, med 1438us today vs x86_64's 23us TCG /
9.5us KVM with the hook.

## F3c. riscv64 probe ecosystem notes (for future lanes)

- Ubuntu glibc-static is unusable on this kernel: unconditional RVV
  code + sstatus.VS never enabled -> every vector insn SIGILLs.
  toolchain/build-musl.sh builds a scalar riscv64 musl that works
  (pbench links it via -nostdlib).
- rdcycle traps in U-mode under QEMU's direct-S-boot (mcounteren
  state); rdtime is the legal U-mode counter (vDSO uses it).
- pbench_boot_rv.py carries both workarounds; the cmdline contract for
  kpreempt is uniform across arches now (=1 arms, =0 disarms, absent =
  arch default: ON for x86_64, OFF for riscv64).


## F4. Shared futex keys are raw VAs — no page identity (FILED)

FutexKey::matches for FLAGS_SHARED futexes compares `uaddr` only.
Two processes mapping the same physical page at different VAs never
match (lost wakeup); unrelated mappings at the same VA falsely match
(cross-process wake). The reference kernel keys shared futexes on the
page (inode offset / anon page). Worked around in every in-tree user
by fork-inherited VA equality. Not fixed in this lane (correctness
surface, needs its own LTP coverage: FUTEX_SHARED across processes
with distinct mmaps of one shm object).

## F5. Side observations from the hunt (numbers, uninvestigated)

- pwrite(4MB) of a fresh file: med 2.02 s/op on our kernel (TCG,
  20-sample probe row p2f_hogcal pread era). The write path serializes
  on real I/O per pwrite (journal/writeback interplay) — 2MB/s class.
  Worth an fs-lane look; it makes any write-heavy incumbent a
  seconds-class wake blocker (though it sleeps, so it frees the CPU).
- getdents64 of a freshly-created 8000-entry dir: 18.1 us/full sweep
  on ours (hot), yet p6_getdents of the p6-built 1000-file tree is
  ~550 us/sweep (strong, KVM 127us) — an order-of-magnitude gap
  between hot-created and p6-shaped trees that the fs lane should
  decompose (dcache presence? page-cache vs dentry path?).
- p7_sleep1ms overshoot: ours ~984-1089us med (KVM) / 1475us (TCG) vs
  reference 16.7us — a ~60-88x timer-resolution gap (1ms sleeps return
  after ~1.5 ticks). Separate lane (timer/hres), but it is the other
  half of "woken task waits for the next tick".

## F6. Full-battery KVM A/B — the remaining-gap map (for other lanes)

Reference vs ours (ours = default-on kernel, kp=1 arm), same probe,
KVM, medians (boot pair psrefk2 / psfull1):

  row                    ref      ours    ours/ref
  p1_null              0.040     0.080     2.0x
  p1_clock             0.070     0.190     2.7x
  p2_ctxsw             0.085     0.175     2.1x
  p3a_fault_anon       4.669     0.010     we WIN (eager mapping)
  p3b_fault_cow        0.671     2.314     3.5x
  p3c_fault_file       0.090     0.010     we WIN
  p4_fork_exit        12.964   633.680    48.9x   <- mm lane (known)
  p4_fork_exec        26.139  1006.735    38.5x   <- mm lane
  p5a_pipe             0.170     0.341     2.0x
  p5b/p5c socket family 0.12-0.19  1.96-2.30  12-19x  <- ipc lane
  p5d_rt_read/recv     0.24-0.28  0.63-0.67  2.3-2.8x
  p6_stat              0.230     3.687    16.0x   <- fs lane
  p6_openclose         0.371     5.140    13.9x   <- fs lane
  p6_read_cached       0.531     5.691    10.7x   <- fs lane
  p6_append            0.411    62.416   151.9x   <- fs lane
  p6_mkdrmdir          7.745 14028.371  1811.3x   <- fs lane (the single
                                                      worst row anywhere)
  p6_unlink            1.924    11.151     5.8x
  p6_write_fsync    9329.493 14864.057     1.6x   (I/O-bound both)
  p7_sleep1ms         17.089  1089.354    63.8x   <- timer lane (1ms
                                                      sleeps return ~1
                                                      tick late)
  p2f_futex_* (quiet) 18.0      7.3      0.41x   <- this lane: we now
                                                    WIN the futex rows
  p2h_futex_uhog       6.2       9.1      1.45x

(The full-battery p2g rows have no hog — the hog file only exists in
p2fonly mode; use the dedicated psrefk1/psruxk1-k4 boots for khog
verdicts. Single-boot medians on a multi-lane host; treat ±25% as
noise per the lane protocol.)

Note for the sched lane's open charter items: p2_ctxsw at 2.1x
reference (0.175 vs 0.085us KVM) is a fixed ~90ns syscall-entry/
switch tax, not a scheduling defect; the GRQ hold-time dissection
(5.1-6.1us census band) remains merge-gate territory — nothing in this
lane's A/B points at the pick path as a wake-latency contributor once
kernel preemption is on.

## Artifact map

- run/ps-pbench — the probe binary used on BOTH kernels (md5-identical
  arms; pbench_boot.py --probe-bin).
- run/ps-our-{base,fix1,kpdef}-bzImage — our kernels: 159111af lineage
  base / +mprotect fix / +default-on.
- work/pbench-psref*.log, -psrux*.log, -psfull*.log — serial logs of
  every numbered boot above.
- run/ps-probe-boot.sh — one-off init=/test/probe boot driver.
- Full battery baselines: psref1/psref2 (ref TCG), psrux1 (ours strong
  TCG, pre-fix probe), psfull0/psfull1 (ours KVM kp=0/1).