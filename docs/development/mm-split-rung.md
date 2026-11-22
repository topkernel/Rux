# Crate-flip ladder — rung M1 measurement: mm policy/core split

Method: rung S1's (docs/development/sched-split-rung.md, the L4
template) — comment-stripped census + qualified cross-references +
raw-pointer triage. Measured 2026-11-22 on a7d1e5638 (+bb36770e5; the
mm tree is main-identical except a 3-line PageTableWalker::walk
leaf fix in x86 mm_ops). ONE PAGE, no flip yet. Zero QEMU — pure
code reading (census script: `run/m1_census.py`).

## The tree's real shape (four findings that reshape the rung)

1. **The §4 policy set is ALREADY SAFE AND TYPED at the file level.**
   The nine §4 policy files (vma, mm_struct, oom_kill, compact,
   kswapd, vmscan, swap, lru, hugepage) carry **zero unsafe blocks**
   and **five `*mut` sites total** — versus sched's 108-site storm.
   Three of the five are ABI/cast noise (kswapd's `*mut c_void`
   thread arg, an `as *mut _` spawn cast); the real two are ONE
   field (`oom_kill::Victim.chosen: Option<*mut Task>`) and ONE
   signature family (`compact_zone(zone: *mut Zone, ..)` + its
   struct field). The abort criterion ("policy reads raw pointers in
   more than a handful of sites that cannot ride a shared handle")
   does NOT fire — the two real sites are single-signature
   conversions (TaskRef / &Zone-under-guard), not a graph.

2. **The coupling lives in the MIXED arch files, not the policy
   files.** Like sched.rs before S1a, `mm_ops.rs` fuses policy
   (mmap placement, find_free_area, brk, allocate_stack,
   set_range_permissions, fork's COW mapping decisions,
   remove_vma_piece) with core (walk, leaf_pte_replace/install —
   the PTE atomics, unmap_pages/clear_pte, copy_page_table_cow,
   create_user_address_space, copy_kernel_mappings); and
   `page_fault.rs` fuses the §4 "fault ROUTING (which arm)" with the
   fill/copy frame primitives (fill_file_page,
   filemap_pin_cache_page, release_fill_page, handle_cow_fault).
   Per-arch mass: mm_ops+page_fault = 13,113 lines / 161 fns across
   x86_64+riscv64+aarch64 (x86 alone: 6,069/61). The x86 mm_ops
   single file holds 81 policy-side and 113 core-side qualified
   refs — this is where the seam gets cut, and it is per-arch.

3. **The §4 CORE set is policy-FREE.** buddy_allocator, zone,
   vmemmap, page_desc, memblock, pcp, page, pagetable (x3 arches),
   unsafe_edges (x3) carry **zero** qualified refs into policy
   modules. The nonzero core→policy direction comes entirely from
   core-ADJACENT files outside the §4 lists: rmap (21 — Vma-typed
   but PTE-coupled; genuinely straddles), page_alloc (12),
   mm/edges (10), mm/mod (6), gup (2), layout (3). Classification
   for those six: keep core-side (they are walk/allocator
   machinery that merely names Vma types) — their Vma references
   ride the safe type, not raw pointers.

4. **The design's named primitives are conceptual.** `FrameWindow`
   and `INV-PC-WIN` do not exist in the tree; the fill/copy frame
   role is played by the fill_file_page family + the COW copy paths
   + the INV-XE-ZP zero/copy edges (mm/edges.rs). The M1 design
   should adopt the as-built names.

## Item counts (fn / static / impl, comment-stripped)

§4 POLICY set (moves to `rux-mm`):

| file | lines | fn | static | impl | unsafe | *mut |
|---|---|---|---|---|---|---|
| vma | 1021 | 76 | 2 | 9 | 0 | 0 |
| mm_struct | 1488 | 97 | 2 | 5 | 0 | 0 |
| oom_kill | 586 | 9 | 3 | 1 | 0 | 1 |
| compact | 535 | 8 | 1 | 0 | 0 | 2 |
| kswapd | 290 | 5 | 6 | 0 | 0 | 2 |
| vmscan | 877 | 13 | 7 | 0 | 0 | 0 |
| swap | 582 | 29 | 12 | 1 | 0 | 0 |
| lru | 579 | 24 | 0 | 0 | 0 | 2 |
| hugepage | 307 | 16 | 1 | 1 | 0 | 0 |
| **sum** | **6265** | **277** | **34** | **17** | **0** | **5** |

§4 CORE set (stays):

| file(s) | lines | fn | static | impl | unsafe | *mut |
|---|---|---|---|---|---|---|
| buddy_allocator | 1729 | 45 | 8 | 5 | 9 | 15 |
| page_alloc | 1693 | 30 | 26 | 0 | 0 | 2 |
| zone | 1289 | 47 | 8 | 4 | 0 | 0 |
| page_desc | 1363 | 93 | 9 | 3 | 5 | 8 |
| memblock | 662 | 49 | 4 | 4 | 0 | 1 |
| pcp / page / vmemmap | 835 | 39 | 8 | 5 | 0 | 0 |
| mm/edges (registry) | 515 | 40 | 16 | 0 | 40 | 12 |
| pagetable x3 arch | 1047 | 103 | 0 | 17 | 2 | 0 |
| unsafe_edges x3 arch | 549 | 33 | 0 | 5 | 17 | 16 |
| uaccess x2 arch (bottoms) | 956 | 28 | 0 | 0 | 17 | 24 |
| **sum** | **10638** | **507** | **79** | **43** | **90** | **78** |

MIXED (the carve targets; fault routing + brk/mmap policy fused
with walks/PTE atomics):

| file | lines | fn | unsafe |
|---|---|---|---|
| x86 mm_ops | 3675 | 47 | 4 |
| x86 page_fault | 2394 | 14 | 28 |
| rv mm_ops / page_fault | 2726 / 1563 | 42 / 10 | 26 / 19 |
| aa mm_ops / page_fault | 2072 / 683 | 44 / 4 | 14 / 17 |
| (mmu_init x3, exception x3, asid x3: boot/core — stay) | 9062 | 260 | 137 |

Adjacent-but-unlisted (measured, parked): vdso (1495 ln, 61 fn,
**14 unsafe** — a service, not policy; needs its own edge cleanup
before any move), slab/small_alloc (allocator floor), gup, rmap,
deferred_free, layout, meminfo/vmstat/pagemap/pglist (safe, stats).

## Qualified cross-references (comments stripped)

policy → core (per §4 policy file): vmscan 18, compact 13, oom 9,
kswapd 8, vma 6, mm_struct 6, swap 6, hugepage 5, lru 3 — **sum 74**
(plus the parked adjacent files: lru-family and stats ≤23 each).
Compare S1: fair alone was 233.

core → policy: **§4 core set: 0.** Adjacent files only: rmap 21,
page_alloc 12, mm/edges 10, mm/mod 6, gup 2, layout 3 — sum 54.

mixed both ways (x86 / rv): mm_ops →policy 81/66, →core 113/81;
page_fault →policy 45/34, →core 39/21.

## Abort-criterion verdict

DOES NOT FIRE. The mm policy layer is typed end-to-end (Vma,
MmStruct, Zone queries, alloc_pages handles); the five `*mut` sites
are three ABI/cast noise + one victim field + one zone-param family.
No trait-signature redesign is required before the flip — unlike
S1's SchedClass `*mut Task` family.

## Verdict: M1 decomposes into M1a + M1b (same shape as S1)

- **M1a (the carve, the real cost):** split the per-arch mm_ops.rs
  and page_fault.rs along the policy/core line — policy half
  (mmap/munmap placement, find_free_area, brk, allocate_stack,
  mprotect/range-permission decisions, fork COW mapping decisions,
  VMA-piece removal, the fault ROUTING arms) vs core half (walk,
  PTE atomics leaf_pte_replace/install, unmap/clear primitives,
  copy_page_table_cow, address-space create/copy, fill_file_page
  family). 161 fns / 13,113 lines over three arches; the x86 pair
  (6,069 ln) is one S1a-class session, rv+aa are smaller echoes.
  The shared-policy halves across arches then MERGE into one
  arch-generic policy module — the per-arch residue is the seam
  (the map/unmap/walk primitive calls), which is what the design's
  "core traps → policy decides → core executes" flow wants anyway.
- **M1b (the crate flip):** the nine §4 policy files move with
  file-level `#![forbid(unsafe_code)]` from line one (they are
  already at zero unsafe — the flip is mechanical); the arch seam
  rides existing pub safe APIs (alloc_pages/free_pages/zone
  queries/walk/map_page family). Predicted reverse surface is
  SMALLER than S1b's 33-item bump list: only 7 `pub(crate)` items
  sit in the core+mixed files (2 generic-core, 5 x86) and the
  policy files' cross-refs are 74 qualified refs over 9 files —
  the composition-bin/hook-table recipe applies unchanged.

Predicted S1b-scale reverse surface for M1b: **~10-15 items**
(7 visibility bumps + the arch-seam hook registration + the two
real pointer conversions: oom victim handle, compact_zone Zone
param). The risk mass is not the flip — it is M1a's per-arch
carve and keeping the fault-path hot rows (pbench P3a/b/c, P6)
inside their perf-gate bands afterward.
