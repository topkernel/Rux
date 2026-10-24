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
