# Memory Compaction Design Document

## 1. Overview

Memory compaction reduces **external fragmentation** by relocating movable pages to consolidate free blocks at low addresses, enabling high-order allocations (e.g. huge pages) that would otherwise fail despite sufficient total free memory.

**Rux implementation**: `kernel/src/mm/compact.rs` (~395 lines)
**Reference**: `refer/linux/mm/compaction.c` (~5000 lines) + `refer/linux/mm/migrate.c` (~1600 lines)

---

## 2. Rux Current Implementation

### 2.1 Architecture

```
compact_zone(zone, order)
    └── compact_zone_inner(cc)
            ├── find_free_page(cc)      // free scanner (DOWN)
            ├── find_migrate_page(cc)   // migrate scanner (UP)
            ├── migrate_page(src, dst)
            │       ├── try_to_unmap(src)
            │       ├── copy_page_contents(src, dst)
            │       ├── remap_page(dst, vaddr)
            │       └── free_pages(src)
            └── zone.has_free_block(order) // check success
```

### 2.2 Data Structures

```rust
pub enum CompactResult {
    Success,    // found a free block of requested order
    Complete,   // zone fully scanned, no suitable block
    Skipped,    // no movable pages to compact
}

struct CompactControl {
    zone: *mut Zone,
    migrate_pfn: usize,    // scans upward from zone start
    free_pfn: usize,       // scans downward from zone end
    order: usize,          // target allocation order
    nr_migrated: usize,    // pages successfully migrated
    nr_scanned: usize,     // total pages scanned (limit: 4096)
}
```

### 2.3 Core Algorithm

**Two-pointer scan** — migrate scanner moves UP, free scanner moves DOWN:

```
Zone: [low_pfn .................................................. end_pfn]
              ^ migrate_pfn (UP)                  free_pfn (DOWN) ^
```

Each iteration:
1. `find_free_page()` — try buddy allocator first (fast path), then walk downward from `free_pfn`
2. `find_migrate_page()` — walk upward from `migrate_pfn`, checking per-page eligibility
3. `migrate_page(src, dst)` — unmap → copy → remap → free source
4. Check if buddy free list now has a block of target order → return `Success`

Termination conditions:
- Scanners meet (`migrate_pfn >= free_pfn`)
- Scan limit reached (`nr_scanned >= 4096`)
- No more free pages or no more movable pages

### 2.4 Migrate Page Selection Criteria

A page is eligible for migration if **all** conditions are met:

| Condition | Check | Rationale |
|-----------|-------|-----------|
| Not free | `!is_free()` | Skip already-free pages |
| Not reserved | `!test_flag(Reserved)` | Reserved pages (kernel image, device mappings) are immovable |
| Anonymous | `is_anonymous()` | Only anonymous pages are movable (file-backed pages not supported) |
| Mapped | `page_mapped()` | Unmapped pages have no PTEs to update |
| refcount == 1 | `refcount() == 1` | Only page-table references; no extra pins (GUP, pipes, etc.) |
| Not dirty | `!is_dirty()` | Avoid writeback complexity during compaction |

### 2.5 Page Migration Flow

```
migrate_page(src_pfn, dst_pfn):
  1. Save old_vaddr from src.index (stored as VPN)
  2. try_to_unmap(src)          → remove all PTEs across all tasks
  3. copy_page_contents(src, dst) → memcpy 4096 bytes (phys→phys)
  4. remap_page(dst, old_vaddr)  → install new PTEs with dst PFN
  5. Transfer metadata: mapping, index, Anonymous/SwapBacked/Referenced flags, refcount=1
  6. free_pages(src_pfn, 0)     → release source to buddy (triggers merge)
```

### 2.6 PTE Remap (`remap_page`)

Walks all tasks' page tables to find and update PTEs:

