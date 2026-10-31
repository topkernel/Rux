eview — syscall entry/dispatch + openat (perf-syscall lane, 2026-10-31)

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

---

# perf-ipc lane findings — 2026-10-31

Worktree: /home/william/rux-agents/perf-ipc (branch perf-ipc, off
origin/main 1e2bef44). Rig: pbench_boot.py battery, q35, TCG
thread=multi, smp2/2G, -cpu max — the SAME battery booted on the
reference bzImage (/home/william/workspace/oh-robot/out/x86_64_virt/
packages/phone/images/bzImage) for the true A/B column.

## The headline numbers (medians, us)

strong profile (release-opt LTO), baseline = origin/main same rig:

| row            | base  | fixed | gain   | reference | vs ref   |
|----------------|-------|-------|--------|-----------|----------|
| p5b_dgram_send | 39.80 | 1.94  | 20.5x  | 2.39      | AHEAD    |
| p5b_recv_peek  | 39.78 | 3.82  | 10.4x  | 2.66      | 1.44x    |
| p5c_stream     | 35.50 | 2.01  | 17.6x  | 2.50      | AHEAD    |
| p5c2_send      | 35.45 | 1.96  | 18.1x  | 1.91      | 1.03x    |
| p5c3_wrecv     | 2.50  | 1.94  | 1.3x   | 1.91      | 1.02x    |
| p5f_dwait      | 35.35 | 1.94  | 18.2x  | 1.91      | 1.02x    |
| p5e            | 2.35  | 1.86  | 1.3x   | 1.90      | AHEAD    |
| p5d_rt_read    | 4.17  | 3.84  | 1.09x  | 4.31      | AHEAD    |
| p5d_rt_recv    | 4.68  | 4.10  | 1.14x  | 3.78      | 1.08x    |
| p5a_pipe       | 2.08  | 2.28  | noise  | 2.88      | AHEAD    |
| p2_ctxsw       | 1.13  | 1.23  | noise  | 1.87      | AHEAD    |

debug profile (same rig, one boot each end): p5b 321.8 -> 23.8 (13.5x),
p5c 276.7 -> 27.7 (10.0x), p5f 276.4 -> 25.5 (10.8x), full battery
DONE rc=0 both profiles, zero panics.

The charter target ("10x-class on the lagging metrics") is met on the
p5b/p5c/p5f family in BOTH profiles; the reference kernel column is now
matched or beaten on the unix-socket family in the strong profile.

## Root cause chain (evidence-backed)

1. WHY the socket rows were 35-40 us: the pbench p2 section pins pid 1
   to cpu0 and never restores the mask, so every later fork inherits
   cpus_allowed={0}; the p5b/p5c sender+receiver pairs run pinned to
   ONE cpu while cpu1 idles (ipcdisc3 proved no other precursor — the
   same shapes unpinned were fast on both kernels).

2. On that same-cpu pair, every send's wake_up_process resched'd the
   wakee's previous cpu (= the sender's cpu, busy) UNCONDITIONALLY:
   the sender preempted at trap-exit per record, the receiver drained
   one 74-byte record and re-slept — a 275 us/record dance (ipcdisc2
   sched trace: wake 19us + preempt-switch 56us + receiver cycle 131us
   + sender cycle 86us). The reference kernel batches the receiver and
   sustains 2.4 us/record on the same pinned shape.

3. Why a granularity-gated preempt could not fix it: the vruntime
   credit is structurally skewed in a ping-pong pair. Per-switch charge
   probe (CHG): the SENDER takes +10 ms vruntime jumps at requeue
   clamps (enqueue floor = min_vruntime − SCHED_LATENCY_NS) while the
   receiver gets sleeper-credit drops; the receiver sits a few ms of
   vruntime "behind" the sender forever, so check_cfs_preempt's 700 us
   gate passed on EVERY wake (WPRE probe: delta steady at 2-6 ms).

## Fixes (commits on perf-ipc)

