# rv-mm review lane — findings (2026-10-30)

Scope: `kernel/src/mm/**` (all 28 files, line-by-line). Watch areas:
PFN-span off-by-ones, refcount leaks, COW edges.
Branch: `rv-mm`. Builds verified per fix: `make build`, `make build
PLATFORM=x86_64`, `make build PLATFORM=aarch64` — all rc=0. Targeted test:
minimal-rootfs riscv64 boot smoke (2 CPU / 1 G, snapshot drive) reaches the
shell, zero KERNPANIC/ALLOCTHROW, zero free-refusal lines.

## FIXED (3)

### F1. deferred_free: drain leaked every slab-resident entry (+ deadline stall)
`mm/deferred_free.rs` — commit `a90b76fc`.

- Registered deferred size classes (Arc<MmStruct>, Arc<FdTable>,
  Arc<SignalStruct>) are <= 4 KiB / 8-aligned layouts; whenever the static
  slab is up (main.rs:348, early boot) `CombinedAllocator::alloc` serves
  them from the slab region via kmalloc, NOT the buddy heap.
- `drain()` unconditionally called `dealloc_untracked_pub`, whose
  out-of-heap guard refuses slab-region pointers (FR_OUT_OF_WINDOW /
  DEALLOC_OUTSIDE) — each such entry was permanently leaked. The drain doc
  comment claimed it routed through `dealloc_direct`; the code never did.
- Secondary bug in the same loop: when a drain stopped at the 64-entry
  batch limit with more DUE entries behind it, `EARLIEST_DEADLINE` went to
  `u64::MAX` and the alloc-path drain hook slept until the next matched
  free lowered it — all remaining due entries stalled up to DEFER_MS (or
  indefinitely on a pool that only drains).
- Fix: route drained entries by region exactly like `dealloc_direct`
  (slab region -> `kfree`, heap -> untracked buddy; neither re-defers),
  and fold the first surviving entry's deadline into the watermark on
  batch-limit truncation.

Gates: contract (a deferred free must eventually return to its owning
allocator) / triple build rc=0 / boot smoke clean.

### F2. heap buddy: merge-past-MAX_ORDER orphaned the buddy half
`mm/buddy_allocator.rs` — commit `bab50e52`.

- `free_blocks`' loop guard (`current_order > MAX_ORDER`) let two
  MAX_ORDER buddies merge first; the merged block was then registered at
  MAX_ORDER while `remove_from_free_list` had already cleared the buddy
  half's free bits — 2^MAX_ORDER pages on no list, invisible to the
  allocator and the census (permanent capacity loss per event).
- The page-buddy reference kernel stops coalescing at the max order;
  now matched with a `>=` guard.
- Latent under the shipped config (BUDDY_MAX_ORDER=20 spans the whole
  heap window; order-20 buddies can never be adjacent), but any smaller
  max order trips it on the first full coalesce.

### F3. compaction: source page kept its LRU links after being freed to the buddy
`mm/compact.rs` — commit `a0afa687`.

- `try_to_unmap_migration` only decrements `_mapcount`; the source page
  kept `Lru` + `lru_next/lru_prev` from the fault path when
  `migrate_page_window` freed it. The LRU chain then ran through a
  buddy-freelist frame; after frame reuse (`clear_all_flags` drops `Lru`
  but not the links) a later `lru_add_page` of the same frame splices
  the stale chain into the new list (double-counted pfn; reclaim can then
  pick a live page as a cold candidate). The destination page, in turn,
  never entered an LRU (lost reclaim visibility).
- Fix mirrors the vmscan swap-out cleanup: `page_remove_lru(src)` +
  `page_add_anon_lru(dst)`.

## FILED (not fixed in this lane)

### FD-1. rmap/compact: Sv39-only page-table descent in shared mm code
`mm/rmap.rs::try_to_unmap_inner` (manual root->[vpn2]->[vpn1]->[vpn0]
descent with `>>30/>>21/>>12` shifts) and `mm/compact.rs::remap_page`
(same descent plus Sv39 PTE encoding `(ppn << 10) | flags | V`).

- On x86_64/aarch64 (48-bit VA, 4 levels) the root table is indexed by
  bits 47:39, but the code indexes it with bits 38:30 — the walk reads
  the wrong slot, `is_valid()` fails, and the unmap silently no-ops.
- Consequences (degradation, not corruption — the ppn re-validation
  under the PTE lock guards against wrong-leaf writes):
  - compaction migration always aborts on x86_64/aarch64
    (`alloc_pages` order>0 falls back without compaction);
  - anonymous swap-out (`ENABLE_SWAP=true` by default) reclaims nothing
    on those arches: `try_to_unmap_with_swap` returns 0, every candidate
    frees its slot and is skipped.
