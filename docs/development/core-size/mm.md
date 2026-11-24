# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# mm — hot 16424 lines (stay) / cold 6761 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| mm/buddy_allocator.rs | `refused_free_note` | 127 | 27 | yes |
| mm/buddy_allocator.rs | `new` | 153 | 23 | yes |
| mm/buddy_allocator.rs | `new` | 175 | 8 | yes |
| mm/buddy_allocator.rs | `get` | 182 | 8 | yes |
| mm/buddy_allocator.rs | `get_mut` | 189 | 38 | yes |
| mm/buddy_allocator.rs | `heap_bounds` | 226 | 16 | yes |
| mm/buddy_allocator.rs | `buddy_block_span` | 241 | 23 | yes |
| mm/buddy_allocator.rs | `new` | 263 | 15 | yes |
| mm/buddy_allocator.rs | `check_magic` | 277 | 9 | yes |
| mm/buddy_allocator.rs | `init` | 285 | 15 | yes |
| mm/buddy_allocator.rs | `init_sized` | 299 | 46 | yes |
| mm/buddy_allocator.rs | `page_is_free` | 344 | 10 | yes |
| mm/buddy_allocator.rs | `fill_free_bits` | 353 | 39 | yes |
| mm/buddy_allocator.rs | `fill_interior_order` | 391 | 29 | yes |
| mm/buddy_allocator.rs | `init_block` | 419 | 19 | yes |
| mm/buddy_allocator.rs | `add_to_free_list` | 437 | 38 | yes |
| mm/buddy_allocator.rs | `remove_from_free_list` | 474 | 34 | yes |
| mm/buddy_allocator.rs | `heap_size_to_order` | 507 | 22 | yes |
| mm/buddy_allocator.rs | `size_to_order` | 528 | 10 | yes |
| mm/buddy_allocator.rs | `get_buddy_idx` | 537 | 7 | yes |
| mm/buddy_allocator.rs | `page_idx_to_addr` | 543 | 8 | yes |
| mm/buddy_allocator.rs | `addr_to_page_idx_checked` | 550 | 15 | yes |
| mm/buddy_allocator.rs | `alloc_blocks` | 564 | 85 | yes |
| mm/buddy_allocator.rs | `free_blocks` | 648 | 90 | yes |
| mm/buddy_allocator.rs | `alloc` | 737 | 107 | yes |
| mm/buddy_allocator.rs | `realloc` | 843 | 32 | yes |
| mm/buddy_allocator.rs | `dealloc` | 874 | 34 | yes |
| mm/buddy_allocator.rs | `trace_kmalloc_at` | 907 | 25 | yes |
| mm/buddy_allocator.rs | `trace_kfree_at` | 931 | 17 | yes |
| mm/buddy_allocator.rs | `dealloc_untracked_pub` | 947 | 11 | yes |
| mm/buddy_allocator.rs | `alloc_page_raw` | 957 | 21 | yes |
| mm/buddy_allocator.rs | `free_page_raw` | 977 | 27 | yes |
| mm/buddy_allocator.rs | `dealloc_untracked` | 1003 | 100 | yes |
| mm/buddy_allocator.rs | `audit_lock_held` | 1102 | 18 | yes |
| mm/buddy_allocator.rs | `init_heap` | 1119 | 8 | yes |
| mm/buddy_allocator.rs | `init_heap_sized` | 1126 | 39 | yes |
| mm/buddy_allocator.rs | `buddy_free_census_lockfree` | 1164 | 28 | yes |
| mm/buddy_allocator.rs | `buddy_stats` | 1191 | 131 | yes |
| mm/buddy_allocator.rs | `record` | 1321 | 20 | yes |
| mm/buddy_allocator.rs | `meta_audit_inner` | 1340 | 220 | yes |
| mm/buddy_allocator.rs | `buddy_meta_shadow_scan` | 1559 | 32 | yes |
| mm/buddy_allocator.rs | `buddy_meta_selftest` | 1590 | 65 | yes |
| mm/buddy_allocator.rs | `dealloc_direct` | 1654 | 29 | yes |
| mm/buddy_allocator.rs | `alloc` | 1682 | 30 | yes |
| mm/buddy_allocator.rs | `dealloc` | 1711 | 19 | yes |
| mm/deferred_free.rs | `register_deferred_size` | 91 | 22 |  |
| mm/deferred_free.rs | `deferred_size` | 112 | 10 |  |
| mm/deferred_free.rs | `try_defer` | 121 | 57 | yes |
| mm/deferred_free.rs | `drain_if_due` | 177 | 14 | yes |
| mm/deferred_free.rs | `drain` | 190 | 54 | yes |
| mm/edges.rs | `first_node_mut` | 37 | 10 | yes |
| mm/edges.rs | `first_node` | 46 | 9 | yes |
| mm/edges.rs | `page_of` | 54 | 15 | yes |
| mm/edges.rs | `page_ref_of` | 68 | 22 | yes |
| mm/edges.rs | `memblock_ref` | 89 | 10 | yes |
| mm/edges.rs | `memblock_mut` | 98 | 10 | yes |
| mm/edges.rs | `walk_fp_chain` | 107 | 18 | yes |
| mm/edges.rs | `slab_read_freelist_link` | 124 | 9 | yes |
| mm/edges.rs | `slab_write_freelist_link` | 132 | 10 | yes |
| mm/edges.rs | `slab_header_mut` | 141 | 10 | yes |
| mm/edges.rs | `slab_header_cache_idx` | 150 | 9 | yes |
| mm/edges.rs | `slab_pages_init` | 158 | 12 | yes |
| mm/edges.rs | `slab_pages` | 169 | 9 | yes |
| mm/edges.rs | `slab_zero` | 177 | 33 | yes |
| mm/edges.rs | `pcp_row_exclusive` | 209 | 14 | yes |
| mm/edges.rs | `pcp_row_count_peek` | 222 | 10 | yes |
| mm/edges.rs | `pcp_row_ref_peek` | 231 | 13 | yes |
| mm/edges.rs | `copy_kva` | 243 | 10 | yes |
| mm/edges.rs | `copy_kva_from` | 252 | 15 | yes |
| mm/edges.rs | `page_slice_mut` | 266 | 9 | yes |
| mm/edges.rs | `page_slice` | 274 | 15 | yes |
| mm/edges.rs | `defer_pool_mut` | 288 | 12 | yes |
| mm/edges.rs | `defer_dealloc` | 299 | 21 | yes |
| mm/edges.rs | `vmemmap_stats_read` | 319 | 9 | yes |
| mm/edges.rs | `vmemmap_stats_write` | 327 | 10 | yes |
| mm/edges.rs | `vmemmap_zero_region` | 336 | 12 | yes |
| mm/edges.rs | `vmemmap_map_kernel_page` | 347 | 14 | yes |
| mm/edges.rs | `switch_address_space` | 360 | 28 | yes |
| mm/edges.rs | `compact_copy_page` | 387 | 14 | yes |
| mm/edges.rs | `kernel_layout_write` | 400 | 10 | yes |
| mm/edges.rs | `kernel_layout_ref` | 409 | 13 | yes |
| mm/edges.rs | `node_data_init` | 421 | 19 | yes |
| mm/edges.rs | `x86_caller_return_address` | 439 | 20 | yes |
| mm/edges.rs | `sa_hdr_mut` | 458 | 10 | yes |
| mm/edges.rs | `sa_hdr_ref` | 467 | 10 | yes |
| mm/edges.rs | `sa_read_u16` | 476 | 9 | yes |
| mm/edges.rs | `sa_write_u16` | 484 | 9 | yes |
| mm/edges.rs | `sa_read_u64` | 492 | 5 | yes |
| mm/gup.rs | `va` | 69 | 6 |  |
| mm/gup.rs | `len` | 74 | 6 | yes |
| mm/gup.rs | `head_off` | 79 | 6 |  |
| mm/gup.rs | `page_of` | 84 | 8 | yes |
| mm/gup.rs | `piece` | 91 | 19 |  |
| mm/gup.rs | `for_each_piece` | 109 | 32 | yes |
| mm/gup.rs | `range_pinned` | 140 | 18 | yes |
| mm/gup.rs | `next_piece` | 157 | 41 | yes |
| mm/gup.rs | `page_end_off` | 197 | 9 |  |
| mm/gup.rs | `first_ptr` | 205 | 10 | yes |
| mm/gup.rs | `copy_into_pinned` | 214 | 17 |  |
| mm/gup.rs | `copy_from_pinned` | 230 | 15 |  |
| mm/gup.rs | `pinned_pages` | 244 | 6 | yes |
| mm/gup.rs | `page_count` | 249 | 6 | yes |
| mm/gup.rs | `unpin` | 254 | 16 |  |
| mm/gup.rs | `drop` | 269 | 18 | yes |
| mm/gup.rs | `pin_user_pages` | 286 | 86 | yes |
| mm/gup.rs | `pin_one_faulted` | 371 | 63 |  |
| mm/gup.rs | `handle_mm_fault_checked` | 433 | 8 |  |
| mm/layout.rs | `heap_region_end` | 90 | 10 |  |
| mm/layout.rs | `effective_static_regions` | 99 | 102 |  |
| mm/layout.rs | `new` | 200 | 28 | yes |
| mm/layout.rs | `init_from_memblock` | 227 | 75 |  |
| mm/layout.rs | `kernel_layout_init` | 301 | 12 |  |
| mm/layout.rs | `kernel_layout` | 312 | 11 |  |
| mm/layout.rs | `is_kernel_layout_initialized` | 322 | 9 |  |
| mm/layout.rs | `phys_memory_base` | 330 | 7 |  |
| mm/layout.rs | `phys_memory_size` | 336 | 7 | yes |
| mm/layout.rs | `kernel_start` | 342 | 7 |  |
| mm/layout.rs | `kernel_end` | 348 | 7 |  |
| mm/layout.rs | `heap_start` | 354 | 7 |  |
| mm/layout.rs | `heap_size` | 360 | 7 |  |
| mm/layout.rs | `slab_start` | 366 | 7 |  |
| mm/layout.rs | `slab_size` | 372 | 7 |  |
| mm/layout.rs | `user_phys_start` | 378 | 7 |  |
| mm/layout.rs | `user_phys_size` | 384 | 7 |  |
| mm/layout.rs | `frame_alloc_start` | 390 | 7 |  |
| mm/layout.rs | `frame_alloc_size` | 396 | 7 |  |
| mm/layout.rs | `heap_end` | 402 | 7 |  |
| mm/layout.rs | `slab_end` | 408 | 7 |  |
| mm/layout.rs | `user_phys_end` | 414 | 18 |  |
| mm/layout.rs | `assert_memblock_consistency` | 431 | 47 |  |
| mm/memblock.rs | `new` | 65 | 10 | yes |
| mm/memblock.rs | `with_flags` | 74 | 12 |  |
| mm/memblock.rs | `end` | 85 | 7 | yes |
| mm/memblock.rs | `contains` | 91 | 7 | yes |
| mm/memblock.rs | `base_pfn` | 97 | 7 |  |
| mm/memblock.rs | `end_pfn` | 103 | 7 | yes |
| mm/memblock.rs | `page_count` | 109 | 18 | yes |
| mm/memblock.rs | `new` | 126 | 18 | yes |
| mm/memblock.rs | `insert_and_merge_all` | 143 | 79 |  |
| mm/memblock.rs | `add` | 221 | 14 | yes |
| mm/memblock.rs | `add_reserved` | 234 | 6 |  |
| mm/memblock.rs | `remove` | 239 | 16 | yes |
| mm/memblock.rs | `iter` | 254 | 7 | yes |
| mm/memblock.rs | `len` | 260 | 7 | yes |
| mm/memblock.rs | `is_empty` | 266 | 7 | yes |
| mm/memblock.rs | `total_size` | 272 | 6 |  |
| mm/memblock.rs | `find` | 277 | 6 | yes |
| mm/memblock.rs | `find_first_available` | 282 | 23 |  |
| mm/memblock.rs | `new` | 304 | 13 | yes |
| mm/memblock.rs | `init` | 316 | 9 | yes |
| mm/memblock.rs | `add_memory` | 324 | 13 |  |
| mm/memblock.rs | `reserve` | 336 | 13 | yes |
| mm/memblock.rs | `reserve_nomap` | 348 | 7 |  |
| mm/memblock.rs | `get_available_region` | 354 | 32 |  |
| mm/memblock.rs | `total_memory` | 385 | 7 |  |
| mm/memblock.rs | `total_reserved` | 391 | 6 |  |
| mm/memblock.rs | `available_memory` | 396 | 7 |  |
| mm/memblock.rs | `memory` | 402 | 7 |  |
| mm/memblock.rs | `reserved` | 408 | 6 | yes |
| mm/memblock.rs | `is_reserved` | 413 | 6 | yes |
| mm/memblock.rs | `is_memory` | 418 | 6 |  |
| mm/memblock.rs | `find_in_range` | 423 | 35 |  |
| mm/memblock.rs | `dump` | 457 | 19 | yes |
| mm/memblock.rs | `for_each_free_range` | 475 | 62 |  |
| mm/memblock.rs | `memblock_raw` | 536 | 9 | yes |
| mm/memblock.rs | `memblock_init` | 544 | 12 |  |
| mm/memblock.rs | `memblock_add` | 555 | 7 |  |
| mm/memblock.rs | `memblock_reserve` | 561 | 7 |  |
| mm/memblock.rs | `memblock_reserve_nomap` | 567 | 7 |  |
| mm/memblock.rs | `memblock_get_available_region` | 573 | 8 |  |
| mm/memblock.rs | `memblock_total_memory` | 580 | 7 |  |
| mm/memblock.rs | `memblock_available_memory` | 586 | 7 | yes |
| mm/memblock.rs | `memblock_is_reserved` | 592 | 7 |  |
| mm/memblock.rs | `memblock_for_each_free_range` | 598 | 10 | yes |
| mm/memblock.rs | `memblock_find_in_range` | 607 | 7 |  |
| mm/memblock.rs | `memblock_dump` | 613 | 12 |  |
| mm/memblock.rs | `memblock` | 624 | 12 | yes |
| mm/memblock.rs | `memblock_mut` | 635 | 9 | yes |
| mm/memblock.rs | `memblock_phys_alloc` | 643 | 21 | yes |
| mm/meminfo.rs | `default` | 78 | 27 | yes |
| mm/meminfo.rs | `format` | 104 | 12 |  |
| mm/meminfo.rs | `fmt` | 115 | 35 |  |
| mm/meminfo.rs | `get_basic_memory_info` | 149 | 28 | yes |
| mm/meminfo.rs | `get_memory_info` | 176 | 34 |  |
| mm/meminfo.rs | `print_memory_info` | 209 | 20 |  |
| mm/meminfo.rs | `get_memory_summary` | 228 | 23 |  |
| mm/meminfo.rs | `is_memory_low` | 250 | 13 |  |
| mm/meminfo.rs | `should_trigger_oom` | 262 | 11 |  |
| mm/mm_hooks.rs | `no_policy_handle_mm_fault` | 117 | 4 |  |
| mm/mm_hooks.rs | `no_policy_mmap` | 120 | 12 |  |
| mm/mm_hooks.rs | `no_policy_munmap` | 131 | 4 |  |
| mm/mm_hooks.rs | `no_policy_find_free_area` | 134 | 4 |  |
| mm/mm_hooks.rs | `no_policy_zap_page_range` | 137 | 4 |  |
| mm/mm_hooks.rs | `no_policy_fork` | 140 | 12 |  |
| mm/mm_hooks.rs | `mm_register_hooks` | 151 | 6 | yes |
| mm/mm_hooks.rs | `hooks` | 156 | 8 |  |
| mm/mm_hooks.rs | `handle_mm_fault` | 163 | 6 | yes |
| mm/mm_hooks.rs | `mmap` | 168 | 14 | yes |
| mm/mm_hooks.rs | `munmap` | 181 | 6 | yes |
| mm/mm_hooks.rs | `find_free_area` | 186 | 6 | yes |
| mm/mm_hooks.rs | `zap_page_range` | 191 | 10 | yes |
| mm/mm_hooks.rs | `fork` | 200 | 37 | yes |
| mm/mm_hooks.rs | `make_swap_entry` | 236 | 9 | yes |
| mm/mm_hooks.rs | `is_swap_entry` | 244 | 7 |  |
| mm/mm_hooks.rs | `make_migration_entry` | 250 | 7 |  |
| mm/mm_hooks.rs | `is_migration_entry` | 256 | 7 |  |
| mm/mm_hooks.rs | `swap_entry_type` | 262 | 7 |  |
| mm/mm_hooks.rs | `swap_entry_offset` | 268 | 106 |  |
| mm/mm_hooks.rs | `no_policy_kswapd_init` | 373 | 4 |  |
| mm/mm_hooks.rs | `no_policy_kswapd_enable_oom` | 376 | 4 |  |
| mm/mm_hooks.rs | `no_policy_wakeup_kswapd` | 379 | 4 |  |
| mm/mm_hooks.rs | `no_policy_try_to_free_pages` | 382 | 4 |  |
| mm/mm_hooks.rs | `no_policy_drain_file_cache_before_oom` | 385 | 4 |  |
| mm/mm_hooks.rs | `no_policy_vscan_heartbeat` | 388 | 4 |  |
| mm/mm_hooks.rs | `no_policy_vscan_census_line` | 391 | 4 |  |
| mm/mm_hooks.rs | `no_policy_zone_dirty_writeback_state` | 394 | 4 |  |
| mm/mm_hooks.rs | `no_policy_zone_below_dirty_writeback_mark` | 397 | 4 |  |
| mm/mm_hooks.rs | `no_policy_fault_oom_kill_or_retry` | 400 | 4 |  |
| mm/mm_hooks.rs | `no_policy_kernel_fault_oom_kill` | 403 | 4 |  |
| mm/mm_hooks.rs | `no_policy_sysrq_oom_kill` | 406 | 4 |  |
| mm/mm_hooks.rs | `no_policy_oom_badness` | 409 | 4 |  |
| mm/mm_hooks.rs | `no_policy_migration_in_progress` | 412 | 4 |  |
| mm/mm_hooks.rs | `no_policy_compact_zone` | 415 | 4 |  |
| mm/mm_hooks.rs | `no_policy_swap_init` | 418 | 4 |  |
| mm/mm_hooks.rs | `no_policy_swap_activate` | 421 | 4 |  |
| mm/mm_hooks.rs | `no_policy_swap_deactivate` | 424 | 4 |  |
| mm/mm_hooks.rs | `no_policy_swap_boot_disk` | 427 | 4 |  |
| mm/mm_hooks.rs | `no_policy_nr_active_swap` | 430 | 4 |  |
| mm/mm_hooks.rs | `no_policy_swap_free_slot` | 433 | 4 |  |
| mm/mm_hooks.rs | `no_policy_swap_stats` | 436 | 4 |  |
| mm/mm_hooks.rs | `no_policy_swap_write_census` | 439 | 4 |  |
| mm/mm_hooks.rs | `no_policy_page_add_anon_lru` | 442 | 4 |  |
| mm/mm_hooks.rs | `no_policy_page_remove_lru` | 445 | 4 |  |
| mm/mm_hooks.rs | `no_policy_page_add_file_lru` | 448 | 4 |  |
| mm/mm_hooks.rs | `no_policy_page_add_file_lru_young` | 451 | 4 |  |
| mm/mm_hooks.rs | `no_policy_page_add_file_lru_young_batch` | 454 | 4 |  |
| mm/mm_hooks.rs | `no_policy_lru_activate_young` | 457 | 4 |  |
| mm/mm_hooks.rs | `no_policy_lru_tail` | 460 | 4 |  |
| mm/mm_hooks.rs | `no_policy_lru_demote_active_file_oldest` | 463 | 10 |  |
| mm/mm_hooks.rs | `mm_register_policy_hooks` | 472 | 6 | yes |
| mm/mm_hooks.rs | `policy_hooks` | 477 | 10 |  |
| mm/mm_hooks.rs | `register_in_tree_mm_policy` | 486 | 46 |  |
| mm/mm_hooks.rs | `kswapd_init` | 531 | 5 |  |
| mm/mm_hooks.rs | `kswapd_enable_oom` | 535 | 5 |  |
| mm/mm_hooks.rs | `wakeup_kswapd` | 539 | 5 | yes |
| mm/mm_hooks.rs | `try_to_free_pages` | 543 | 5 | yes |
| mm/mm_hooks.rs | `drain_file_cache_before_oom` | 547 | 5 | yes |
| mm/mm_hooks.rs | `vscan_heartbeat` | 551 | 5 | yes |
| mm/mm_hooks.rs | `vscan_census_line` | 555 | 5 | yes |
| mm/mm_hooks.rs | `zone_dirty_writeback_state` | 559 | 5 | yes |
| mm/mm_hooks.rs | `zone_below_dirty_writeback_mark` | 563 | 5 | yes |
| mm/mm_hooks.rs | `fault_oom_kill_or_retry` | 567 | 5 | yes |
| mm/mm_hooks.rs | `kernel_fault_oom_kill` | 571 | 5 | yes |
| mm/mm_hooks.rs | `sysrq_oom_kill` | 575 | 5 | yes |
| mm/mm_hooks.rs | `oom_badness` | 579 | 5 | yes |
| mm/mm_hooks.rs | `migration_in_progress` | 583 | 5 | yes |
| mm/mm_hooks.rs | `compact_zone` | 587 | 5 | yes |
| mm/mm_hooks.rs | `swap_init` | 591 | 5 | yes |
| mm/mm_hooks.rs | `swap_activate` | 595 | 5 | yes |
| mm/mm_hooks.rs | `swap_deactivate` | 599 | 5 | yes |
| mm/mm_hooks.rs | `swap_boot_disk` | 603 | 5 | yes |
| mm/mm_hooks.rs | `nr_active_swap` | 607 | 5 | yes |
| mm/mm_hooks.rs | `swap_free_slot` | 611 | 5 | yes |
| mm/mm_hooks.rs | `swap_stats` | 615 | 5 | yes |
| mm/mm_hooks.rs | `swap_write_census` | 619 | 5 | yes |
| mm/mm_hooks.rs | `page_add_anon_lru` | 623 | 5 | yes |
| mm/mm_hooks.rs | `page_remove_lru` | 627 | 5 | yes |
| mm/mm_hooks.rs | `page_add_file_lru` | 631 | 5 | yes |
| mm/mm_hooks.rs | `page_add_file_lru_young` | 635 | 5 | yes |
| mm/mm_hooks.rs | `page_add_file_lru_young_batch` | 639 | 5 | yes |
| mm/mm_hooks.rs | `lru_activate_young` | 643 | 5 | yes |
| mm/mm_hooks.rs | `lru_tail` | 647 | 5 | yes |
| mm/mm_hooks.rs | `lru_demote_active_file_oldest` | 651 | 4 | yes |
| mm/mm_struct.rs | `new` | 70 | 7 | yes |
| mm/mm_struct.rs | `lock` | 76 | 7 | yes |
| mm/mm_struct.rs | `lock_irqsave` | 82 | 15 | yes |
| mm/mm_struct.rs | `new` | 96 | 18 | yes |
| mm/mm_struct.rs | `lock` | 113 | 10 | yes |
| mm/mm_struct.rs | `lock_irqsave` | 122 | 261 | yes |
| mm/mm_struct.rs | `new` | 382 | 90 | yes |
| mm/mm_struct.rs | `new_shared` | 471 | 11 |  |
| mm/mm_struct.rs | `new_with_type` | 481 | 6 |  |
| mm/mm_struct.rs | `new_kernel` | 486 | 6 |  |
| mm/mm_struct.rs | `new_user` | 491 | 11 |  |
| mm/mm_struct.rs | `set_exe_text_key` | 501 | 8 | yes |
| mm/mm_struct.rs | `inherit_exe_text` | 508 | 12 | yes |
| mm/mm_struct.rs | `pgd` | 519 | 7 | yes |
| mm/mm_struct.rs | `root_ppn` | 525 | 7 | yes |
| mm/mm_struct.rs | `asid` | 531 | 7 | yes |
| mm/mm_struct.rs | `set_asid` | 537 | 10 |  |
| mm/mm_struct.rs | `note_tlb_cpu` | 546 | 25 | yes |
| mm/mm_struct.rs | `clear_tlb_cpu` | 570 | 12 |  |
| mm/mm_struct.rs | `tlb_cpu_mask` | 581 | 13 |  |
| mm/mm_struct.rs | `alloc_asid` | 593 | 26 | yes |
| mm/mm_struct.rs | `free_asid` | 618 | 19 |  |
| mm/mm_struct.rs | `pte_lock` | 636 | 23 |  |
| mm/mm_struct.rs | `mm_surgery_guard` | 658 | 9 | yes |
| mm/mm_struct.rs | `space_type` | 666 | 9 |  |
| mm/mm_struct.rs | `start_code` | 674 | 7 |  |
| mm/mm_struct.rs | `set_start_code` | 680 | 7 |  |
| mm/mm_struct.rs | `end_code` | 686 | 7 |  |
| mm/mm_struct.rs | `set_end_code` | 692 | 7 |  |
| mm/mm_struct.rs | `start_data` | 698 | 7 |  |
| mm/mm_struct.rs | `set_start_data` | 704 | 7 |  |
| mm/mm_struct.rs | `end_data` | 710 | 7 |  |
| mm/mm_struct.rs | `set_end_data` | 716 | 9 |  |
| mm/mm_struct.rs | `start_brk` | 724 | 7 | yes |
| mm/mm_struct.rs | `set_start_brk` | 730 | 14 | yes |
| mm/mm_struct.rs | `brk` | 743 | 7 | yes |
| mm/mm_struct.rs | `set_brk_val` | 749 | 10 | yes |
| mm/mm_struct.rs | `brk_lock` | 758 | 9 | yes |
| mm/mm_struct.rs | `start_stack` | 766 | 7 | yes |
| mm/mm_struct.rs | `set_start_stack` | 772 | 7 | yes |
| mm/mm_struct.rs | `stack_limit` | 778 | 7 |  |
| mm/mm_struct.rs | `set_stack_limit` | 784 | 9 | yes |
| mm/mm_struct.rs | `arg_start` | 792 | 7 | yes |
| mm/mm_struct.rs | `set_arg_start` | 798 | 7 |  |
| mm/mm_struct.rs | `arg_end` | 804 | 7 | yes |
| mm/mm_struct.rs | `set_arg_end` | 810 | 7 |  |
| mm/mm_struct.rs | `env_start` | 816 | 7 | yes |
| mm/mm_struct.rs | `set_env_start` | 822 | 7 |  |
| mm/mm_struct.rs | `env_end` | 828 | 7 | yes |
| mm/mm_struct.rs | `set_env_end` | 834 | 9 |  |
| mm/mm_struct.rs | `total_vm` | 842 | 7 | yes |
| mm/mm_struct.rs | `add_total_vm` | 848 | 12 | yes |
| mm/mm_struct.rs | `sub_total_vm` | 859 | 20 |  |
| mm/mm_struct.rs | `release_unmapped_pages` | 878 | 18 |  |
| mm/mm_struct.rs | `locked_vm` | 895 | 7 |  |
| mm/mm_struct.rs | `pinned_vm` | 901 | 7 |  |
| mm/mm_struct.rs | `data_vm` | 907 | 7 |  |
| mm/mm_struct.rs | `exec_vm` | 913 | 7 |  |
| mm/mm_struct.rs | `stack_vm` | 919 | 9 |  |
| mm/mm_struct.rs | `rss` | 927 | 7 | yes |
| mm/mm_struct.rs | `add_rss` | 933 | 8 | yes |
| mm/mm_struct.rs | `sub_rss` | 940 | 13 |  |
| mm/mm_struct.rs | `mmap_base` | 952 | 7 |  |
| mm/mm_struct.rs | `set_mmap_base` | 958 | 7 |  |
| mm/mm_struct.rs | `highest_vm_end` | 964 | 10 |  |
| mm/mm_struct.rs | `update_highest_vm_end` | 973 | 21 |  |
| mm/mm_struct.rs | `mm_users_inc` | 993 | 13 | yes |
| mm/mm_struct.rs | `mm_users_dec` | 1005 | 22 |  |
| mm/mm_struct.rs | `mm_users_dec_and_test` | 1026 | 7 |  |
| mm/mm_struct.rs | `mm_users` | 1032 | 7 |  |
| mm/mm_struct.rs | `mm_count_inc` | 1038 | 9 |  |
| mm/mm_struct.rs | `mm_count_dec` | 1046 | 22 |  |
| mm/mm_struct.rs | `mm_count_dec_and_test` | 1067 | 7 |  |
| mm/mm_struct.rs | `mm_count` | 1073 | 9 |  |
| mm/mm_struct.rs | `flags` | 1081 | 7 | yes |
| mm/mm_struct.rs | `set_flags` | 1087 | 7 | yes |
| mm/mm_struct.rs | `has_flag` | 1093 | 9 | yes |
| mm/mm_struct.rs | `owner_pid` | 1101 | 7 |  |
| mm/mm_struct.rs | `set_owner_pid` | 1107 | 12 |  |
| mm/mm_struct.rs | `vma_read` | 1118 | 11 | yes |
| mm/mm_struct.rs | `vma_try_read` | 1128 | 11 |  |
| mm/mm_struct.rs | `vma_rwsem_diag` | 1138 | 8 |  |
| mm/mm_struct.rs | `pin_vma_file` | 1145 | 6 | yes |
| mm/mm_struct.rs | `unpin_vma_file` | 1150 | 7 | yes |
| mm/mm_struct.rs | `get_vma_file` | 1156 | 8 | yes |
| mm/mm_struct.rs | `try_get_vma_file` | 1163 | 14 |  |
| mm/mm_struct.rs | `vma_write` | 1176 | 6 | yes |
| mm/mm_struct.rs | `find_vma` | 1181 | 7 | yes |
| mm/mm_struct.rs | `add_vma` | 1187 | 17 | yes |
| mm/mm_struct.rs | `remove_vma` | 1203 | 20 |  |
| mm/mm_struct.rs | `setup_segment_layout` | 1222 | 29 |  |
| mm/mm_struct.rs | `setup_stack` | 1250 | 13 |  |
| mm/mm_struct.rs | `setup_argv` | 1262 | 7 | yes |
| mm/mm_struct.rs | `setup_envp` | 1268 | 10 | yes |
| mm/mm_struct.rs | `set_saved_auxv` | 1277 | 10 | yes |
| mm/mm_struct.rs | `saved_auxv` | 1286 | 23 | yes |
| mm/mm_struct.rs | `drop` | 1308 | 180 | yes |
| mm/mod.rs | `alloc_and_map_user_table` | 118 | 21 | yes |
| mm/mod.rs | `alloc_and_map_user_memory` | 138 | 30 |  |
| mm/mod.rs | `switch_address_space` | 167 | 13 | yes |
| mm/mod.rs | `flush_tlb_page_local` | 179 | 63 | yes |
| mm/page.rs | `new` | 30 | 5 | yes |
| mm/page.rs | `as_usize` | 34 | 5 | yes |
| mm/page.rs | `is_aligned` | 38 | 5 |  |
| mm/page.rs | `floor` | 42 | 5 |  |
| mm/page.rs | `ceil` | 46 | 5 |  |
| mm/page.rs | `frame_number` | 50 | 6 | yes |
| mm/page.rs | `ppn` | 55 | 7 |  |
| mm/page.rs | `new` | 61 | 5 | yes |
| mm/page.rs | `as_usize` | 65 | 5 | yes |
| mm/page.rs | `is_aligned` | 69 | 5 |  |
| mm/page.rs | `floor` | 73 | 5 |  |
| mm/page.rs | `ceil` | 77 | 5 |  |
| mm/page.rs | `page_number` | 81 | 12 |  |
| mm/page.rs | `new` | 92 | 5 | yes |
| mm/page.rs | `containing_address` | 96 | 5 |  |
| mm/page.rs | `start_address` | 100 | 5 |  |
| mm/page.rs | `range` | 104 | 14 |  |
| mm/page.rs | `new` | 117 | 5 | yes |
| mm/page.rs | `containing_address` | 121 | 5 |  |
| mm/page.rs | `start_address` | 125 | 5 |  |
| mm/page.rs | `range` | 129 | 12 |  |
| mm/page.rs | `init_page_descriptors` | 140 | 7 |  |
| mm/page.rs | `frame_to_page` | 146 | 6 | yes |
| mm/page.rs | `frame_to_page_mut` | 151 | 4 | yes |
| mm/page_alloc.rs | `reclaim_gate_exit_release` | 56 | 93 | yes |
| mm/page_alloc.rs | `gate_release` | 148 | 27 | yes |
| mm/page_alloc.rs | `gate_holder_is_me` | 174 | 16 | yes |
| mm/page_alloc.rs | `gate_note_phase` | 189 | 12 | yes |
| mm/page_alloc.rs | `gate_note_round_done` | 200 | 11 | yes |
| mm/page_alloc.rs | `gate_note_deep_pass` | 210 | 15 | yes |
| mm/page_alloc.rs | `reclaim_gate_census` | 224 | 69 | yes |
| mm/page_alloc.rs | `gfp_reference_mask` | 292 | 33 | yes |
| mm/page_alloc.rs | `gfp_migratetype` | 324 | 14 | yes |
| mm/page_alloc.rs | `alloc_pages` | 337 | 49 | yes |
| mm/page_alloc.rs | `alloc_pages_inner` | 385 | 289 | yes |
| mm/page_alloc.rs | `alloc_page` | 673 | 23 | yes |
| mm/page_alloc.rs | `oom_forensic_dump` | 695 | 288 | yes |
| mm/page_alloc.rs | `alloc_page_batch` | 982 | 39 | yes |
| mm/page_alloc.rs | `get_zeroed_page` | 1020 | 42 | yes |
| mm/page_alloc.rs | `is_foreign_user_frame` | 1061 | 15 | yes |
| mm/page_alloc.rs | `report_free_refusal` | 1075 | 47 | yes |
| mm/page_alloc.rs | `pcache_leaf_phantom_zero_put` | 1121 | 42 | yes |
| mm/page_alloc.rs | `free_pages` | 1162 | 290 | yes |
| mm/page_alloc.rs | `free_page` | 1451 | 16 | yes |
| mm/page_alloc.rs | `virt_to_page` | 1466 | 6 | yes |
| mm/page_alloc.rs | `virt_to_pfn` | 1471 | 9 | yes |
| mm/page_alloc.rs | `page_to_phys` | 1479 | 6 | yes |
| mm/page_alloc.rs | `page_to_virt` | 1484 | 34 | yes |
| mm/page_alloc.rs | `init_zone_system` | 1517 | 137 | yes |
| mm/page_alloc.rs | `__get_free_pages` | 1653 | 10 | yes |
| mm/page_alloc.rs | `__get_free_page` | 1662 | 10 | yes |
| mm/page_alloc.rs | `__get_zeroed_page` | 1671 | 10 | yes |
| mm/page_alloc.rs | `__free_pages` | 1680 | 10 | yes |
| mm/page_alloc.rs | `__free_page` | 1689 | 4 | yes |
| mm/page_desc.rs | `new` | 113 | 6 | yes |
| mm/page_desc.rs | `from_raw` | 118 | 6 | yes |
| mm/page_desc.rs | `raw` | 123 | 6 | yes |
| mm/page_desc.rs | `test` | 128 | 6 | yes |
| mm/page_desc.rs | `set` | 133 | 6 | yes |
| mm/page_desc.rs | `clear` | 138 | 6 | yes |
| mm/page_desc.rs | `test_and_set` | 143 | 7 | yes |
| mm/page_desc.rs | `test_and_clear` | 149 | 7 | yes |
| mm/page_desc.rs | `clear_all` | 155 | 127 | yes |
| mm/page_desc.rs | `text_stamp_slot` | 281 | 15 | yes |
| mm/page_desc.rs | `text_stamp_insert` | 295 | 31 | yes |
| mm/page_desc.rs | `text_stamp_remove` | 325 | 18 | yes |
| mm/page_desc.rs | `text_stamp_remove_if_owner` | 342 | 22 | yes |
| mm/page_desc.rs | `new` | 363 | 17 | yes |
| mm/page_desc.rs | `text_stamp_note` | 379 | 15 | yes |
| mm/page_desc.rs | `text_stamp_get` | 393 | 20 | yes |
| mm/page_desc.rs | `new` | 412 | 21 | yes |
| mm/page_desc.rs | `init_reserved` | 432 | 7 | yes |
| mm/page_desc.rs | `init_free` | 438 | 29 | yes |
| mm/page_desc.rs | `flags` | 466 | 7 | yes |
| mm/page_desc.rs | `test_flag` | 472 | 7 | yes |
| mm/page_desc.rs | `set_flag` | 478 | 7 | yes |
| mm/page_desc.rs | `clear_all_flags` | 484 | 7 | yes |
| mm/page_desc.rs | `clear_flag` | 490 | 7 | yes |
| mm/page_desc.rs | `test_and_set_flag` | 496 | 7 | yes |
| mm/page_desc.rs | `test_and_clear_flag` | 502 | 7 | yes |
| mm/page_desc.rs | `is_locked` | 508 | 7 | yes |
| mm/page_desc.rs | `is_reserved` | 514 | 7 | yes |
| mm/page_desc.rs | `is_dirty` | 520 | 7 | yes |
| mm/page_desc.rs | `is_cow` | 526 | 7 | yes |
| mm/page_desc.rs | `is_anonymous` | 532 | 7 | yes |
| mm/page_desc.rs | `is_uptodate` | 538 | 9 | yes |
| mm/page_desc.rs | `refcount` | 546 | 8 | yes |
| mm/page_desc.rs | `get_page` | 553 | 13 | yes |
| mm/page_desc.rs | `put_page` | 565 | 29 | yes |
| mm/page_desc.rs | `put_page_site` | 593 | 15 | yes |
| mm/page_desc.rs | `try_get_page` | 607 | 21 | yes |
| mm/page_desc.rs | `set_refcount` | 627 | 9 | yes |
| mm/page_desc.rs | `mapcount` | 635 | 8 | yes |
| mm/page_desc.rs | `add_mapcount` | 642 | 8 | yes |
| mm/page_desc.rs | `sub_mapcount` | 649 | 7 | yes |
| mm/page_desc.rs | `is_mapped` | 655 | 7 | yes |
| mm/page_desc.rs | `inc_mapcount` | 661 | 7 | yes |
| mm/page_desc.rs | `dec_mapcount` | 667 | 7 | yes |
| mm/page_desc.rs | `reset_mapcount` | 673 | 9 | yes |
| mm/page_desc.rs | `private` | 681 | 7 | yes |
| mm/page_desc.rs | `set_private` | 687 | 9 | yes |
| mm/page_desc.rs | `mapping` | 695 | 7 | yes |
| mm/page_desc.rs | `set_mapping` | 701 | 7 | yes |
| mm/page_desc.rs | `index` | 707 | 7 | yes |
| mm/page_desc.rs | `set_index` | 713 | 14 | yes |
| mm/page_desc.rs | `page_type` | 726 | 14 | yes |
| mm/page_desc.rs | `set_page_type` | 739 | 9 | yes |
| mm/page_desc.rs | `lru_next` | 747 | 7 | yes |
| mm/page_desc.rs | `set_lru_next` | 753 | 7 | yes |
| mm/page_desc.rs | `lru_prev` | 759 | 7 | yes |
| mm/page_desc.rs | `set_lru_prev` | 765 | 8 | yes |
| mm/page_desc.rs | `set_lru_list` | 772 | 9 | yes |
| mm/page_desc.rs | `lru_list` | 780 | 7 | yes |
| mm/page_desc.rs | `clear_lru_list` | 786 | 13 | yes |
| mm/page_desc.rs | `rmap_alt_add` | 798 | 15 | yes |
| mm/page_desc.rs | `rmap_alt_remove` | 812 | 14 | yes |
| mm/page_desc.rs | `rmap_alt_clear` | 825 | 9 | yes |
| mm/page_desc.rs | `rmap_alt_for_each` | 833 | 15 | yes |
| mm/page_desc.rs | `next_free` | 847 | 7 | yes |
| mm/page_desc.rs | `set_next_free` | 853 | 9 | yes |
| mm/page_desc.rs | `order` | 861 | 7 | yes |
| mm/page_desc.rs | `set_order` | 867 | 7 | yes |
| mm/page_desc.rs | `is_free` | 873 | 17 | yes |
| mm/page_desc.rs | `copy_page_contents` | 889 | 56 | yes |
| mm/page_desc.rs | `descs_published` | 944 | 7 | yes |
| mm/page_desc.rs | `set_nr_descs` | 950 | 11 | yes |
| mm/page_desc.rs | `nr_descs` | 960 | 14 | yes |
| mm/page_desc.rs | `max_pfn` | 973 | 10 | yes |
| mm/page_desc.rs | `pfn_valid` | 982 | 7 | yes |
| mm/page_desc.rs | `phys_valid` | 988 | 14 | yes |
| mm/page_desc.rs | `set_present_pages` | 1001 | 8 | yes |
| mm/page_desc.rs | `present_pages` | 1008 | 20 | yes |
| mm/page_desc.rs | `mark_non_ram_pages` | 1027 | 57 | yes |
| mm/page_desc.rs | `init_mem_map` | 1083 | 66 | yes |
| mm/page_desc.rs | `pfn_to_page` | 1148 | 21 | yes |
| mm/page_desc.rs | `pfn_to_page_mut` | 1168 | 28 | yes |
| mm/page_desc.rs | `page_to_pfn` | 1195 | 10 | yes |
| mm/page_desc.rs | `phys_to_page` | 1204 | 7 | yes |
| mm/page_desc.rs | `phys_to_page_mut` | 1210 | 7 | yes |
| mm/page_desc.rs | `frame_to_page` | 1216 | 7 | yes |
| mm/page_desc.rs | `frame_to_page_mut` | 1222 | 9 | yes |
| mm/page_desc.rs | `total_pages` | 1230 | 7 | yes |
| mm/page_desc.rs | `page_size` | 1236 | 27 | yes |
| mm/page_desc.rs | `page_desc_stats` | 1262 | 68 | yes |
| mm/page_desc.rs | `get_page` | 1329 | 10 | yes |
| mm/page_desc.rs | `get_page_mut` | 1338 | 16 | yes |
| mm/page_desc.rs | `page_desc` | 1353 | 12 | yes |
| mm/pagemap.rs | `from` | 40 | 38 | yes |
| mm/pcp.rs | `new` | 67 | 34 | yes |
| mm/pcp.rs | `with_row` | 100 | 26 |  |
| mm/pcp.rs | `alloc_order0` | 125 | 83 | yes |
| mm/pcp.rs | `alloc_order0_batch` | 207 | 34 | yes |
| mm/pcp.rs | `free_order0` | 240 | 137 | yes |
| mm/pcp.rs | `to_cached` | 376 | 14 |  |
| mm/pcp.rs | `to_in_use` | 389 | 17 |  |
| mm/pcp.rs | `cached_pages_total` | 405 | 13 |  |
| mm/pcp.rs | `cached_pages_per_cpu` | 417 | 17 |  |
| mm/pcp.rs | `forensics_scan_rows` | 433 | 25 |  |
| mm/pglist.rs | `new` | 99 | 33 | yes |
| mm/pglist.rs | `init` | 131 | 9 | yes |
| mm/pglist.rs | `add_zone` | 139 | 10 | yes |
| mm/pglist.rs | `zone` | 148 | 6 | yes |
| mm/pglist.rs | `zone_mut` | 153 | 6 | yes |
| mm/pglist.rs | `zone_at` | 158 | 10 |  |
| mm/pglist.rs | `node_id` | 167 | 6 |  |
| mm/pglist.rs | `nr_zones` | 172 | 6 |  |
| mm/pglist.rs | `start_pfn` | 177 | 6 | yes |
| mm/pglist.rs | `spanned_pages` | 182 | 6 |  |
| mm/pglist.rs | `present_pages` | 187 | 6 | yes |
| mm/pglist.rs | `free_pages` | 192 | 14 | yes |
| mm/pglist.rs | `is_initialized` | 205 | 6 | yes |
| mm/pglist.rs | `stats` | 210 | 47 | yes |
| mm/pglist.rs | `init_node_data` | 256 | 10 | yes |
| mm/pglist.rs | `node_data` | 265 | 14 |  |
| mm/pglist.rs | `node_data_mut` | 278 | 11 | yes |
| mm/pglist.rs | `first_online_node` | 288 | 13 | yes |
| mm/pglist.rs | `first_online_node_mut` | 300 | 6 | yes |
| mm/pglist.rs | `num_online_nodes` | 305 | 15 |  |
| mm/pglist.rs | `select_zone` | 319 | 7 |  |
| mm/pglist.rs | `select_zone_mut` | 325 | 9 |  |
| mm/pglist.rs | `print_buddyinfo` | 333 | 30 |  |
| mm/pglist.rs | `print_zoneinfo` | 362 | 29 |  |
| mm/policy/aarch64.rs | `try_expand_stack` | 35 | 156 |  |
| mm/policy/aarch64.rs | `protnone_represent` | 190 | 97 |  |
| mm/policy/aarch64.rs | `handle_mm_fault` | 286 | 309 | yes |
| mm/policy/aarch64.rs | `map_vma` | 594 | 24 |  |
| mm/policy/aarch64.rs | `set_brk` | 617 | 121 | yes |
| mm/policy/aarch64.rs | `mmap` | 737 | 166 | yes |
| mm/policy/aarch64.rs | `find_free_area` | 902 | 8 | yes |
| mm/policy/aarch64.rs | `find_free_area_locked` | 909 | 45 |  |
| mm/policy/aarch64.rs | `munmap` | 953 | 144 | yes |
| mm/policy/aarch64.rs | `zap_page_range` | 1096 | 13 | yes |
| mm/policy/aarch64.rs | `do_brk` | 1108 | 6 |  |
| mm/policy/aarch64.rs | `allocate_stack` | 1113 | 27 |  |
| mm/policy/aarch64.rs | `fork` | 1139 | 121 | yes |
| mm/policy/riscv64.rs | `try_expand_stack` | 47 | 223 |  |
| mm/policy/riscv64.rs | `pte_counts_as_mapped` | 269 | 6 |  |
| mm/policy/riscv64.rs | `handle_mm_fault` | 274 | 771 | yes |
| mm/policy/riscv64.rs | `anon_faultaround_forward` | 1044 | 81 |  |
| mm/policy/riscv64.rs | `is_protnone_marker` | 1124 | 21 |  |
| mm/policy/riscv64.rs | `handle_swap_fault` | 1144 | 159 |  |
| mm/policy/riscv64.rs | `map_vma` | 1302 | 23 |  |
| mm/policy/riscv64.rs | `set_brk` | 1324 | 131 | yes |
| mm/policy/riscv64.rs | `mmap` | 1454 | 220 | yes |
| mm/policy/riscv64.rs | `find_free_area` | 1673 | 8 | yes |
| mm/policy/riscv64.rs | `find_free_area_locked` | 1680 | 46 |  |
| mm/policy/riscv64.rs | `munmap` | 1725 | 176 | yes |
| mm/policy/riscv64.rs | `zap_page_range` | 1900 | 15 | yes |
| mm/policy/riscv64.rs | `munmap_legacy` | 1914 | 46 |  |
| mm/policy/riscv64.rs | `set_range_permissions` | 1959 | 55 |  |
| mm/policy/riscv64.rs | `do_brk` | 2013 | 6 |  |
| mm/policy/riscv64.rs | `allocate_stack` | 2018 | 24 |  |
| mm/policy/riscv64.rs | `fork` | 2041 | 126 | yes |
| mm/policy/x86_64.rs | `try_expand_stack` | 84 | 201 |  |
| mm/policy/x86_64.rs | `pte_counts_as_mapped` | 284 | 6 |  |
| mm/policy/x86_64.rs | `handle_mm_fault` | 289 | 1620 | yes |
| mm/policy/x86_64.rs | `is_protnone_marker` | 1908 | 19 |  |
| mm/policy/x86_64.rs | `handle_swap_fault` | 1926 | 157 |  |
| mm/policy/x86_64.rs | `map_vma` | 2082 | 24 |  |
| mm/policy/x86_64.rs | `set_brk` | 2105 | 124 | yes |
| mm/policy/x86_64.rs | `mmap` | 2228 | 324 | yes |
| mm/policy/x86_64.rs | `find_free_area_32bit` | 2551 | 9 |  |
| mm/policy/x86_64.rs | `find_free_area_32bit_locked` | 2559 | 38 |  |
| mm/policy/x86_64.rs | `find_free_area` | 2596 | 10 | yes |
| mm/policy/x86_64.rs | `find_free_area_locked` | 2605 | 43 |  |
| mm/policy/x86_64.rs | `munmap` | 2647 | 269 | yes |
| mm/policy/x86_64.rs | `zap_page_range` | 2915 | 17 | yes |
| mm/policy/x86_64.rs | `remove_vma_piece` | 2931 | 72 |  |
| mm/policy/x86_64.rs | `range_is_pte_empty` | 3002 | 156 |  |
| mm/policy/x86_64.rs | `set_range_permissions` | 3157 | 73 |  |
| mm/policy/x86_64.rs | `do_brk` | 3229 | 6 |  |
| mm/policy/x86_64.rs | `allocate_stack` | 3234 | 24 |  |
| mm/policy/x86_64.rs | `fork` | 3257 | 161 | yes |
| mm/rmap.rs | `new` | 51 | 11 | yes |
| mm/rmap.rs | `get` | 61 | 7 | yes |
| mm/rmap.rs | `put` | 67 | 6 | yes |
| mm/rmap.rs | `refcount` | 72 | 6 | yes |
| mm/rmap.rs | `set_vma` | 77 | 6 |  |
| mm/rmap.rs | `get_vma` | 82 | 25 |  |
| mm/rmap.rs | `page_add_anon_rmap` | 106 | 31 |  |
| mm/rmap.rs | `page_add_file_rmap` | 136 | 24 |  |
| mm/rmap.rs | `page_remove_rmap` | 159 | 30 |  |
| mm/rmap.rs | `page_mapped` | 188 | 11 | yes |
| mm/rmap.rs | `page_get_mappings` | 198 | 16 |  |
| mm/rmap.rs | `page_referenced` | 213 | 7 | yes |
| mm/rmap.rs | `page_clear_referenced` | 219 | 12 |  |
| mm/rmap.rs | `try_to_unmap` | 230 | 12 |  |
| mm/rmap.rs | `try_to_unmap_with_swap` | 241 | 14 | yes |
| mm/rmap.rs | `page_record_mapping` | 254 | 56 | yes |
| mm/rmap.rs | `try_to_unmap_migration` | 309 | 9 | yes |
| mm/rmap.rs | `try_to_unmap_inner` | 317 | 204 |  |
| mm/rmap.rs | `rmap_stats` | 520 | 5 |  |
| mm/slab.rs | `new` | 79 | 35 | yes |
| mm/slab.rs | `new` | 113 | 24 | yes |
| mm/slab.rs | `alloc` | 136 | 44 | yes |
| mm/slab.rs | `alloc_from_slab` | 179 | 65 | yes |
| mm/slab.rs | `free` | 243 | 78 | yes |
| mm/slab.rs | `create_slab` | 320 | 44 | yes |
| mm/slab.rs | `move_slab_to_full` | 363 | 26 | yes |
| mm/slab.rs | `move_slab_from_full` | 388 | 50 | yes |
| mm/slab.rs | `new` | 437 | 10 | yes |
| mm/slab.rs | `base_addr` | 446 | 6 | yes |
| mm/slab.rs | `max_pages` | 451 | 6 | yes |
| mm/slab.rs | `alloc_page` | 456 | 11 | yes |
| mm/slab.rs | `get_page_addr` | 466 | 6 | yes |
| mm/slab.rs | `get_header_mut` | 471 | 10 | yes |
| mm/slab.rs | `get_next` | 480 | 6 | yes |
| mm/slab.rs | `set_next` | 485 | 6 | yes |
| mm/slab.rs | `get_prev` | 490 | 6 | yes |
| mm/slab.rs | `set_prev` | 495 | 6 | yes |
| mm/slab.rs | `find_slab_by_addr` | 500 | 27 | yes |
| mm/slab.rs | `slab_pages_cell` | 526 | 42 | yes |
| mm/slab.rs | `init` | 567 | 9 | yes |
| mm/slab.rs | `is_initialized` | 575 | 6 | yes |
| mm/slab.rs | `pages` | 580 | 8 | yes |
| mm/slab.rs | `find_cache_index` | 587 | 24 | yes |
| mm/slab.rs | `audit_cache_locks` | 610 | 11 | yes |
| mm/slab.rs | `kmalloc` | 620 | 37 | yes |
| mm/slab.rs | `kfree` | 656 | 47 | yes |
| mm/slab.rs | `kzalloc` | 702 | 16 | yes |
| mm/slab.rs | `init_slab` | 717 | 7 | yes |
| mm/slab.rs | `is_slab_initialized` | 723 | 11 | yes |
| mm/slab.rs | `slab_region` | 733 | 12 | yes |
| mm/slab.rs | `slab_stats` | 744 | 41 | yes |
| mm/small_alloc.rs | `hdr_of` | 86 | 11 | yes |
| mm/small_alloc.rs | `first_off` | 96 | 6 | yes |
| mm/small_alloc.rs | `class_of` | 101 | 15 | yes |
| mm/small_alloc.rs | `effective_size` | 115 | 16 | yes |
| mm/small_alloc.rs | `bmp_set` | 130 | 7 | yes |
| mm/small_alloc.rs | `bmp_clear` | 136 | 8 | yes |
| mm/small_alloc.rs | `is_slab_ptr` | 143 | 17 | yes |
| mm/small_alloc.rs | `object_span` | 159 | 54 | yes |
| mm/small_alloc.rs | `new` | 212 | 100 | yes |
| mm/small_alloc.rs | `alloc` | 311 | 129 | yes |
| mm/small_alloc.rs | `alloc_locked` | 439 | 117 | yes |
| mm/small_alloc.rs | `free` | 555 | 161 | yes |
| mm/small_alloc.rs | `flush_batch` | 715 | 109 | yes |
| mm/small_alloc.rs | `live_stats_kib` | 823 | 26 | yes |
| mm/small_alloc.rs | `audit_section_peek` | 848 | 17 | yes |
| mm/small_alloc.rs | `audit_class_locks` | 864 | 11 | yes |
| mm/small_alloc.rs | `census_peek` | 874 | 98 | yes |
| mm/small_alloc.rs | `census_peek3` | 971 | 17 | yes |
| mm/small_alloc.rs | `is_kheap` | 987 | 8 | yes |
| mm/small_alloc.rs | `is_userva` | 994 | 79 | yes |
| mm/small_alloc.rs | `census_line` | 1072 | 32 | yes |
| mm/unwatch.rs | `note` | 72 | 17 |  |
| mm/unwatch.rs | `dump_tail` | 88 | 37 | yes |
| mm/unwatch.rs | `covered_by_unmap` | 124 | 33 | yes |
| mm/vdso.rs | `vdso_ehdr` | 80 | 29 | yes |
| mm/vdso.rs | `vdso_seq` | 108 | 10 |  |
| mm/vdso.rs | `vdso_data_tick` | 117 | 90 | yes |
| mm/vdso.rs | `bump` | 206 | 10 | yes |
| mm/vdso.rs | `bump_pub` | 215 | 11 |  |
| mm/vdso.rs | `rd` | 225 | 9 | yes |
| mm/vdso.rs | `wr` | 233 | 15 |  |
| mm/vdso.rs | `read_tsc_ordered` | 247 | 31 |  |
| mm/vdso.rs | `tsc_delta_nominal` | 277 | 12 |  |
| mm/vdso.rs | `seq_try_begin` | 288 | 13 |  |
| mm/vdso.rs | `seq_end` | 300 | 9 |  |
| mm/vdso.rs | `publish_tsc_anchor` | 308 | 20 |  |
| mm/vdso.rs | `tsc_anchor_step_forward` | 327 | 21 |  |
| mm/vdso.rs | `tsc_anchor_step_back` | 347 | 19 |  |
| mm/vdso.rs | `tsc_anchor_peek` | 365 | 42 |  |
| mm/vdso.rs | `tsc_track_now_nominal` | 406 | 43 |  |
| mm/vdso.rs | `vdso_data_tick` | 448 | 48 | yes |
| mm/vdso.rs | `vdso_pages_phys` | 495 | 26 | yes |
| mm/vdso.rs | `vdso_pages_phys` | 520 | 11 | yes |
| mm/vdso.rs | `is_vdso_phys` | 530 | 32 |  |
| mm/vdso.rs | `new` | 561 | 8 | yes |
| mm/vdso.rs | `label` | 568 | 4 |  |
| mm/vdso.rs | `emit` | 571 | 5 | yes |
| mm/vdso.rs | `br` | 575 | 6 |  |
| mm/vdso.rs | `jal_rel` | 580 | 5 |  |
| mm/vdso.rs | `finish` | 584 | 13 |  |
| mm/vdso.rs | `r_type` | 596 | 4 |  |
| mm/vdso.rs | `i_type` | 599 | 4 |  |
| mm/vdso.rs | `s_type` | 602 | 6 |  |
| mm/vdso.rs | `b_imm` | 607 | 5 |  |
| mm/vdso.rs | `j_imm` | 611 | 4 |  |
| mm/vdso.rs | `u_type` | 614 | 4 |  |
| mm/vdso.rs | `beq_skel` | 617 | 4 |  |
| mm/vdso.rs | `bne_skel` | 620 | 37 |  |
| mm/vdso.rs | `emit_li` | 656 | 20 |  |
| mm/vdso.rs | `emit_data_base` | 675 | 9 |  |
| mm/vdso.rs | `emit_seqlock_read` | 683 | 22 |  |
| mm/vdso.rs | `emit_interpolate` | 704 | 9 |  |
| mm/vdso.rs | `emit_ret` | 712 | 6 |  |
| mm/vdso.rs | `emit_clock_gettime` | 717 | 47 |  |
| mm/vdso.rs | `emit_gettimeofday` | 763 | 34 |  |
| mm/vdso.rs | `emit_clock_getres` | 796 | 70 |  |
| mm/vdso.rs | `new` | 865 | 9 | yes |
| mm/vdso.rs | `b` | 873 | 4 |  |
| mm/vdso.rs | `label` | 876 | 4 |  |
| mm/vdso.rs | `jcc` | 879 | 6 |  |
| mm/vdso.rs | `jmp32` | 884 | 6 |  |
| mm/vdso.rs | `jcc32` | 889 | 5 |  |
| mm/vdso.rs | `finish` | 893 | 26 |  |
| mm/vdso.rs | `lea_data_base` | 918 | 27 |  |
| mm/vdso.rs | `seqlock_read` | 944 | 29 |  |
| mm/vdso.rs | `interpolate` | 972 | 12 |  |
| mm/vdso.rs | `split_store` | 983 | 24 |  |
| mm/vdso.rs | `dispatch` | 1006 | 26 |  |
| mm/vdso.rs | `syscall_fallback` | 1031 | 8 |  |
| mm/vdso.rs | `emit_clock_gettime` | 1038 | 35 |  |
| mm/vdso.rs | `emit_gettimeofday` | 1072 | 41 |  |
| mm/vdso.rs | `emit_clock_getres` | 1112 | 33 |  |
| mm/vdso.rs | `elf_hash` | 1144 | 16 |  |
| mm/vdso.rs | `build_code_page` | 1159 | 328 |  |
| mm/vdso.rs | `vdso_init` | 1486 | 11 |  |
| mm/vma.rs | `new` | 70 | 6 | yes |
| mm/vma.rs | `from_bits` | 75 | 6 | yes |
| mm/vma.rs | `bits` | 80 | 6 | yes |
| mm/vma.rs | `contains` | 85 | 6 | yes |
| mm/vma.rs | `insert` | 90 | 6 | yes |
| mm/vma.rs | `remove` | 95 | 7 | yes |
| mm/vma.rs | `is_readable` | 101 | 7 | yes |
| mm/vma.rs | `is_writable` | 107 | 7 | yes |
| mm/vma.rs | `is_executable` | 113 | 7 | yes |
| mm/vma.rs | `is_shared` | 119 | 7 | yes |
| mm/vma.rs | `is_private` | 125 | 17 |  |
| mm/vma.rs | `to_page_perm` | 141 | 22 | yes |
| mm/vma.rs | `default` | 162 | 96 | yes |
| mm/vma.rs | `from_bits` | 257 | 7 | yes |
| mm/vma.rs | `set_creator` | 263 | 7 |  |
| mm/vma.rs | `mark_forked` | 269 | 6 |  |
| mm/vma.rs | `bits` | 274 | 7 | yes |
| mm/vma.rs | `creator_name` | 280 | 19 | yes |
| mm/vma.rs | `is_forked` | 298 | 7 | yes |
| mm/vma.rs | `default` | 304 | 8 | yes |
| mm/vma.rs | `new` | 311 | 20 | yes |
| mm/vma.rs | `start` | 330 | 7 | yes |
| mm/vma.rs | `end` | 336 | 7 | yes |
| mm/vma.rs | `size` | 342 | 7 | yes |
| mm/vma.rs | `page_count` | 348 | 7 | yes |
| mm/vma.rs | `flags` | 354 | 6 | yes |
| mm/vma.rs | `set_flags` | 359 | 7 | yes |
| mm/vma.rs | `vma_type` | 365 | 6 | yes |
| mm/vma.rs | `set_type` | 370 | 7 | yes |
| mm/vma.rs | `contains` | 376 | 6 | yes |
| mm/vma.rs | `overlaps` | 381 | 7 | yes |
| mm/vma.rs | `set_offset` | 387 | 7 | yes |
| mm/vma.rs | `offset` | 393 | 6 | yes |
| mm/vma.rs | `set_file_fd` | 398 | 7 | yes |
| mm/vma.rs | `file_fd` | 404 | 6 | yes |
| mm/vma.rs | `set_file_size` | 409 | 7 | yes |
| mm/vma.rs | `file_size` | 415 | 7 | yes |
| mm/vma.rs | `origin` | 421 | 7 | yes |
| mm/vma.rs | `set_origin` | 427 | 8 | yes |
| mm/vma.rs | `split` | 434 | 49 | yes |
| mm/vma.rs | `can_merge` | 482 | 25 |  |
| mm/vma.rs | `merge` | 506 | 11 |  |
| mm/vma.rs | `merge_at_end` | 516 | 7 |  |
| mm/vma.rs | `fmt` | 522 | 27 |  |
| mm/vma.rs | `new` | 548 | 24 | yes |
| mm/vma.rs | `add` | 571 | 108 | yes |
| mm/vma.rs | `find` | 678 | 21 | yes |
| mm/vma.rs | `find_mut` | 698 | 27 | yes |
| mm/vma.rs | `remove` | 724 | 17 | yes |
| mm/vma.rs | `iter` | 740 | 8 | yes |
| mm/vma.rs | `iter_after` | 747 | 15 | yes |
| mm/vma.rs | `last_at_or_before` | 761 | 11 |  |
| mm/vma.rs | `has_vma_starting_within` | 771 | 7 |  |
| mm/vma.rs | `count` | 777 | 6 | yes |
| mm/vma.rs | `get` | 782 | 6 | yes |
| mm/vma.rs | `get_mut` | 787 | 6 | yes |
| mm/vma.rs | `first` | 792 | 6 | yes |
| mm/vma.rs | `last` | 797 | 6 | yes |
| mm/vma.rs | `find_vma_after` | 802 | 6 | yes |
| mm/vma.rs | `clear` | 807 | 8 | yes |
| mm/vma.rs | `max_end` | 814 | 19 |  |
| mm/vma.rs | `expand_downwards` | 832 | 62 |  |
| mm/vma.rs | `find_stack_vma` | 893 | 24 |  |
| mm/vma.rs | `default` | 916 | 32 | yes |
| mm/vma.rs | `user_start` | 947 | 4 |  |
| mm/vma.rs | `user_end` | 950 | 4 |  |
| mm/vma.rs | `default_stack_size` | 953 | 4 |  |
| mm/vma.rs | `default_stack_top` | 956 | 4 |  |
| mm/vma.rs | `heap_start` | 959 | 4 |  |
| mm/vma.rs | `heap_end` | 962 | 26 |  |
| mm/vma.rs | `user_start` | 987 | 7 |  |
| mm/vma.rs | `user_end` | 993 | 7 |  |
| mm/vma.rs | `default_stack_size` | 999 | 7 |  |
| mm/vma.rs | `default_stack_top` | 1005 | 7 |  |
| mm/vma.rs | `heap_start` | 1011 | 7 |  |
| mm/vma.rs | `heap_end` | 1017 | 5 |  |
| mm/vmemmap.rs | `pfn_to_vmemmap` | 54 | 15 |  |
| mm/vmemmap.rs | `vmemmap_to_pfn` | 68 | 11 |  |
| mm/vmemmap.rs | `is_vmemmap_initialized` | 78 | 17 |  |
| mm/vmemmap.rs | `init_vmemmap` | 94 | 117 |  |
| mm/vmemmap.rs | `vmemmap_stats` | 210 | 17 | yes |
| mm/vmstat.rs | `note_fault` | 48 | 9 | yes |
| mm/vmstat.rs | `note_major_fault` | 56 | 6 |  |
| mm/vmstat.rs | `note_free` | 61 | 6 |  |
| mm/vmstat.rs | `note_alloc` | 66 | 12 |  |
| mm/zone.rs | `name` | 71 | 12 | yes |
| mm/zone.rs | `index` | 82 | 23 | yes |
| mm/zone.rs | `new` | 104 | 9 | yes |
| mm/zone.rs | `is_empty` | 112 | 6 | yes |
| mm/zone.rs | `nr_free` | 117 | 6 | yes |
| mm/zone.rs | `inc_free` | 122 | 6 |  |
| mm/zone.rs | `dec_free` | 127 | 63 |  |
| mm/zone.rs | `new_free_areas` | 189 | 12 |  |
| mm/zone.rs | `new` | 200 | 27 | yes |
| mm/zone.rs | `init` | 226 | 25 | yes |
| mm/zone.rs | `is_initialized` | 250 | 6 | yes |
| mm/zone.rs | `zone_type` | 255 | 6 | yes |
| mm/zone.rs | `zone_id` | 260 | 6 |  |
| mm/zone.rs | `node_id` | 265 | 6 |  |
| mm/zone.rs | `start_pfn` | 270 | 6 | yes |
| mm/zone.rs | `end_pfn` | 275 | 6 | yes |
| mm/zone.rs | `spanned_pages` | 280 | 6 |  |
| mm/zone.rs | `present_pages` | 285 | 6 | yes |
| mm/zone.rs | `managed_pages` | 290 | 6 | yes |
| mm/zone.rs | `nr_free` | 295 | 7 | yes |
| mm/zone.rs | `free_pages_add` | 301 | 6 |  |
| mm/zone.rs | `free_pages_sub` | 306 | 8 |  |
| mm/zone.rs | `set_managed_pages` | 313 | 6 | yes |
| mm/zone.rs | `watermark` | 318 | 6 | yes |
| mm/zone.rs | `set_watermark` | 323 | 11 |  |
| mm/zone.rs | `watermark_ok` | 333 | 8 | yes |
| mm/zone.rs | `free_pages_order` | 340 | 15 |  |
| mm/zone.rs | `alloc_pages` | 354 | 38 | yes |
| mm/zone.rs | `alloc_pages_order0_batch` | 391 | 190 | yes |
| mm/zone.rs | `alloc_pages_locked` | 580 | 68 |  |
| mm/zone.rs | `alloc_from_order` | 647 | 68 |  |
| mm/zone.rs | `free_pages` | 714 | 163 | yes |
| mm/zone.rs | `find_buddy` | 876 | 14 |  |
| mm/zone.rs | `is_buddy_free` | 889 | 29 |  |
| mm/zone.rs | `add_to_free_list` | 917 | 30 | yes |
| mm/zone.rs | `add_to_free_list_init` | 946 | 21 | yes |
| mm/zone.rs | `remove_from_free_list` | 966 | 53 | yes |
| mm/zone.rs | `alloc_single_page` | 1018 | 9 | yes |
| mm/zone.rs | `has_free_block` | 1026 | 17 | yes |
| mm/zone.rs | `forensic_free_report` | 1042 | 32 | yes |
| mm/zone.rs | `stats` | 1073 | 78 | yes |
| mm/zone.rs | `zone_type` | 1150 | 18 | yes |
| mm/zone.rs | `int_sqrt` | 1167 | 20 |  |
| mm/zone.rs | `setup_per_zone_wmarks` | 1186 | 54 | yes |
| mm/zone.rs | `pfn_to_phys` | 1239 | 6 | yes |
| mm/zone.rs | `phys_to_pfn` | 1244 | 6 | yes |
| mm/zone.rs | `print_zone_info` | 1249 | 41 |  |