- 977adb51 sched: (a) wakes dispatch to an IDLE cpu within the wakee's
  affinity when one exists (claim_idle_cpu_for; the GRQ keeps the IPI
  advisory — blocker handoffs still happen in place), (b) the idle loop
  honors NEED_RESCHED before halting (closes the IPI-before-halt
  stranded-task window the idle dispatch newly exercised), (c) with no
  idle cpu, the busy-cpu preempt is gated on the MEASURED wait: a wakee
  that last ran within one SCHED_LATENCY_NS window is a batch peer and
  is left to the slice/blocker machinery; a wakee that waited a full
  window keeps the immediate preempt. RT/DL wake paths unchanged.
- e1e7d8f9 unix: single-copy send (owned-Vec segment enqueue), write(2)
  fast path, no-zero-fill staging for sendto/recvfrom/writev-gather.
- 236dec39 + revert eab076ec pipe: the O_ASYNC registry lock-free
  empty check was measured, then WITHDRAWN — see the layout note.
- 3f24ac59 test(probes): ipcdisc/ipcdisc2/ipcdisc3 in probes.tar.gz.

## p1_null layout variance (documented, not chased)

p1_null on this rig swings 0.47-0.65 us across semantically adjacent
strong builds: origin/main 0.481/0.481; sched-only 0.471; sched+unix
0.481; +pipe 0.631/0.641; pipe-reverted (SOURCE IDENTICAL to the
sched+unix tree) 0.651 with a different bzImage md5 — incremental LTO
reuses different object sets and re-rolls code layout, and under
TCG/MTTCG that is worth ±0.17 us on the null syscall. The socket-family
results are layout-stable (2.0 us across every variant). p1_clock moved
0.671 -> 0.752 (+12%, inside the gate's +-20% band). Verdict columns
for the merge gate should re-anchor p1_null on its own builds; treat
+-0.2 us on p1_null as build noise on this rig unless it survives a
same-source rebuild.

## What is still behind the reference (filed, not fixed here)

- p1_null 0.63 vs 0.24: the syscall entry itself (arch lane).
- p5b_recv_peek 3.82 vs 2.66: the hilogd peek+consume shape = two full
  syscalls per record; each syscall carries the p1 tax.
- The vruntime accounting skew (sender +10ms requeue clamps, sleeper
  credit drops) is the deeper CFS defect that made the wakeup gate
  useless — the sleep-gate is a robust workaround, but a fair.rs fix
  (min_vruntime semantics / place-on-wake) would restore true
  vruntime-gated wakeup preemption. Evidence: CHG probe logs in the
  session record; WPRE deltas 2-6 ms steady.
- binder SLOW-TXN family: not re-measured this session (the wake-path
  fix removes the per-record preempt class that fed the 18/boot
  residue's queue-latency component; the image-side 15/30/89 s family
  is userspace-owned). Suggest a slowtxn re-bank on the next OH boot.

## Gates

make test (riscv64, in-guest suite): 1204/1204 green after the fixes.
Builds: x86_64, riscv64, aarch64 rc=0. riscv64 smoke boots (unit-suite
init and shell init) green, zero real panics. Perf-gate census/pbench
re-bank belongs to the coordinator's merge gate (sched/ is a guarded
domain: wake→pick deltas and the census bands need a re-anchor pass on
this branch).

---

# perf-mm lane findings — 2026-10-31

## FINAL A/B board (same machine, same battery, TCG q35/-cpu max/SMP2/2G)

`test/pbench_boot.py` on the reference bzImage (refsmoke run), our
strong-profile kernel at main 1e2bef44 (base run), and our strong
kernel with this lane's two fixes (after run, commits d2fac01a +
6cd3c9b8; provenance run/pmm-fix-built-from.txt; lines in
run/pbench-pmm-{refsmoke,base,after}-lines.txt):

| bench | base med | after med | ref med | base/ref | after/ref |
|---|---|---|---|---|---|
| p3c fault file (AVG, us) | 16.871 | **1.039** | 0.580 | 29.1x | **1.8x** |
| p4_fork_exit (med) | 507.0 | **447.3** | 346.9 | 1.46x | **1.29x** |
| p4_fork_exec (med) | 1621.1 | **1543.3** | 841.7 | 1.93x | 1.83x |
| p6_write_fsync (med) | 20592 | **14131** | 16583 | 1.24x | **0.85x (ahead)** |
| p6_append (med) | 89.3 | 93.3 | 9.4 | 9.5x | 9.9x |