1. `for_each_task()` — iterate all tasks
2. Check if task has an address space (`mm`)
3. Check if any anonymous VMA contains `old_vaddr`
4. Walk page table (PGD → PUD → PMD → PTE) to find the PTE
5. Update PPN bits while preserving all flags (R/W/X/U/D/A/G):
   ```
   new_pte = (old_pte & ~PPN_MASK) | (new_pfn << 10)
   ```
6. `sfence.vma` per-address TLB flush
7. Increment mapcount on the new page

### 2.7 Compaction Trigger

In `page_alloc.rs`, compaction is triggered as a **synchronous fallback** when a high-order allocation fails:

```rust
// alloc_pages() fallback path
if order > 0 && allocation_failed {
    let cr = compact_zone(zone, order);
    if cr == CompactResult::Success {
        // retry allocation from buddy
    }
}
```

### 2.8 Zone Helper Methods

Two methods added to `Zone` for compaction support:

- `alloc_single_page()` — allocate order-0 from buddy (used as migration destination)
- `has_free_block(order)` — check if buddy free list has a block of given order

---

## 3. Differences from Reference Implementation

### 3.1 Structural Comparison

| Aspect | Rux | Reference |
|--------|-----|-----------|
| Total code | ~395 lines (1 file) | ~6600 lines (2 files) |
| CompactResult | 3 variants | 9 variants (5 success states + 4 failure states) |
| CompactControl fields | 6 | 30+ |
| Page migratetype | Not implemented | 4 types: Movable/Unmovable/Reclaimable/Isolate |
| Pageblock order | Not implemented | Configurable (default: MAX_ORDER-1) |
| Background daemon | Not implemented | kcompactd per-NUMA-node |
| Migration entries | Not implemented | Swap-like PTE markers |
| Batch migration | Not implemented | Migrate pages in batches (two-phase) |

### 3.2 CompactResult Differences

**Rux** — 3 simple variants:

```rust
enum CompactResult {
    Success,    // found free block
    Complete,   // zone scanned, no block
    Skipped,    // no movable pages
}
```

**Reference** — 9 fine-grained variants:

```rust
enum compact_result {
    COMPACT_SKIPPED,         // compaction not attempted
    COMPACT_DEFERRED,        // defer compaction (rate-limited)
    COMPACT_CONTENDED,       // aborted due to lock contention
    COMPACT_PARTIAL_SKIPPED, // partial scan, some pages isolated
    COMPACT_COMPLETE,        // full zone scan, may or may not have block
    COMPACT_PARTIAL,         // partial scan, has free block
    COMPACT_SUCCESS,         // full scan, found free block
    COMPACT_NO_SUITABLE_PAGE,// full scan, no suitable pages
    COMPACT_NOT_SUITABLE_ZONE, // zone not suitable for compaction
};
```

Key differences:
- Reference distinguishes between **partial** and **full** scans (CONTENDED, PARTIAL, COMPLETE, SUCCESS)
- Reference has **DEFERRED** state for rate-limiting repeated compaction attempts
- Reference has **CONTENDED** for aborting on lock contention (SMP)
- Reference tracks whether the free block is at the right position for the caller

### 3.3 CompactControl Field Differences

**Rux** — 6 fields:

```rust
struct CompactControl {
    zone: *mut Zone,
    migrate_pfn: usize,
    free_pfn: usize,
    order: usize,
    nr_migrated: usize,
    nr_scanned: usize,
}
```

**Reference** — 30+ fields (key additions):

| Field | Purpose | Rux Status |
|-------|---------|------------|
| `nr_freepages` / `nr_migratepages` | Isolated page counts | Not tracked |
| `migratetype` | Target page migratetype | Not implemented |
| `alloc_flags` | GFP flags for allocation | Not passed through |
| `pfn` / `start_pfn` / `end_pfn` | Scan boundaries (may differ from zone) | Uses zone boundaries directly |
| `zone` / `contended` | Zone pointer + contention flag | Zone only |
| `ignore_skip_hint` | Whether to ignore cached skip hints | Not applicable |
| `finish_pageblock` | Finish current pageblock before stopping | Not implemented |
| `nr_pageblocks_skipped` | Skip hint statistics | Not tracked |
| `order` | Target order | Implemented |
| `search_order` | Fallback search order | Not implemented |
| `total_migrate_scanned` / `total_free_scanned` | Accumulated scan counters | Not tracked |
| `classify_zone` | Zone classification (suitable/not) | Not implemented |
| `capture_control` | Stolen page tracking for CMA | Not implemented |
| `alloc_contig` | Contiguous allocation mode | Not implemented |
| `proactive_compact` | Proactive compaction trigger | Not implemented |
| `mode` | COMPACT_ASYNC/SYNC_FULL | Not implemented (always SYNC_FULL) |

