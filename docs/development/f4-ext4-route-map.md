# F4 ext4 metadata — cost anatomy and fix-route map (leg 18, pa28/pa29)

Status: measurement-only diagnosis for the fs lane. No kernel fix in this
leg. Ranked routes below carry expected-effect estimates from the pa28/pa29
probe boots (KVM, q35, -smp 2 -m 4G, quiet OH boot, fresh reflink userdata
per boot, `syscost=1 fsw=1`).

## 0. Measurement setup and the probe

`dfx=fsw` (cmdline `fsw=1`, kernel/src/dfx/fsw.rs) counts the ext4/bio
layer's volumes: block-cache breads / device reads / device writes /
sync-dirty attempts / evictions / whole-cache drains, ext4 inode
read-writes, directory lookups and blocks scanned, list_dir calls, inode
and block allocator runs, jbd2 events, EXT4_BIG_LOCK acquisitions, and
VFS walk hit/miss. Two compact lines per minute, offset 30 s from the
syscost dump (pa28 lesson: dumping right behind syscost's ~70 lines loses
the console-emit race — 11 of 12 dumps vanished; the offset fixed it).

- pa28-b: 780 s boot, fsw counters armed but dumps console-raced; one
  cumulative line survived at t=420 (values consistent with pa29).
- pa29: 480 s boot, full fsw + syscost data. Cumulative t=450:

| counter | value | meaning |
|---|---|---|
| bread | 2,813,931 | buffer-cache lookups |
| dev_rd | 25,164 | actual device reads (**99.1 % hit rate**) |
| dev_wr | 12,107 | actual synchronous 4K writes |
| sync_dirty | 102,570 | sync_dirty_buffer calls (89.7 K no-op: already clean) |
| evict | 24,895 (5 dirty) | LRU evictions — the 4 MB cache thrashes, victims almost always clean |
| sync_all | 761 | whole-cache sync_buffers() drains |
| read_inode | 158,148 | inode-table reads (all cache hits after warmup) |
| write_inode | 4,269 | inode-table writes |
| dir lookup | 247,211 scans over 1,731,942 blocks (avg 7 blk/scan) | linear, no htree |
| list_dir | 27,610 | full-directory re-materializations (getdents) |
| new_inode / alloc_run / free_run | 590 / 1,383 / 213 | allocator round trips |
| jbd2 (start/stop/lazy/commit/revoke) | all 0 | **no journal anywhere on this workload** |
| big lock | 13,003 takes, 785 contended (6 %) | guards only file read/write cores, not open/stat |
| walk | 540,506 walks, 3,733,547 hits, 378,392 misses | **dcache hit rate 90.5 %** |

Phase structure (from 60 s deltas): ALL 12 K synchronous writes, all 102 K
sync-dirty attempts, all 761 whole-cache drains, and ~21 K of the 25 K
device reads land in **phase A (t=0-90 s** — provisioning: e2fsck,
resize2fs, init file creation; mkdir count 859, pwrite 1,682). Phase B
(t=90-150 s, .so/SA loading) adds 2.0 M breads (99 %+ hits — pure CPU)
and the bulk of dir lookups. Phase C (t>150 s): ext4 is IDLE (bread delta
~60/min). The deep-boot wall-clock is owned by F3 wake-loss and the
binder cost classes, not by ext4.

Image facts (dumpe2fs, pa-series oh-img): userdata-tok.img (the only rw
ext4) has **no journal and no metadata_csum** — every write takes the
NO-JOURNAL path; system/vendor/sys_prod/chip_prod are metadata_csum
(RO mounts; sys_prod rw→RO downgraded, one print per boot).

## 1. Corrected weight of F4 (honest ledger)

The "~600 CPU-s/boot" in the ranking came from pa17's stall-inflated
averages (open avg 1,893 us carried 100 s maxes — that tail is F3, not
F4). Clean numbers:

- pa29 t=420 syscall wall, fs-class: open 41.6 s + mkdir 37.9 + pread
  33.6 + pwrite 31.8 + newfstatat 30.7 + getdents 11.4 + smaller ≈
  **200 s wall in 420 s**, essentially all before t=150.