p1/p2/p3a/p3b/p5a unchanged (ahead or parity; full lines in the run/
files). The p6_append +4.5% med is the populate's one page-cache
insert per 4 KB append write — the explicit trade for the 16x p3c win;
the row's remaining 10x is the ext4 append/journal path (fs lane).

Probe rows (test/pmmprobe.c, PMMP lines) across the lane's boots:

| row | base | after (clean build) |
|---|---|---|
| mm1 cold file fault avg (us/page) | 14.107 | 1.002 |
| mm2 warm file fault avg | 0.942 | 0.836 |
| mm5 fork syscall med | 172-209 | 176.9 (ref: 217.1) |
| mm6 fork+exit+wait med | 500-553 | 463.9 (ref: 346.9) |
| mm7 vfork round trip med | 202-233 | 158.2 |
| mm8 fork+exec+wait med | 1596-1677 | 1532.9 (ref pbench row: 841.7) |

---


Method: true A/B per charter — `test/pbench_boot.py` boots the SAME
pbench P1-P7 battery on the reference bzImage
(/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage,
6.6.101) and on our strong-profile bzImage, same machine (q35, TCG
thread=multi, -cpu max, SMP2, 2G), same probe binary, back-to-back
windows. Host load 4-8 during these runs (not quiet-protocol grade;
gaps smaller than ~1.3x are not verdict-grade at that load — the big
rows below are far outside noise).

## The headline correction

The charter's numbers (p3a p99 581us bimodal; fork+exit 9ms; fork+exec
13ms) predate two merged fixes (RCU QS on user-return c312944d, pcp
pagesets 45ccb66d) and were debug-profile numbers. On current main,
strong profile, the true A/B board is:

| bench | Rux med (us) | ref med (us) | gap |
|---|---|---|---|
| p1_null | 0.601 | 0.270 | 2.2x |
| p1_clock | 0.761 | 0.861 | ahead |
| p2_ctxsw | 1.347 | 2.044 | ahead |
| p3a anon fault | 0.080 (avg 2.12) | 5.711 (avg 8.07) | ahead |
| p3b COW fault | 2.954 | 7.915 | ahead |
| p3c file fault | 0.070 (avg 16.87) | 0.080 (avg 0.58) | avg 29x |
| p4_fork_exit | 507.0 | 346.9 | 1.46x |
| p4_fork_exec | 1621.1 | 841.7 | 1.93x |
| p7 sleep1ms | 1192.2 | 16.6 | 72x (timer domain, not mm) |

Full lines: run/pbench-pmm-base-lines.txt (Rux main 1e2bef44 strong),
run/pbench-pmm-refsmoke-lines.txt (reference, same machine).

p3a bimodality explained: med 0.08us = the 15/16 stores that hit pages
the anon fault-around already mapped (16-page forward window, VMA >= 1MB
write faults); the tail (~52us p99 on strong) is the fault-around batch
itself (16 allocs + 64KB zeroing + 16 maps). Per-page effective cost
(avg 2.12 vs ref 8.07) is AHEAD of the reference — the reference faults
every page individually (no anon fault-around; its THP is bimodal under
this config). No action needed on p3a.

## F1 (FIXED): write path never populated the page cache — p3c cold faults 14.3x slower than warm

Decomposition probe (test/pmmprobe.c, PMMP rows; run/pmm-decomp* logs):

| shape | Rux avg us/page | ref avg us/page |
|---|---|---|
| MM1 cold file fault (write+fsync, mmap, touch) | 14.107 | 0.692 |
| MM2 warm (pread first, mmap, touch) | 0.942 | 0.635 |