### 3.4 Page Scanning Differences

#### Migrate Scanner

**Rux `find_migrate_page()`** — 6 simple checks:

```rust
if is_free()           → skip
if Reserved            → skip
if !is_anonymous()     → skip  (no file-backed, no slab)
if !page_mapped()      → skip
if refcount() != 1     → skip  (no extra pins)
if is_dirty()          → skip
```

**Reference `isolate_migratepages_block()`** — 18+ per-page checks:

1. `PageBuddy` — skip buddy free pages
2. `PageHuge` — skip huge pages (handled separately)
3. `PageOffline` — skip offline pages (hot-plug)
4. Page migratetype check — skip Unmovable/Reclaimable pages in Movable block
5. `__PageMovable` — skip pages with `a_ops->migratepage` (device pages)
6. `PageTransHuge` — transparent huge page handling (split or skip)
7. `PageUnevictable` — skip unevictable pages (mlock)
8. `PageIsolated` — skip already-isolated pages
9. `PageSlab` — slab pages are handled via shrinker, not direct migration
10. `PageSwapCache` — swap cache pages need special handling
11. `PageCompound` — compound page order check
12. `PageLRU` — must be on LRU list to isolate
13. `PageMlocked` — mlocked pages cannot be migrated
14. `PageWriteback` — cannot migrate during writeback
15. `PageDirty` — Rux also checks this
16. `PageActive` — active/inactive LRU distinction
17. Page reference count check (using `folio_expected_ref_count()`)
18. `page_mapped()` — Rux also checks this
19. Trylock page lock — skip if page is locked by another CPU
20. `isolate_lru_page()` — atomically remove from LRU list

After identifying a movable page, the reference:
- **Isolates** the page (removes from LRU list, sets `PageIsolated` flag)
- Adds to `cc->migratepages` list for batch processing
- Advances to next **pageblock** boundary (not per-page)

#### Free Scanner

**Rux `find_free_page()`** — two strategies:

```rust
// Fast path: buddy allocator
zone.alloc_single_page()

// Slow path: walk downward, check is_free() && !Reserved
```

**Reference `isolate_freepages_block()`** — per-order freelist handling:

1. Scan within pageblock boundaries
2. For each free page, determine its buddy order from `PageBuddy` order
3. **Split** free blocks of higher order to get order-0 pages
4. Isolate order-0 pages from buddy free list
5. Skip order-0 pages that belong to wrong migratetype
6. Track remaining free pages for later re-insertion

Key difference: reference isolates free pages from the buddy allocator and tracks them separately, allowing rollback if migration fails. Rux directly allocates from buddy (no rollback path).

### 3.5 Page Migration Differences

#### Migration Entries

**Rux** — No migration entries. Uses direct unmap+remap:

```
try_to_unmap(src) → copy → remap_page(dst, vaddr) → free(src)
```

Problem: Between `try_to_unmap()` and `remap_page()`, the page has **no PTE mappings**. If a concurrent page fault occurs on the old virtual address, it would allocate a **new** zeroed page (via demand paging), causing data loss.

Current mitigation: compaction runs with interrupts disabled (preempt_disable via spinlock), and the window between unmap and remap is very small (microseconds for a single page copy).

**Reference** — Uses **migration entries** (special swap-like PTE markers):