- riscv64 is unaffected (Sv39 3-level matches the shifts).
- Proposed fix (needs arch-side additions, hence out of this lane's
  mm/** scope): add one leaf-slot primitive per arch in
  `arch/*/mm/mm_ops.rs`, e.g.
  `unsafe fn leaf_pte_replace(root_ppn, vaddr, expect_ppn, new_bits) -> bool`
  plus `unsafe fn leaf_pte_install(root_ppn, vaddr, new_ppn, saved_low_bits) -> bool`
  (each arch extracts indexes/encodes its own PTE format), then delete
  the manual descents from rmap.rs and compact.rs. ~40 lines per arch.

### FD-2. minor observations (no action)
- `mm/vmemmap.rs:159` — dead `ConsoleWriter` block (created and dropped,
  no output); leftover debug.
- `mm/buddy_allocator.rs` — `heap_size_to_order` has no callers;
  `buddy_stats()` has a formatting glitch on its first line.
- `mm/page_alloc.rs::free_pages` checks zone membership only for the
  leader pfn of an order>0 block (trusted-caller assumption; the
  reference kernel's bad_page() checks are partially emulated by the
  OnFreelist/memblock/stamp tripwires).
- `mm/page_alloc.rs::alloc_page_batch` converts `out[..want]` pfn->phys
  even beyond `got`; callers must only read `got` entries.

## FALSE-POSITIVE RETRACTED (suspected during review, cleared by re-derivation)

- Zone `free_pages` counter on merged frees: buddy merge removes the
  buddy (already counted free) and adds the union; net free-page delta is
  exactly `1 << order` — the `fetch_add(1 << order)` at the end is
  correct (same shape as the page-buddy reference).
- `VmaManager::add` overlap/merge logic: prev-merge guard
  (`has_vma_starting_within`), successor chain-merge, and the fresh-insert
  overlap check (`range(start..=end)` + `start < end` test) re-derived
  against the vma_merge reference semantics — no swallow/overlap window.
- `find_stack_vma` case 2: only the FIRST VMA above the fault address is
  a growth candidate; an intervening non-growable VMA correctly yields
  None.
- `MmStruct::saved_auxv` terminator walk incl. full-vector/no-terminator
  edge (n lands on 44; `(n + 2).min(AT_VECTOR_SIZE)` includes the AT_NULL
  pair or clamps) — correct.
- `memblock::insert_and_merge_all` accounting (fold-to-fixed-point,
  compaction pass, `total_size` subtract/add, full-array rejection
  happens before any mutation) — correct; no double-count survives a
  bridging insert.
- swap entry encode/decode bit layout (`[9:8]` type, `[53:10]` offset,
  signature bit 62; migration marker bit 61) round-trips consistently;
  mkswap header geometry check `(pages + 1) * sectors_per_page >
  swap_sectors` is sound.
- small_alloc magazine: reclaim-blocked-by-construction argument holds
  (a magazine-resident slot keeps its page's `free_count` below
  `objs_per_page`, so an empty page can never be returned to the buddy
  while an entry is parked); push-time screening prevents bad pointers
  from parking; r8final len guards cover the divergence family.
- slab: `free()` validation chain (region find, cache ownership by
  object_size, header-region guard, alignment, index bounds,
  full-slab double-free guard) and the kmalloc cache_idx back-stamp —
  consistent; 4096-class correctly degenerates to the buddy
  (objects_per_slab == 0 -> create_slab None).
- `mm_users_dec`/`mm_count_dec` CAS-underflow guards, `alloc_asid`
  race (loser frees its speculative draw), `update_highest_vm_end`
  CAS-max, `sub_total_vm`/`sub_rss` saturating fetch_update — all sound.
- `nr_descs`/`pfn_valid`/`phys_valid`/vmemmap `nr_pages` triangle is
  consistent (single publisher `set_nr_descs` from init_vmemmap; all
  bounds share `nr_descs()`).
- `init_mem_map` reserved-span page math (floor/ceil clamps to the
  initialized span) and `mark_non_ram_pages` (whole-page-in-region RAM
  test, matching the reference's partial-page-reserved rule) — correct.
- vmscan swap-out slot-leak arms (write-fail / partial-unmap) are
  deliberate (freeing would UAF the swap-in path); refcount==1 gate
  prevents the fork-COW slot UAF (review MM-C3 already landed).
- kswapd deep-drain escalation and OOM strike table: bounded, episode-
  reset, no gate deadlock (IN_DIRECT_RECLAIM not waited on from kswapd).
- deferred-free pool ring arithmetic (`used = head - drained`, CAP
  rejection before write) — correct.