- CPU share of that wall ≈ 60-70 % (the rest is virtio wait inside
  synchronous writes) → **≈ 120-140 CPU-s/boot**, concentrated in the
  first 90 s. After t=150 s F4 contributes ~0.
- Per-op factors vs Linux stay 100-300x where measured before (open 156
  us vs 1-2 us, getdents 407 us vs 2-4 us, mkdir 44 ms vs ~50 us).
  F4 is a FIRST-PHASE cost and a per-op-latency outlier, not a
  whole-boot CPU family.

## 2. Cost anatomy — what the money buys, per phase

### Phase A (0-90 s): synchronous write-through, no journal

Every metadata update syncs inline (the journal path's lazy-commit
machinery is journal-gated and never engages):

- `write_block_from_vec` (namei.rs) marks dirty then calls
  `bio::sync_dirty_buffer` — one synchronous virtio round trip per
  bitmap / group-descriptor / directory block write.
- `update_superblock_free_inodes/blocks` (namei.rs / allocator.rs):
  bread + RMW + **synchronous SUPERBLOCK write on every inode alloc/free
  and every block-run alloc/free** — 1,383 alloc runs x 3 sync writes
  (bitmap + group desc + superblock) ≈ 4.1 K of the 12 K device writes.
- `ext4_mkdir_no_journal` ends with **`bio::sync_buffers()` — a
  whole-cache drain per mkdir** (761 drains ≈ 859 mkdirs + fsync-class
  callers). Each drain walks all 64 buckets.
- `ext4_sync_file` (fsync): breads EVERY data block of the file (pulling
  them through the 4 MB buffer cache, evicting metadata), then ends with
  another whole-cache sync_buffers().

102,570 sync-dirty calls produced 12,107 real writes — 88 % were
redundant re-syncs of already-clean blocks (shared inode-table /
bitmap / superblock blocks re-synced once per operation touching them).

Phase-A read side: the fixed **1024-entry (4 MB) buffer cache**
thrashes — 21 K misses / 21 K evictions in 90 s, each miss a device
read Linux would have served from an unbounded page cache.

### Phase B (90-150 s): pure-CPU metadata lookups

2.0 M bread calls at >99 % hit rate: the cost is the bread path itself
(bucket irqsave spinlock + LRU splice + refcount) — the same irqsave-floor
family as F1's measured 136-168 ns/op class, times 46 K calls/s.
On top:

- 247 K ext4 directory lookups, each a **linear scan of every directory
  block** (1.73 M block parses; avg 7 blocks, large dirs pull it up) —
  no htree (dir_index) support. 90.5 % of VFS walk components are dcache
  hits; the 370 K misses pay the scan plus `read_inode` (158 K calls).
- 27.6 K getdents64 calls each **re-materialize the whole directory**
  (file_getdents64 → ext4 list_dir → read every block, clone every
  entry, then index by f_pos).

### Phase C (150 s+): idle

No fs activity to fix. Wall-clock here is F3 + binder (separate lane).

## 3. Ranked fix-route map (for the fs lane)

Ranked by measured weight x expected effect / risk. Routes 1+2+7 are one
theme: **stop paying synchronous I/O per metadata op on the no-journal
path**; Linux's no-journal ext4 is fully write-back with barriers only at
durability points.