```
try_to_unmap(src)  →  install migration PTE  →  copy  →  remove migration PTE  →  remap
                           ^                                           ^
                    If page fault hits this PTE:                         |
                    → wait for migration to complete                    |
                    → use new page                                      |
```

Migration entries:
- Replace the normal PTE with a special entry: `swp_entry(MIGRATION_ENTRY, pfn)`
- A concurrent page fault on this entry will call `migration_entry_wait()` and block
- After `remove_migration_ptes()` installs the new PTE, the waiting task resumes
- This provides **correctness under concurrency** — no data loss possible

#### Batch Migration

**Rux** — Migrates one page at a time:

```rust
loop {
    find_free_page()     → dst_pfn
    find_migrate_page()  → src_pfn
    migrate_page(src, dst)  // unmap + copy + remap immediately
}
```

**Reference** — Two-phase batch migration:

```
Phase 1: Isolate
  - isolate_migratepages_block() → collect pages into cc->migratepages list
  - isolate_freepages_block()    → collect free pages into cc->freepages list
  - Up to COMPACT_CLUSTER_MAX (32) pages per batch

Phase 2: Migrate
  - migrate_pages() → iterates the isolated list
  - For each page:
    a. try_to_unmap(page)              → unmap + install migration entries
    b. migrate_folio_move(page, new):
       - folio_lock(old)
       - copy_highpage(new, old)       → copy contents
       - remove_migration_ptes(old)    → replace migration entries with new PTEs
       - folio_unlock(old)
    c. If migration fails → putback_lru_page() → re-insert to LRU
    d. If migration succeeds → free old page to buddy
```

Key differences:
- Reference isolates pages first, then migrates in batch (better cache locality)
- Reference supports migration **failure** per-page (putback to LRU)
- Rux cannot recover from a failed migration (page is already unmapped)
- Reference processes up to 32 pages per cluster; Rux processes 1

### 3.6 Pageblock Migratetype

**Rux** — Not implemented. All pages are treated uniformly; migratetype is determined at scan-time by checking page flags.

**Reference** — Pages are grouped into **pageblocks** (default: `MAX_ORDER - 1` pages = 1024 pages = 4MB). Each pageblock has a migratetype stored in `page->flags`:

| Migratetype | Description | Can compact? |
|-------------|-------------|--------------|
| MIGRATE_UNMOVABLE | Kernel pages, slab | No |
| MIGRATE_MOVABLE | Anonymous, page cache | Yes |
| MIGRATE_RECLAIMABLE | File-backed, reclaimable | Yes (after reclaim) |
| MIGRATE_ISOLATE | Temporary isolation state | No |

Benefits:
- **Skip hints**: After scanning a pageblock with mostly unmovable pages, set a skip hint to avoid re-scanning
- **Defer mechanism**: If too many pageblocks are skipped, defer compaction entirely
- **Partial compaction**: Can compact only Movable pageblocks within a zone
- **CMA (Contiguous Memory Allocator)**: Isolate Movable pageblocks for CMA allocations

### 3.7 kcompactd Background Daemon

**Rux** — No background compaction. Compaction is only triggered synchronously from `alloc_pages()`.

**Reference** — `kcompactd` per-NUMA-node kernel thread:

```
Trigger conditions:
  - Watermark hit: kswapd cannot free enough pages, escalates to kcompactd
  - Proactive compaction: /proc/sys/vm/compact_memory or sysctl trigger
  - Fragmentation index exceeds threshold (extfrag_threshold)
  - Direct compaction request from memory hotplug

Behavior:
  - Runs at lowest scheduling priority
  - Can be woken with specific target order and priority
  - Has deferral mechanism to avoid thrashing
  - Per-node kcompactd: kcompactd_max_threads controls parallelism
```

### 3.8 Compaction Trigger & Retry

**Rux** — Simple fallback:

```rust
if order > 0 && alloc_failed {
    compact_zone(zone, order);
    retry allocation;  // single retry
}
```

**Reference** — Multi-level retry with priority escalation:

