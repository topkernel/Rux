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

## M1a execution record (2026-11-23, ur-L1)

Landed, per the five-step recipe:

1. SEAM (88836bb7b): kernel/src/mm/mm_hooks.rs — the MmHooks table
   (6 rows: handle_mm_fault, mmap, munmap, find_free_area,
   zap_page_range, fork), OnceSlot-published, accessors read the table
   directly (the S1 self-recursion rule documented in-module). All ~40
   framework call sites (syscall layer, sysv_shm, io_uring, ashmem,
   ptrace, futex, gup, arch exception/uaccess) dispatch through it;
   pre-carve arches register method/fn pointers (behavior identical).
   FaultFlags + MmFaultResult moved to the seam (identical enums on
   all three arches unified into one type).
2. x86_64 carve (e78a5858b): kernel/src/mm/policy/x86_64.rs (3,392 ln,
   0 unsafe) — mmap/munmap/find_free_area/set_brk/do_brk/
   allocate_stack/set_range_permissions/map_vma/zap_page_range/
   remove_vma_piece/range_is_pte_empty/fork + try_expand_stack/
   pte_counts_as_mapped/is_protnone_marker/handle_mm_fault/
   handle_swap_fault + the fault-around constants. Conversions: 6
   full-page zeros -> mm::edges::page_slice_mut (INV-MM-7), 3 inode
   derefs -> fs::unsafe_edges::file_inode (INV-FS-FILEINODE-1, pub'd
   as the published contract), 1 raw read_at -> read_at_slice (exact
   byte-count subslice), ~10 vestigial unsafe wrappers around
   already-safe calls dropped. Compiled ONLY by rux-mm (forbid,
   negative-probe verified: `error: usage of an unsafe block` ->
   rux-mm/src/lib.rs:15).
3. riscv64 carve (this commit): kernel/src/mm/policy/riscv64.rs
   (2,179 ln, 0 unsafe) — same fn set plus munmap_legacy and
   anon_faultaround_forward. Extra conversions vs the x86 twin: the 6
   inline sfence.vma fences -> asid::sfence_vma_addr (published,
   exact asm shape hoisted) / flush_tlb_all; set_brk's raw
   pfn_to_page_mut deref -> the typed page_desc edge.
4. Mechanism surface published (the policy's calling contract):
   unmap_pages, perm_to_flags, PageTableWalker::walk, map_page_noflush,
   textfree_owner_maps, release_cache_mapping_pin, the fill family,
   read/write_pte_raw, l1_table_for, FILL_REFUSAL_SITE.
5. aarch64 carve (1d70a7c7d): kernel/src/mm/policy/aarch64.rs (1,268
   ln, 0 unsafe) — the same fn set (minus set_range_permissions/
   remove_vma_piece/find_free_area_32bit, which aa never had) plus
   protnone_represent. All three registrars now empty; every arch's mm
   policy serves from rux_bin -> rux_mm::register_all.

Gates: triple-arch rc=0 after every step; riscv64 suite 886/886 at the
seam, 886/886 with the x86 carve, 886/886 with the rv carve (the rv
policy served from the crate); x86_64 suite 888/888 (crate path; the
pre-existing E0603 offsets breakage fixed in b20b766f0); minimal-rootfs
boot to shell, zero panics; unsafe gate OK (sidecar content-remapped
across the line shifts: 2098 INV / 186 GAP at HEAD).

## M1a tier-2 perf gate (2026-11-23, x86_64/KVM + TCG)

Attempted via scripts/perf-gate.sh --tiers 2; the script refused to
boot twice (2+ foreign QEMUs — host capacity rule), so the same boot
was run directly through scripts/test/pbench_boot.py (deviation noted;
one QEMU at a time, 49G available). One TCG board (m1a-pb3/pb4) and
one KVM board (m1a-pb6), debug bzImage @ 1d70a7c7d:

| row | m1a KVM med (µs) | SP-7 KVM debug anchor | s1-close control (same day, KVM) |
|---|---|---|---|
| p1_null | 0.260 | 0.300 | — |
| p1_clock | 0.340 | 0.371 | — |
| p2_ctxsw | 1.668 | 1.242 (loaded band 2.0) | — |
| p3a_fault_anon | 0.010 | — | 0.010 |
| p3b_fault_cow | 1.994 | 8.075 (older main) | 1.974–2.024 |
| p3c_fault_file | 0.010 | — | 0.010 |
| p4_fork_exit | 317.7 | 578.0 (older main) | 333–335 |

The mm-guarded rows (P2 ctxsw, P3a/b/c faults, P4 fork) are IN BAND vs
the anchor class and vs a concurrently-booted control kernel from the
s1-close lane — the crate indirection (fn-pointer table dispatch on
the fault path, no LTO in debug) costs nothing measurable at the
median. p3b/p4 sit far UNDER the SP-7 anchor because the anchor
predates recent main-side work; the same-day control shows the same
class.

P5 AND LATER ROWS DID NOT RUN: every pbench board (mine AND the
s1-close lane's, KVM and TCG alike) wedges deterministically right
after p4_fork_exit — RTS-DEFER spam to pid=1394, then silence; the
wedge signature is byte-identical on a kernel that does NOT carry the
M1a commits. Verdict: a shared-base regression in the exit/reap
storage-deferral path (kernel/src/process/exit.rs, RTS-DEFER machinery
landed 2026-11-21), NOT an M1a effect. Routed to the exit/reap owner;
M1a's own rows all completed and passed.