| # | route | what Linux does that Rux doesn't | measured handle | expected effect | risk |
|---|---|---|---|---|---|
| 1 | **Lazy no-journal write path** — defer bitmap/group-desc/superblock/dir-block writes as dirty buffers; keep the publication ORDER (pre→entry→post) but drain at durability points (fsync/sync/eviction/umount) instead of inline | no-journal ext4 mounts still write-back: create/mkdir/unlink are memory ops | 12,107 sync writes + 102,570 sync attempts + 761 whole-cache drains, ALL in phase A; mkdir avg 44 ms, pwrite 19 ms, unlink 55 ms | mkdir/pwrite/unlink/unlink-class avgs drop to <1 ms (the 3-6 sync I/Os per op vanish); provisioning window shrinks by an estimated 30-60 s of boot wall; 12 K device writes batch to ~1-2 K | crash-ordering: the publication queue must work journal-less (it already encodes the order; the drain points change) |
| 2 | **Stop the per-allocation superblock/group-desc/bitmap sync triple** (folds into 1; listed separately because it is mechanical even if 1 slips) | free-count updates are dirty metadata, written back in batches | 1,383 alloc + 213 free runs x 3 sync writes | -4 K sync writes in phase A; alloc path CPU+wall -60-80 % | trivial: mark dirty, skip the inline sync_dirty_buffer |
| 3 | **fsync scope: file's dirty blocks + its metadata only** — no whole-cache sync_buffers, no full-file block loop (track dirty-by-inode instead of scanning) | fsync flushes the file's dirty pages + one journal commit | 761 whole-cache drains; fsync avg 34-90 ms | fsync 34-90 ms → ~5-15 ms; removes a global stall (every drain blocks all ext4 users behind the buffer-cache locks) | medium: needs per-inode dirty tracking |
| 4 | **getdents64: iterate from f_pos** instead of re-materializing the directory each call | readdir resumes at the stored position, reads incrementally | 27,610 list_dir full re-scans; getdents avg 407 us, tot 11.4 s | getdents avg 407→<40 us (10x), tot → ~1 s | low: mechanical, position bookkeeping only |
| 5 | **Buffer cache sizing** — replace the fixed 1024 entries (4 MB) with a memory-fraction cap | page/buffer cache grows to fill RAM | 25,164 device reads + 24,895 evictions in phase A/B, victims 99.98 % clean | device reads -50-70 % (phase-A misses mostly vanish); eviction churn (each = LRU splice + hash unlink + free) mostly gone | low; watch memory accounting (fake-OOM lane owns the budget) |
| 6 | **bread CPU path** — 2.0 M calls/boot through bucket-irqsave + global-LRU-splice per call; batch LRU moves or make hit path lock-light | per-cpu buffer heads / iloc caching | 2.8 M breads, 46 K/s peak; open avg 156 us is mostly this + walk | open/newfstatat CPU -20-40 % | medium: lock-ordering doc (INV-LOCK-*) applies |
| 7 | **htree (dir_index) reads + writes** | indexed directories: O(log n) lookup; Linux creates them when dirs grow | 247 K linear scans over 1.73 M blocks; **ext4_add_entry returns EOPNOTSUPP on EXT4_INDEX_FL dirs** (writes into indexed dirs fail) | big-dir lookup -80-95 %; fixes a real compat hole (an indexed dir cannot take new entries today) | high: classic dx-tree surgery |
| 8 | **walk allocation churn** — normalize/copy strings per component per syscall (507 K walks) | single-pass walk, no per-component allocation | churn feeds the F1/F5 allocation-churn lever (magazines landed; churn hunt is the allocator's main lever) | open/stat CPU -10-20 % | low, but belongs to the churn-hunt lane |
| 9 | **metadata_csum_seed + orphan_file support (compat, A-line sc205)** — mount currently refuses unknown incompat 0x2000 (CSUM_SEED) → ENODEV x3 → beget ExecReboot → panic park; orphan_file (0x40000) same class. Also: metadata_csum **rw mounts are downgraded to RO** (sys_prod) because write-side csum maintenance is missing | Linux mounts both; csum writes maintained in metadata ops | A-line sc205 (userdata copy killed the boot); pa27/pa28/pa29 "downgraded to read-only" print x1/boot | unlocks userdata images Linux uses; RO-downgrade removal is a real capability (not perf) | csum_seed is small (the seed calc already exists in csum.rs); orphan_file needs real inode-cleanup semantics |

Recommended sequencing for the fs lane: 2 → 4 → 1 → 3 → 5 → 9(seed) →
6 → 7 → 8. Route 2 and 4 are low-risk immediate wins inside phase A/B;
route 1 is the big one and de-risks by reusing the publication queue;
route 9's csum_seed half is a two-line incompat-bit acceptance plus the
existing seed preference (csum.rs csum_seed()).

## 4. What this changes in the perf ranking

- F4 total weight: **medium-small** (~120-140 CPU-s/boot, all in the
  first 150 s), down from the "~300-500/~600 CPU-s" estimates that
  carried pa17's F3-inflated tails. Per-op factors (100-300x) unchanged.
- F4 does NOT own any deep-boot wall-clock (idle after t=150); it owns
  first-phase latency and the provisioning window.
- The jbd2/journal layer is dormant on this workload (all counters 0) —
  journal-commit cost is NOT an OH-boot cost center today; it matters
  for LTP-class workloads on journaled images.

## 5. Leg 43 addendum (2026-10-27): the two armed F4 follow-ons

Route 2 (counters) landed leg-31; route 4 (getdents cache) landed
leg-39/42 (strong -57.8% off main); route 3's whole-cache drain tail
was largely replaced by scoped publication draining (c3e5a314, main
0cd29a75; fsync -25.8%, mkdrmdir -21.7%, unlink -42.4% strong). The
remaining residuals decompose along the JOURNAL bit, and the two
targets are different images:

**(a) No-journal mounts — route 1 proper (the GNOME/OH side).** On the
OH/GNOME boots the journal never engages (fsw `j=` counters read
0.1010.0.0.0 at t=1710-1830: 1010 lazy stops, zero commits), so every
metadata write on the no-journal userdata still pays inline
synchronous I/O through `write_block_from_vec` → `bio::sync_dirty_buffer`
(namei.rs): bitmaps, group descs, inode-table blocks, dir blocks.
Design: arm the publication machinery WITHOUT a handle — identity
blocks become dirty-deferred with publication records (pre = the
op's bitmap/table/desc blocks, entry = the dir block, post = the
removal-side writes), drained at the standing durability points
(fsync/sync_buffers/eviction/high-water) which already enforce H1/H2
via the constraint flushes. The ordering logic (pre→entry→post) is
exactly what defer_entry_publication records under a handle; the
journal gate at its head is what needs a no-journal arm. Risk profile
unchanged from the map's route-1 row (crash-ordering); verification:
LTP mutation family + a crash-order review against the ghost-file
entry in the namei.rs comments. Expected: mkdir/unlink-class avgs on
no-journal images drop toward the write-back class (the map's <1 ms
estimate), provisioning window shrinks.

**(b) Journaled mounts — the mkdrmdir residual is the per-rmdir
REVOKE-FORCED synchronous commit.** jbd2_journal_stop already batches
commits lazily (LAZY_COMMIT_MAX_BUFFERS=1024, h_sync, or revoke
records pending). rmdir/unlink take revoke records, and pending
revokes FORCE the commit at stop: every rmdir pays a full synchronous
journal commit (descriptor + revoke blocks + write-through of the
transaction's registered buffers). Making revokes lazy requires
moving them from the journal-global `revoke_records` vec onto the
running transaction, committed with it IN ORDER — safe against
stale-replay only if reallocation of a revoked block before its revoke
reaches the log is handled (Linux's revoke-cancel records); that
semantics review is the precondition, not the code move. pbench's
mkdrmdir row (p6 median 55-58 ms strong class, min ~33 ms) is this
row's measurement handle; the fsw jcommit_fast counter (today 0 on OH,
should read ≈ rmdir count on pbench) is the mechanism probe.

**Arm-1 measurement status (leg 45): BLOCKED by a kernel failure —
no-journal root lookup.** The A/B instrument exists
(pbench_boot.py --nojournal: mkfs the probe rootfs with
^has_journal,^metadata_csum) but the boot dies at init selection:
ext4 mounts ("ext4 mounted / [ok]") yet every init candidate reads
"not found" (work/pbench-pa71nj1-serial.log). The image itself is
clean: a fresh mkfs passes e2fsck -fn, debugfs lists /test/pbench
(inode 13), and the only feature deltas vs the working journal image
are has_journal and orphan_file. The OH userdata image has the same
no-journal feature set and works on OH boots — but everything ever
read from it was written by the kernel itself; reading MKFS-WRITTEN
no-journal metadata appears never to have been exercised anywhere.
This must be fixed (fs lane) before route-1's expected-effect anchor
can be measured, and it is a compat hole in its own right
(mkfs.ext4 -O ^has_journal + use is a real-world pattern). Repro:
`python3 test/pbench_boot.py --bzimage <kernel> --tag nj
--nojournal`. Feature-bisect (pa71ob1, journal + ^orphan_file via
`--mkfs-features "^metadata_csum,^orphan_file"`): that image boots
and runs the full probe — orphan_file absence is exonerated; the
trigger is has_journal absence itself (mount succeeds, journal_ino==0
skips init_journal, but no mkfs-written path resolves afterward).
Leg-46 status: the fs fix has NOT landed on main as of 129a41fa; the
J-vs-NJ re-comparison stays queued on it.

**Arm-1 MEASURED (leg 50, pa85 — unblocked by main's zero-serve fix
1e27b095, in 2a960538: the root cause was a buffer-cache entry
published BH_Uptodate while its DMA had not landed; the no-journal
lookup death and the LTP all-zeros family were one bug).** One
release-strong kernel (run/pa84-main-strong-bzImage, a8a5afac =
2a960538 content), six boots rc=0 (interleaved J,NJ,NJ,J at load1
11-16 + one fsw=1 boot per arm). NJ boots now run the FULL probe —
the unblock is total. p6 medians (us), J1/J2 vs NJ1/NJ2:

| row | J | NJ | NJ/J | reading |
|---|---|---|---|---|
| p6_append | 85.5 / 90.4 | 2004 / 2058 | **23.1x** | every extending 4K write pays inline sync on no-journal — route-1's live target |
| p6_mkdrmdir | 15372 / 16272 | 16360 / 16148 | 1.03x | median FLAT: the ~15.5 ms per-mkdir scoped publication flush is journal-independent (both arms pay it; the fs lane's named residual) |
| p6_unlink | 113 / 82.8 | 78.8 / 78.5 | 0.80x | no-journal unlink already at write-back class (nothing to revoke/commit) |
| p6_write_fsync | 9612 / 10186 | 8821 / 9276 | 0.91x | fsync marginally cheaper without the journal commit |
| p6_stat / getdents / read_cached | 15.7-16.3 / 609-647 / 38.3-39.4 | 16.0-17.0 / 612-672 / 35.9-38.9 | ~1.0x | in band |

Tail exposure: NJ1's mkdrmdir avg read 167.8 ms (p99 3.78 s, max
12.3 s) vs the J arms' 19-21 ms — one NJ boot of two; NJ2 was clean
(19.2 ms avg). The medians are the honest comparison; the tail is a
risk note for bursty provisioning on no-journal images.

fsw mechanism (end-of-boot cumulative): J wr=15119 device writes,
sd=3083 real sync-dirty out of 22403 calls (19403 skipped under the
journal handle), j=7047.7046.6044.1002.853 (1002 fast commits, 853
revokes); NJ wr=17465 (+15%), sd=9360 real sync-dirty (**3.0x**, out
of 32389 calls), j=0.4009.0.0.0 (journal fully dormant). Scoped
publication flushes similar (859 vs 1006).

**Verdict (the expected-effect anchor for route 1, superseding the
map's phase-A estimates):** on CURRENT main, route-1's remaining
value is the WRITE path — ~1.95 ms per extending write (23x), with
the achievable target class proven by the J arm on the same kernel
(~88 us deferred). The map's "mkdir/pwrite/unlink-class avgs drop to
<1 ms" is superseded: unlink is already there (arm-2 + scoped
publication), the mkdir median is owned by the scoped flush (a
different lever, named for the fs lane), and append/pwrite-extension
is the live 23x target. Tail insurance on bursty metadata
(mkdrmdir p99 3.8 s) is secondary value.

**Arm-2 semantics note (leg 44) — what must be true before a lazy
revoke can be coded.** Current state, with the exact shapes:

- Storage: revokes live in `journal.revoke_records: Spinlock<Vec<(u64
  blocknr, Tid)>>` — JOURNAL-GLOBAL, not per-transaction
  (jbd2/revoke.rs insert_revoke_hash; jbd2_journal_revoke stamps the
  ISSUING transaction's tid but stores into the global table).
- Commit forcing: `jbd2_journal_stop` (transaction.rs ~line 247) skips
  the lazy-commit fast path when `!journal.revoke_records.is_empty()`
  — so ANY pending revoke, from ANY operation, forces a synchronous
  commit at the NEXT stop. That is the per-rmdir cost: unlink/rmdir
  issue revokes (frees of the inode/bitmap/dir blocks), and their
  stop commits descriptor + revoke blocks + write-through of every
  registered buffer synchronously.
- Commit drains the GLOBAL table: commit.rs calls take_revoke_records
  (mem::take of the whole vec) and writes all of it as revoke blocks
  of the COMMITTING transaction. A revoke issued by an operation
  still running in a LATER transaction can therefore be logged early
  (conservative: an early revoke only suppresses records at older log
  positions — recovery replays in tid order and `block_is_revoked_for
  (b, tid)` skips when revoke.tid >= descriptor tid).
- Cancel is a NO-OP STUB: jbd2_journal_cancel_revoke has no code (the
  "remove from hash table" body is empty), and the on-disk format has
  no cancel entry (write_one_revoke_record/flush_descriptor are also
  stubs). Nothing today un-revokes a block.

The hazard a lazy revoke introduces — reallocation of a revoked block
before its revoke reaches the log:

1. Op A (rmdir) frees block B and records revoke(B, tid_A), deferred
   (not yet logged). The free itself is order-protected by the
   publication machinery (post-capture: B's free persists after the
   entry removal) — that part is independent and stays.
2. Op B reallocates B for new content and journals a descriptor for it
   in transaction T2. Linux here CANCELS the pending revoke
   (revoke_cancel): same-transaction reuse cancels in memory;
   cross-transaction reuse (or an already-logged revoke) writes a
   CANCEL record (the revoke entry's high bit, JBD2_FLAG_REVOKE_CANCEL
   semantics) so recovery stops suppressing replays of B.
   Without cancel: crash after T2 commits but before A's revoke is
   logged leaves recovery replaying OLD records for B up to the revoke
   point and NEW records after — with a stale revoke tid arithmetic
   (`revoke.tid >= tid`) the OLD records for B that sit between the
   free and the revoke... specifically, the records written by any
   transaction with tid >= tid_A for OTHER purposes do not protect B,
   and the pre-free records for B (tid < tid_A, correct to suppress)
   stay suppressed — the DANGEROUS window is the reverse: A's revoke
   landing AFTER T2's new-content descriptor, suppressing T2's replay
   of B's new content (revoke.tid_A >= tid_T2 is false when tid_A <
   tid_T2, so with strictly increasing tids this direction is safe);
   the actual exposure is (i) B's new content never replayed while the
   allocation metadata IS replayed → leaked/lost block, and (ii) with
   the current GLOBAL drain, A's revoke may be written by an EARLIER
   committing transaction than tid_A — legal (early revoke suppresses
   only older records) — but once revokes are per-transaction this
   early-write disappears and the cancel obligation becomes real.

Requirements before any code move (the review checklist):

R1. Move revoke storage to per-transaction sets (issued-tid preserved);
    the commit writes only ITS transaction's set (plus, optionally,
    inherited unresolved ones — decide and document).
R2. Implement cancel: (a) in-memory cancel when the revoked block is
    re-registered in the SAME transaction; (b) an on-disk cancel entry
    (high-bit scheme) when a revoke already logged or issued by an
    earlier OPEN transaction must be undone — requires extending
    write_revoke_records AND scan_revoke_block (recovery must process
    cancels, not just revokes).
R3. Stop-condition change: drop the global `revokes_pending` force;
    replace with nothing (pure lazy, relies on R1+R2) or a bounded
    revoke-batch high-water like LAZY_COMMIT_MAX_BUFFERS. The forcing
    condition must NOT read other transactions' pending revokes.
R4. Recovery ordering audit: replay pass must interleave descriptor
    and revoke blocks in log order (it does today — scan is
    sequential); cancels processed at their log position; the final
    table answer for "replay B?" is the LAST log entry for B
    (revoke or cancel wins by position).
R5. Reallocation-cancel trigger site: allocator free→alloc reuse
    paths that journal a previously revoked block must route through
    the cancel (today the allocation goes through
    jbd2_journal_get_create_access — the cancel hook belongs there,
    where the buffer's JournalHead state is known).
R6. Verification plan: pbench mkdrmdir row (expect 55-58 ms → the
    low-ms class if the force was the whole residual), fsw
    jcommit_fast ≈ 0 under rmdir load (was ≈ rmdir count), crash-
    consistency spot: kill -9 mid-mkdrmdir-loop + e2fsck -n clean
    (the standing F4-R2 stance: recovery is e2fsck).

Expected effect if the whole residual is the forced commit:
mkdrmdir median 55-58 ms strong class → single-digit ms (the commit's
3-6 synchronous I/Os vanish; what remains is the scoped drain of the
op's own publication records).

## 6. Arm-2 semantics decision (leg 49): the lazy revoke CLEARS — per-transaction sets, no cancel records, slow path gains write-through

The leg-44 checklist above was written against Linux's shape. Re-reading
the code before touching it surfaced four facts about OUR jbd2 that
change the weight of the hazards, so this section records the corrected
model and the decisions it forces. Written before any code, per the
ledger's own rule.

### 6.1 Corrected facts (what our jbd2 actually does)

F-a. **Metadata is deferred, not written through, during ops.**
`bio::sync_dirty_buffer` (bio.rs) skips the sync whenever the current
task holds an active ext4 journal handle — the buffer stays BH_Dirty.
Persistence points are: the commit's write-through fast path, fsync/sync
(paths that set h_sync or call sync_buffers), and buffer-cache eviction.
(The Phase 2.5 comment in commit.rs claiming "already synced during the
operation" predates the deferral and is stale.)

F-b. **Slow-path journal records are born dead.** The slow path (taken
when revokes are pending) writes journal-area copies of the registered
blocks, revoke blocks, a commit block, then Phase 4 sets
tail = head and writes s_start past ALL of it. A COMPLETED slow commit
therefore leaves the operation's real metadata only in dirty RAM
buffers; recovery at next mount starts at s_start and never sees the
records. The commit's Phase 4 comment says as much ("this
implementation writes every journaled buffer THROUGH ... the log region
is immediately reusable") — but the write-through it describes is the
FAST path's loop; the slow path itself never syncs the registered
buffers' final locations.

F-c. **The replayable set at crash is at most one transaction.** Commits
are synchronous and inline in jbd2_journal_stop; there is one running
transaction at a time; each completed commit advances tail = head. So
the only records recovery can replay are those of a commit that was in
flight at crash time — specifically the window between its commit block
landing and its journal-superblock write (before the commit block,
PASS_SCAN requires one and replays nothing). Multi-transaction replay
sets cannot occur in this design.

F-d. **Today's rmdir commit already leaves rmdir metadata unpersisted.**
The per-rmdir forced commit takes the slow path (revokes pending), which
per F-b does not write through. So going lazy does not weaken rmdir's
own persistence: both before and after, the freeing metadata rides dirty
buffers to the later drain points, and recovery is e2fsck (the standing
F4-R2 stance).

### 6.2 Decisions on R1-R6

**R1 — per-transaction set + a spill, no global force.** Revokes move
to `Transaction::t_revoke_records: Spinlock<Vec<(u64, Tid)>>`, deduped
per (block, tid). `Journal::revoke_records` keeps two residual roles:
the RECOVERY accumulator (scan_revoke_block fills it, PASS_REPLAY's
filter reads it, recovery clears it — unchanged), and a SPILL for the
rare revoke issued while its transaction is no longer Running (an SMP
handle joining a committing transaction — start_this_handle does not
check t_state — or a stale handle on a Finished transaction). The spill
is drained into the next transaction at creation
(jbd2_get_transaction). Lock order: j_running_transaction, then the
revoke vec(s); no path takes them in reverse. The spill choice is R1's
"inherited unresolved ones" option, scoped to the only case that needs
it.

**R2 — no cancel records, in memory or on disk.** The dangerous
direction (a revoke suppressing the replay of NEWER content) requires
revoke.tid >= the descriptor's tid with the content newer than the
revoke. With per-transaction sets that cannot arise: a revoke is stamped
with its issuing transaction's tid and is logged by that transaction (or
a later one, via the spill, under a still-later header tid), while any
descriptor carrying post-revoke content for the block belongs to a
transaction with a strictly greater tid — the filter
(`revoke.tid >= desc_tid`) does not suppress it. Same-tid suppression
(realloc inside the issuing transaction) skips replay of that block's
own commit-time snapshot; the content still reaches disk through the
commit's write-through pass (R2') and the standing deferral drain
points. This divergence from Linux (no JBD2_FLAG_REVOKE_CANCEL needed)
is a consequence of the write-through commit model, not an oversight;
recorded in docs/architecture/design-divergences.md.
`jbd2_journal_cancel_revoke` stays a stub, its comment upgraded to say
why.

**R2' — the slow path gains the fast path's write-through loop.** With
revokes riding transactions, mixed batches (writes + a pending revoke)
would flip commits that are fast-path today into slow-path — and per
F-b the slow path does not persist. Without this addition the change
would silently widen the dirty window for write-heavy batches sharing a
window with any pending revoke. With it, slow ≡ fast + revoke/commit/
superblock bookkeeping: sync every registered buffer to its final
location first (same loop, same per-block publication-constraint guard),
then write the journal-area copies, revoke blocks, commit block, and
superblock exactly as now. The journal-area copies stay for format
compatibility; they remain born dead per F-b (documented there).

**R3 — drop the global force; bound with a revoke high-water.** The
lazy-skip condition in jbd2_journal_stop stops reading
journal.revoke_records and instead forces a commit when the
transaction's OWN set reaches `LAZY_COMMIT_MAX_REVOKES = 510` (one
revoke block's worth at the smallest entry size: (4096 - 12) / 8).
Rationale: ext4 callers use plain ext4_journal_start(nblocks) — no
revoke-credit reservation — so no existing bounder covers revoke-only
volume (an unlink loop registers a handful of unique buffers but
thousands of revokes); tids and log space stay bounded at one revoke
block per forced commit. The forcing condition reads no other
transaction's state (R3's own requirement).

**R4 — recovery unchanged.** The on-disk format does not change: revoke
blocks already carry the committing transaction's tid in their header,
entries are plain block numbers, PASS_SCAN collects them into the
recovery table, PASS_REPLAY filters by `revoke.tid >= desc_tid`, and
for the single-transaction replay sets (F-c) that is exactly log-order
semantics. In-memory entry tids are bookkeeping only; recovery
re-derives tid from block headers.

**R5 — no cancel hook in get_create_access.** Follows from R2: same-tid
suppression is benign (content is delivered by write-through), cross-tid
is not suppressed. The allocation reuse paths need no change.

**R6 — verification.** (i) pbench A/B on release-strong kernels
(M = merged main control, B = branch): p6_mkdrmdir and p6_unlink are
the target rows (prediction below); p6_write_fsync, p6_append, p6_stat
and the P5 rows are the in-band guards (fsync still forces commits, now
carrying the write-through pass — watch row). (ii) fsw mechanism probe:
jcommit_slow under the pbench boot drops from ≈ unlink+rmdir count to a
handful (batch count); jlazy absorbs the rest. (iii) Correctness:
dual-arch build rc=0, riscv64 minimal-rootfs smoke zero panics,
`make test` green. (iv) Crash-consistency class is unchanged per F-d
(rmdir metadata was already dirty-after-commit; the change adds
persistence via R2', removes none); the standing e2fsck stance applies
unchanged.

### 6.3 Expected effect (updated with the deferral model)

Per rmdir today: one forced slow commit = 1 descriptor + k journal-area
copies (k = unique registered blocks: dir block, parent dir, inode
table, bitmap, group desc, superblock — deduped by blocknr, so ~5-8) +
1 revoke block + 1 commit block + 1 journal superblock ≈ 8-12
synchronous virtio round trips, matching the 52-58 ms strong median
class. After the change: 0 synchronous I/Os per op; one batch commit per
510 revokes / 1024 unique buffers / fsync, with the shared identity
blocks deduped to one write each. Prediction: p6_mkdrmdir strong median
→ single-digit ms; p6_unlink (same per-op forced commit, ~16 ms class
today) → low ms; the amortized batch commit adds ≤ (2k + 3) syncs per
~500 delete ops.