```
__alloc_pages_slowpath():
  1. __alloc_pages_direct_reclaim()   → try reclaim first
  2. __alloc_pages_direct_compact()   → try compaction
     - compact_priority = DEF_COMPACT_PRIORITY (INITIAL)
     - Loop up to MAX_COMPACT_RETRIES (16) times:
       a. compact(zone, order, priority)
       b. If COMPACT_SUCCESS → retry allocation → done
       c. If partial success → decrease priority, continue
       d. If CONTENDED → retry with lower priority
       e. should_compact_retry() → check worth retrying:
          - retry_count < MAX_COMPACT_RETRIES
          - compaction made progress (pages migrated)
          - fragmentation index still high
  3. __alloc_pages_may_oom()          → OOM killer as last resort
```

Key differences:
- Reference tries **reclaim first**, then compaction (Rux goes straight to compaction)
- Reference has **priority levels** that control compaction aggressiveness (scan depth, defer checks)
- Reference has **16 retries** with progress-based decisions
- Reference calls OOM killer if compaction also fails
- `should_compact_retry()` considers: watermark status, compaction progress, fragmentation index, order requirement

### 3.9 Skip Hints & Defer Mechanism

**Rux** — Not implemented. Every compaction pass scans the entire zone.

**Reference** — Skip hints to avoid wasted work:

```
Pageblock skip hints:
  - After scanning a pageblock, if >75% pages are unmovable → set skip hint
  - Future compaction passes skip marked pageblocks
  - Skip hint expires after COMPACT_SKIP_DEFRAG_COUNT (8) compaction attempts
  - Compaction with ignore_skip_hint=true can override (high priority)

Defer mechanism:
  - If >50% pageblocks have skip hints → set zone compact_deferred
  - Deferred zone is skipped for COMPACT_DEFERRED_SHIFT (6) allocation attempts
  - Prevents CPU waste on heavily fragmented zones
```

### 3.10 Fast Search Optimizations

**Rux** — Linear scan only.

**Reference** — Two fast-path optimizations:

1. **`fast_find_migrateblock()`**:
   - Maintains a cached "migrate block" PFN per zone
   - On subsequent compaction, start scanning from cached position
   - Reduces redundant scanning of already-compacted regions

2. **`fast_isolate_freepages()`**:
   - When looking for a free page, first check cached free block PFN
   - Uses per-order free area to find highest-order free block quickly
   - Falls back to linear scan only if cache miss

### 3.11 Locking & Concurrency

**Rux** — Minimal locking:

- `Zone` spinlock held during `alloc_single_page()` (buddy allocation)
- No page-level locking during migration
- `preempt_disable()` implied by existing spinlock usage
- No TLB flush batching (per-address `sfence.vma`)

**Reference** — Multi-level locking:

- Zone lock for buddy operations
- Page lock (`folio_lock`) during migration (prevents concurrent access)
- LRU lock for page isolation
- `lru_cache_disable()` during migration to prevent new LRU additions
- TLB flush batching: `try_to_unmap()` collects pages, then `flush_tlb_range()` once
- `compact_lock_irqsave()` / `compact_unlock_irqrestore()` helpers

### 3.12 CMA (Contiguous Memory Allocator) Integration

**Rux** — Not implemented.

**Reference** — Compaction is the primary mechanism for CMA allocations:

```
alloc_contig_range():
  1. isolate_migratepages_range()  → isolate all movable pages
  2. migrate_pages()               → migrate to new locations
  3. isolate_freepages_range()     → isolate remaining free pages
  4. alloc_contig_migrate_range()  → second migration pass for stragglers
  5. free_contig_range()           → release the contiguous block
```

CMA uses `capture_control` to track "stolen" pages — pages that were migrated to make room for CMA but whose allocation was later cancelled.

### 3.13 Memory Hotplug Support

**Rux** — Not implemented.

**Reference** — Compaction supports memory hotplug operations:

- `offline_pages()` — offline a memory section by migrating all pages away
- `migrate_misplaced_page()` — NUMA page migration
- `migrate_to_node()` — explicit node migration