Our ext4 write path writes through the buffer cache and only UPDATES
existing page-cache frames (writeback_update_range); it never inserts.
The reference kernel's buffered write populates ITS page cache, so its
"cold" faults are cache hits. Every written-then-mapped file (the
dynamic linker's every lib) paid the populate path per fault window:
filemap_pin_cache_page miss -> ext4_fault_populate_cache ->
EXT4_BIG_LOCK + fill_page_cache_batch + RA state.

Fix: `PageCache::writeback_populate_range` (fs/page_cache.rs) — after a
successful write, update existing frames (unchanged semantics) AND
insert the fully-written page run as clean cache frames via
insert_batch (one wlock + buddy allocations for the whole run; partial
head/tail pages stay to the fault-time populate). Write-core call site
switched (fs/ext4/file.rs).

Numbers (same boot class, pmm-pop vs pmm-base/decomp runs):
- MM1 cold file fault avg: 14.107 -> 0.985 us/page (14.3x), now == warm
  (0.977) and == the reference cold shape (0.692, 1.4x residual).
- Warm path unchanged (0.94 -> 0.91-0.98).

## F2: fork+exit gap decomposition — fork at parity; exit work = 190us of the 347us; futex scan was 64us of it (FIXED)

Decomposition (mm5/mm6/mm7/mm8 rows, 400 iterations each):

| shape | Rux med | ref med |
|---|---|---|
| MM5 fork syscall alone (child parks 200ms) | 172-209 | 217 |
| MM6 fork+exit+wait round trip | 500-553 | 346.9 |
| MM7 vfork round trip (no mm copy, no mm teardown) | 202-233 | — |
| MM8 fork+exec+wait | 1596-1677 | 841.7 (pbench row) |

Kernel-side attribution (EXITC TSC buckets, 10 MHz clock, steady-state
windows during the mm6 loop; run/pmm-diag2-serial.log):

| exit-path piece | us/exit |
|---|---|
| MmStruct::drop total (mm_drop bracket) | 80 |
|   — VMA tree release under vma_write | 14 |
|   — synchronize_rcu | 1 (RCU_GEN frozen: cheap, confirms the ledger) |
|   — free_user_page_tables walk (in this context) | ~65 |
| exit "rest" block (mm-drop -> ZOMBIE) | 92 |
|   — futex_cleanup 512-bucket scan | 64 |
|   — fdtable drop | 20 |
|   — reparent | 2 |
| notify (deferred, in idle context) | 14 |
| reap (release_task, incl. its own synchronize_rcu) | 17 |

Sum of bracketed work ~ 190us; the roundtrip is 523us, so ~330us is
scheduling/wake latency (child's final context switch to idle, deferred
notify, parent wake + reschedule) — the sched lane's domain, not mm.

### F2a (FIXED): futex_cleanup scanned all 512 buckets on EVERY exit

`futex_cleanup` took `HASH_HEADS[bucket].lock_irqsave()` for all 512
buckets and walked every chain on every task exit — 64us under TCG for
tasks that never parked on a futex (the overwhelmingly common case; the
sleeper-side return unlinks the waiter itself).

Fix: `Task::futex_wait_enqueued` residency flag — set at the single
enqueue site (futex_wait_timeout's chain link, under the bucket lock),
cleared by every unlink/free path (remove_waiter both branches, the
woken-sleeper slot free, and the cleanup scan itself). futex_cleanup
returns after one load when the flag is clear. Numbers: diag3 boot
(below).

### F2b (root cause of the residual scans): the flag read HEAP GARBAGE on fork-allocated tasks

The diag4 counters (ffp/fscan) showed fork+exec children still
full-scanning 64/64 while vfork children skipped. The flag had NO
setter for those tasks: `alloc_task_slot` builds the Task via
`new_task_at` — a FIELD-BY-FIELD ptr::write init list over a recycled
heap page (the sc130 comment in task.rs documents this exact trap for
the last_fails rings) — and my at-construction `AtomicBool::new(false)`
only runs in `Task::new` (first construction). Fork tasks read the
stale page bit; whether a given task scanned depended on its page's
history (mm7's mixed ffp=52/fscan=12 vs mm8's 64/64).

Fix: init `futex_wait_enqueued` in `new_task_at`'s init list (commit
6cd3c9b8). Lesson for every future Task field: add it to BOTH
constructors.

## F3: exec adds ~1.1ms on our side (mm8: fork+exec+wait 1627 vs mm6 500)

Structural audit notes (exit/notify chain, for the sched lane):
- Parent notification is DEFERRED to after the dying task's final
  context switch (defer_exit_notify -> processed in __schedule's tail
  from the incoming/idle context, with parent-CPU steering). The
  ~330us scheduling-latency residual of the round trip lives in this
  chain (switch-to-idle, notify, parent reschedule) — fork syscall and
  all bracketed kernel work sum to ~370us of the 523us roundtrip.
- copy_page_table_cow EXONERATED (fork syscall at parity; the walk is
  chunked with per-leaf-table PTE sections; pte_install_log and the
  FUT/PT_LEDGER forensics are switch-gated).
- The teardown walk (~65us for a ~25-page mm) is a mix of per-PTE
  descriptor work (rmap_alt_remove, page_remove_rmap, put_page_site,
  free_pages via pcp) and per-table ledger checks — no single dominant
  line item; diffuse. Left as-is this lane.

## F4: exec cost (secondary)

Reference fork+exec 841.7 vs fork+exit 346.9 -> exec adds ~495us.
Ours: 1596-1677 vs 500-553 -> exec adds ~1100us. The exec path
(do_execve_elf) maps segments eagerly, then walks per page to tighten
permissions (4-level walk per page), reads the ELF + phdrs, copies
argv/envp, and the exec'd image re-faults its pages on first run.
~2.2x the reference's exec cost — fs+mm mix; not attacked this lane
(the execcost DFX switch exists for the follow-up).

## Code-level audit (charter items not driven by a measurable A/B row)

- **TLB shootdown**: x86_64 flush is fire-and-forget IPI broadcast +
  local CR3 reload (fork end, munmap end). No synchronous wait (the
  reference waits since its 4.x rework) — our shape is the FAST one;
  note the narrow freed-frame race window it accepts is documented at
  the call sites. COW-fault resolution uses per-page invlpg + IPI
  (mm_ops 2494/2540). No change.
- **munmap**: per-page TWO root-level walks (PageTableWalker::walk +
  clear_pte) + rmap/put/free, ONE full flush + one IPI per batch —
  sound shape; the double walk per page is a known ~2x zap tax vs the
  reference's table-driven descent, but munmap is not in the battery
  and the fork/exec rows that matter are at/near parity. Filed, not
  changed.
- **copy_page_table_cow**: per-PTE 2 atomic RMWs (get_page +
  inc_mapcount) + parent-PTE rewrite, leaf-table chunks, full flush at
  end. MEASURED at parity via mm5 (fork syscall 172-209 vs ref 217).
- **VMA tree**: BTreeMap keyed by start; O(log n) find/add — same
  class as the reference. mprotect's collect+sort under vma_write
  remains the known wart (sibling lane F3, pending there).
- **Demand-fill vs fault-around**: anon write faults on VMAs >= 1MB
  get a 16-page forward window (x86_64 AND aarch64 — the aarch64 port
  exists, header-documented, `faaround=0` opt-out; the charter's
  "aarch64 deferred" note is resolved). File read/exec faults get the
  16-page cache-direct window on both arches. Effective per-page anon
  cost measured AHEAD of the reference (2.1 vs 8.1 us avg).
- **Allocator family (small_alloc/buddy/slab)**: not reachable from
  the x86_64 A/B battery; the standing perf-gate census (REST share,
  magazine discipline, small_alloc class-lock sections) owns those
  numbers on riscv64 — no new evidence here; no change.
- **deferred_free deadline-stall and buddy merge bugs**: already fixed
  on this branch's base (a90b76fc, bab50e52 — see
  run/review-findings-mm.md).

## Non-mm rows noted for the owning lanes (not acted on here)

- p1_null 2.2x (syscall entry path).
- p7 sleep1ms overshoot 72x (16.6us ref vs 1192us ours — timer/tick
  domain).
- p6 rows (stat/openclose/read_cached/append) 3-9x behind — fs domain.

## Instrument

- test/pmmprobe.c — the PMMP decomposition probe (cold/warm file fault,
  anon fault, fork syscall vs round trip vs vfork vs fork+exec). Boot
  with: pbench_boot.py --probe-bin work/pmmprobe --bzimage <k>.