---

## 4. Known Limitations of Current Implementation

| # | Limitation | Impact | Severity |
|---|-----------|--------|----------|
| 1 | No migration entries | Concurrent page fault during migration window can cause data loss (theoretically) | Medium |
| 2 | No page-level locking | Concurrent GUP (get_user_pages) on migrating page is unsafe | Medium |
| 3 | No file-backed page migration | Only anonymous pages can be compacted | Medium |
| 4 | No batch migration | Poor cache locality, no per-page failure recovery | Low |
| 5 | No kcompactd | Compaction only runs synchronously on allocation failure | Low |
| 6 | No skip hints | Repeated compaction scans same unmovable pages | Low |
| 7 | Single retry | Only one compaction attempt per allocation failure | Low |
| 8 | Linear free scanner | No cached free block position for fast search | Low |
| 9 | No THP migration | Transparent huge pages are skipped | Low |
| 10 | No PCP drain | Per-CPU pageset pages are invisible to compaction | Low |

---

## 5. Improvement Roadmap

### Phase A: Concurrency Safety (High Priority)

#### A1. Migration Entries

Implement migration PTE markers to handle concurrent page faults:

```
try_to_unmap(page):
  - Instead of clearing PTE, install: swp_entry(MIGRATION_ENTRY, old_pfn)
  - Concurrent page fault on migration entry → migration_entry_wait()

remove_migration_ptes(old_page, new_page):
  - Walk all tasks' VMA trees (reuse rmap)
  - Replace migration entries with PTEs pointing to new_page
  - Wake all waiters via page wait queue
```

Files: `compact.rs`, `mm/page_table.rs`, `mm/swap_entry.rs` (new)

#### A2. Page Locking

Add page-level spinlock for migration:

```
migrate_page():
  - folio_lock(src)  → fail if locked by another CPU
  - try_to_unmap()   → install migration entries
  - copy + remap
  - folio_unlock(src)
  - free src
```

Files: `page_desc.rs` (add page lock), `compact.rs`

#### A3. TLB Flush Batching

Replace per-address TLB flush with batched flush:

```
try_to_unmap() returns list of addresses that were unmapped
After all pages in batch are migrated:
  flush_tlb_range(mm, start, end)  // single shootdown
```

Files: `compact.rs`, `arch/riscv64/mm/tlb.rs`

### Phase B: Robustness (Medium Priority)

#### B1. Pageblock Migratetype

Implement pageblock-based page classification:

```
Constants:
  PAGEBLOCK_ORDER = MAX_ORDER - 1 = 9  (512 pages = 2MB)
  MIGRATE_UNMOVABLE = 0
  MIGRATE_MOVABLE = 1
  MIGRATE_RECLAIMABLE = 2
  MIGRATE_ISOLATE = 3

Page flag bits: PB_migrate_skip, PB_migrate_type (2 bits)

During boot:
  - Mark all pageblocks as MIGRATE_MOVABLE initially
  - Re-classify as pages are allocated (slab → UNMOVABLE, etc.)

During compaction:
  - Only scan MIGRATE_MOVABLE pageblocks for migration candidates
  - Skip UNMOVABLE pageblocks (kernel, slab)
```

Files: `page_desc.rs` (flag bits), `compact.rs`, `mm/pageblock-flags.rs` (new)

#### B2. Skip Hints & Defer

Implement pageblock skip caching:

```
After scanning a pageblock:
  - Count unmovable pages
  - If unmovable > 75% → set PB_migrate_skip flag
  - Track per-zone skip count

Defer mechanism:
  - If skipped_pageblocks > 50% of total → zone.compact_deferred = true
  - Reset defer after COMPACT_DEFERRED_SHIFT (6) allocation attempts
```

Files: `compact.rs`, `zone.rs` (defer counter)

#### B3. Batch Migration

Rewrite to isolate-then-migrate pattern:

```
struct CompactControl {
    migratepages: List<Page>,  // isolated source pages
    freepages: List<Page>,     // isolated destination pages
    nr_migratepages: usize,
    nr_freepages: usize,
}

compact_zone_inner():
  Phase 1: Isolate
    - isolate_migratepages_block() → fill migratepages list (max 32)
    - isolate_freepages_block()    → fill freepages list

  Phase 2: Migrate
    for (src, dst) in zip(migratepages, freepages):
      if !migrate_page(src, dst):
        putback_lru_page(src)  // rollback on failure
```

Files: `compact.rs`

### Phase C: Performance (Lower Priority)

#### C1. kcompactd Daemon

Background compaction thread:

```
kcompactd per zone:
  - Woken by kswapd when watermark not met after reclaim
  - Woken by write to /proc/sys/vm/compact_memory
  - Runs at lowest scheduler priority
  - Compacts with async priority (limited scan depth)
  - Sleeps on wait queue when idle
```

Files: `compact.rs` (new kcompactd module), `sched/kthread.rs`

#### C2. Fast Search Caching

Cache scanner positions between compaction passes:

```
Zone:
  cached_migrate_pfn: AtomicUsize,  // start next scan from here
  cached_free_pfn: AtomicUsize,

compact_zone():
  - Start from cached positions instead of zone boundaries
  - Update cache on compaction completion
```

Files: `compact.rs`, `zone.rs`

#### C3. Retry with Priority

Multi-level compaction retry in page allocator:

```
__alloc_pages_direct_compact():
  for priority in [DEF_PRIORITY .. MIN_PRIORITY]:
    result = compact_zone(zone, order, priority)
    match result:
      Success  → retry alloc → return
      Partial  → decrease priority, continue
      Contended → retry with lower priority
      Complete → break
```

Files: `page_alloc.rs`, `compact.rs`

#### C4. File-backed Page Migration

Extend compaction to handle page cache pages:

```
find_migrate_page():
  - Also accept file-backed pages (PageCache flag)
  - Check page->mapping->a_ops->migratepage
  - If migratepage is supported → migrate via filesystem callback
  - If not → skip (or trigger writeback + reclaim first)
```

Files: `compact.rs`, `fs/page_cache.rs`

#### C5. THP Migration

Support migrating transparent huge pages:

```
find_migrate_page():
  - Detect PageTransHuge
  - Isolate as 2MB block
  - Allocate 2MB destination (split from buddy if needed)
  - Copy PMD-mapped contents
  - Update PMD entries instead of PTE entries
```

Files: `compact.rs`, `mm/hugepage.rs`

---

## 6. Summary Matrix

| Feature | Rux Status | Reference Status | Phase |
|---------|-----------|-----------------|-------|
| Two-pointer scan | Implemented | Implemented | -- |
| Migrate page criteria (6 checks) | Implemented | 18+ checks | -- |
| Single-page migration | Implemented | Batch (32 pages) | B3 |
| Direct unmap+remap | Implemented | Migration entries | A1 |
| Synchronous trigger only | Implemented | Sync + async (kcompactd) | C1 |
| Pageblock migratetype | Not implemented | 4 types | B1 |
| Skip hints & defer | Not implemented | Implemented | B2 |
| Fast search caching | Not implemented | Implemented | C2 |
| Multi-retry with priority | Not implemented | 16 retries | C3 |
| CMA integration | Not implemented | Implemented | -- |
| Memory hotplug | Not implemented | Implemented | -- |
| NUMA page migration | Not implemented | Implemented | -- |
| File-backed migration | Not implemented | Implemented | C4 |
| THP migration | Not implemented | Implemented | C5 |
| Capture control | Not implemented | Implemented | -- |
| PCP drain before compact | Not implemented | Implemented | -- |
| Fragmentation index | Not implemented | Implemented | -- |
| Page-level locking | Not implemented | Implemented | A2 |
| TLB flush batching | Not implemented | Implemented | A3 |

---

**Document Version**: v1.0
**Last Updated**: 2026-04-07
