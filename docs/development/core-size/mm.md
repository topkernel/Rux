# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# mm — hot 12779 lines (stay) / cold 9537 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| mm/buddy_allocator.rs | `refused_free_note` | 127 | 26 | yes |
| mm/buddy_allocator.rs | `new` | 153 | 22 | yes |
| mm/buddy_allocator.rs | `new` | 175 | 7 | yes |
| mm/buddy_allocator.rs | `get` | 182 | 7 | yes |
| mm/buddy_allocator.rs | `get_mut` | 189 | 37 | yes |
| mm/buddy_allocator.rs | `heap_bounds` | 226 | 15 | yes |
| mm/buddy_allocator.rs | `buddy_block_span` | 241 | 22 | yes |
| mm/buddy_allocator.rs | `new` | 263 | 14 | yes |
| mm/buddy_allocator.rs | `check_magic` | 277 | 8 | yes |
| mm/buddy_allocator.rs | `init` | 285 | 14 | yes |
| mm/buddy_allocator.rs | `init_sized` | 299 | 45 | yes |
| mm/buddy_allocator.rs | `page_is_free` | 344 | 9 | yes |
| mm/buddy_allocator.rs | `fill_free_bits` | 353 | 38 | yes |
| mm/buddy_allocator.rs | `fill_interior_order` | 391 | 28 | yes |
| mm/buddy_allocator.rs | `init_block` | 419 | 18 | yes |
| mm/buddy_allocator.rs | `add_to_free_list` | 437 | 37 | yes |
| mm/buddy_allocator.rs | `remove_from_free_list` | 474 | 33 | yes |
| mm/buddy_allocator.rs | `heap_size_to_order` | 507 | 21 | yes |
| mm/buddy_allocator.rs | `size_to_order` | 528 | 9 | yes |
| mm/buddy_allocator.rs | `get_buddy_idx` | 537 | 6 | yes |
| mm/buddy_allocator.rs | `page_idx_to_addr` | 543 | 7 | yes |
| mm/buddy_allocator.rs | `addr_to_page_idx_checked` | 550 | 14 | yes |
| mm/buddy_allocator.rs | `alloc_blocks` | 564 | 84 | yes |
| mm/buddy_allocator.rs | `free_blocks` | 648 | 89 | yes |
| mm/buddy_allocator.rs | `alloc` | 737 | 106 | yes |
| mm/buddy_allocator.rs | `realloc` | 843 | 31 | yes |
| mm/buddy_allocator.rs | `dealloc` | 874 | 33 | yes |
| mm/buddy_allocator.rs | `trace_kmalloc_at` | 907 | 24 | yes |
| mm/buddy_allocator.rs | `trace_kfree_at` | 931 | 16 | yes |
| mm/buddy_allocator.rs | `dealloc_untracked_pub` | 947 | 10 | yes |
| mm/buddy_allocator.rs | `alloc_page_raw` | 957 | 20 | yes |
| mm/buddy_allocator.rs | `free_page_raw` | 977 | 26 | yes |
| mm/buddy_allocator.rs | `dealloc_untracked` | 1003 | 99 | yes |
| mm/buddy_allocator.rs | `audit_lock_held` | 1102 | 17 | yes |
| mm/buddy_allocator.rs | `init_heap` | 1119 | 7 | yes |
| mm/buddy_allocator.rs | `init_heap_sized` | 1126 | 38 | yes |
| mm/buddy_allocator.rs | `buddy_free_census_lockfree` | 1164 | 27 | yes |
| mm/buddy_allocator.rs | `buddy_stats` | 1191 | 130 | yes |
| mm/buddy_allocator.rs | `record` | 1321 | 19 | yes |
| mm/buddy_allocator.rs | `meta_audit_inner` | 1340 | 219 | yes |
| mm/buddy_allocator.rs | `buddy_meta_shadow_scan` | 1559 | 31 | yes |
| mm/buddy_allocator.rs | `buddy_meta_selftest` | 1590 | 64 | yes |
| mm/buddy_allocator.rs | `dealloc_direct` | 1654 | 28 | yes |
| mm/buddy_allocator.rs | `alloc` | 1682 | 29 | yes |
| mm/buddy_allocator.rs | `dealloc` | 1711 | 18 | yes |
| mm/deferred_free.rs | `register_deferred_size` | 91 | 21 |  |
| mm/deferred_free.rs | `deferred_size` | 112 | 9 |  |
| mm/deferred_free.rs | `try_defer` | 121 | 56 | yes |
| mm/deferred_free.rs | `drain_if_due` | 177 | 13 | yes |
| mm/deferred_free.rs | `drain` | 190 | 53 | yes |
| mm/edges.rs | `first_node_mut` | 37 | 9 | yes |
| mm/edges.rs | `first_node` | 46 | 8 | yes |
| mm/edges.rs | `page_of` | 54 | 14 | yes |
| mm/edges.rs | `page_ref_of` | 68 | 21 | yes |
| mm/edges.rs | `memblock_ref` | 89 | 9 | yes |
| mm/edges.rs | `memblock_mut` | 98 | 9 | yes |
| mm/edges.rs | `walk_fp_chain` | 107 | 17 | yes |
| mm/edges.rs | `slab_read_freelist_link` | 124 | 8 | yes |
| mm/edges.rs | `slab_write_freelist_link` | 132 | 9 | yes |
| mm/edges.rs | `slab_header_mut` | 141 | 9 | yes |
| mm/edges.rs | `slab_header_cache_idx` | 150 | 8 | yes |
| mm/edges.rs | `slab_pages_init` | 158 | 11 | yes |
| mm/edges.rs | `slab_pages` | 169 | 8 | yes |
| mm/edges.rs | `slab_zero` | 177 | 32 | yes |
| mm/edges.rs | `pcp_row_exclusive` | 209 | 13 |  |
| mm/edges.rs | `pcp_row_count_peek` | 222 | 9 | yes |
| mm/edges.rs | `pcp_row_ref_peek` | 231 | 12 | yes |
| mm/edges.rs | `copy_kva` | 243 | 9 | yes |
| mm/edges.rs | `copy_kva_from` | 252 | 14 | yes |
| mm/edges.rs | `page_slice_mut` | 266 | 8 | yes |
| mm/edges.rs | `page_slice` | 274 | 14 | yes |
| mm/edges.rs | `defer_pool_mut` | 288 | 11 | yes |
| mm/edges.rs | `defer_dealloc` | 299 | 20 | yes |
| mm/edges.rs | `vmemmap_stats_read` | 319 | 8 | yes |
| mm/edges.rs | `vmemmap_stats_write` | 327 | 9 | yes |
| mm/edges.rs | `vmemmap_zero_region` | 336 | 11 | yes |
| mm/edges.rs | `vmemmap_map_kernel_page` | 347 | 13 | yes |
| mm/edges.rs | `switch_address_space` | 360 | 27 | yes |
| mm/edges.rs | `compact_copy_page` | 387 | 13 | yes |
| mm/edges.rs | `kernel_layout_write` | 400 | 9 | yes |
| mm/edges.rs | `kernel_layout_ref` | 409 | 12 | yes |
| mm/edges.rs | `node_data_init` | 421 | 18 | yes |
| mm/edges.rs | `x86_caller_return_address` | 439 | 19 | yes |
| mm/edges.rs | `sa_hdr_mut` | 458 | 9 | yes |
| mm/edges.rs | `sa_hdr_ref` | 467 | 9 | yes |
| mm/edges.rs | `sa_read_u16` | 476 | 8 | yes |
| mm/edges.rs | `sa_write_u16` | 484 | 8 | yes |
| mm/edges.rs | `sa_read_u64` | 492 | 4 | yes |
| mm/gup.rs | `va` | 69 | 5 |  |
| mm/gup.rs | `len` | 74 | 5 | yes |
| mm/gup.rs | `head_off` | 79 | 5 |  |
| mm/gup.rs | `page_of` | 84 | 7 | yes |
| mm/gup.rs | `piece` | 91 | 18 |  |
| mm/gup.rs | `for_each_piece` | 109 | 31 | yes |
| mm/gup.rs | `range_pinned` | 140 | 17 |  |
| mm/gup.rs | `next_piece` | 157 | 40 |  |
| mm/gup.rs | `page_end_off` | 197 | 8 |  |
| mm/gup.rs | `first_ptr` | 205 | 9 |  |
| mm/gup.rs | `copy_into_pinned` | 214 | 16 |  |
| mm/gup.rs | `copy_from_pinned` | 230 | 14 |  |
| mm/gup.rs | `pinned_pages` | 244 | 5 |  |
| mm/gup.rs | `page_count` | 249 | 5 |  |
| mm/gup.rs | `unpin` | 254 | 15 |  |
| mm/gup.rs | `drop` | 269 | 17 | yes |
| mm/gup.rs | `pin_user_pages` | 286 | 85 |  |
| mm/gup.rs | `pin_one_faulted` | 371 | 62 |  |
| mm/gup.rs | `handle_mm_fault_checked` | 433 | 7 |  |
| mm/layout.rs | `heap_region_end` | 90 | 9 |  |
| mm/layout.rs | `effective_static_regions` | 99 | 101 |  |
| mm/layout.rs | `new` | 200 | 27 | yes |
| mm/layout.rs | `init_from_memblock` | 227 | 74 |  |
| mm/layout.rs | `kernel_layout_init` | 301 | 11 |  |
| mm/layout.rs | `kernel_layout` | 312 | 10 |  |
| mm/layout.rs | `is_kernel_layout_initialized` | 322 | 8 |  |
| mm/layout.rs | `phys_memory_base` | 330 | 6 |  |
| mm/layout.rs | `phys_memory_size` | 336 | 6 |  |
| mm/layout.rs | `kernel_start` | 342 | 6 |  |
| mm/layout.rs | `kernel_end` | 348 | 6 |  |
| mm/layout.rs | `heap_start` | 354 | 6 |  |
| mm/layout.rs | `heap_size` | 360 | 6 |  |
| mm/layout.rs | `slab_start` | 366 | 6 |  |
| mm/layout.rs | `slab_size` | 372 | 6 |  |
| mm/layout.rs | `user_phys_start` | 378 | 6 |  |
| mm/layout.rs | `user_phys_size` | 384 | 6 |  |
| mm/layout.rs | `frame_alloc_start` | 390 | 6 |  |
| mm/layout.rs | `frame_alloc_size` | 396 | 6 |  |
| mm/layout.rs | `heap_end` | 402 | 6 |  |
| mm/layout.rs | `slab_end` | 408 | 6 |  |
| mm/layout.rs | `user_phys_end` | 414 | 17 |  |
| mm/layout.rs | `assert_memblock_consistency` | 431 | 46 |  |
| mm/memblock.rs | `new` | 65 | 9 | yes |
| mm/memblock.rs | `with_flags` | 74 | 11 |  |
| mm/memblock.rs | `end` | 85 | 6 | yes |
| mm/memblock.rs | `contains` | 91 | 6 | yes |
| mm/memblock.rs | `base_pfn` | 97 | 6 |  |
| mm/memblock.rs | `end_pfn` | 103 | 6 | yes |
| mm/memblock.rs | `page_count` | 109 | 17 |  |
| mm/memblock.rs | `new` | 126 | 17 | yes |
| mm/memblock.rs | `insert_and_merge_all` | 143 | 78 |  |
| mm/memblock.rs | `add` | 221 | 13 | yes |
| mm/memblock.rs | `add_reserved` | 234 | 5 |  |
| mm/memblock.rs | `remove` | 239 | 15 | yes |
| mm/memblock.rs | `iter` | 254 | 6 | yes |
| mm/memblock.rs | `len` | 260 | 6 | yes |
| mm/memblock.rs | `is_empty` | 266 | 6 | yes |
| mm/memblock.rs | `total_size` | 272 | 5 |  |
| mm/memblock.rs | `find` | 277 | 5 | yes |
| mm/memblock.rs | `find_first_available` | 282 | 22 |  |
| mm/memblock.rs | `new` | 304 | 12 | yes |
| mm/memblock.rs | `init` | 316 | 8 | yes |
| mm/memblock.rs | `add_memory` | 324 | 12 |  |
| mm/memblock.rs | `reserve` | 336 | 12 |  |
| mm/memblock.rs | `reserve_nomap` | 348 | 6 |  |
| mm/memblock.rs | `get_available_region` | 354 | 31 |  |
| mm/memblock.rs | `total_memory` | 385 | 6 |  |
| mm/memblock.rs | `total_reserved` | 391 | 5 |  |
| mm/memblock.rs | `available_memory` | 396 | 6 |  |
| mm/memblock.rs | `memory` | 402 | 6 |  |
| mm/memblock.rs | `reserved` | 408 | 5 | yes |
| mm/memblock.rs | `is_reserved` | 413 | 5 | yes |
| mm/memblock.rs | `is_memory` | 418 | 5 |  |
| mm/memblock.rs | `find_in_range` | 423 | 34 |  |
| mm/memblock.rs | `dump` | 457 | 18 | yes |
| mm/memblock.rs | `for_each_free_range` | 475 | 61 |  |
| mm/memblock.rs | `memblock_raw` | 536 | 8 | yes |
| mm/memblock.rs | `memblock_init` | 544 | 11 |  |
| mm/memblock.rs | `memblock_add` | 555 | 6 |  |
| mm/memblock.rs | `memblock_reserve` | 561 | 6 |  |
| mm/memblock.rs | `memblock_reserve_nomap` | 567 | 6 |  |
| mm/memblock.rs | `memblock_get_available_region` | 573 | 7 |  |
| mm/memblock.rs | `memblock_total_memory` | 580 | 6 |  |
| mm/memblock.rs | `memblock_available_memory` | 586 | 6 | yes |
| mm/memblock.rs | `memblock_is_reserved` | 592 | 6 |  |
| mm/memblock.rs | `memblock_for_each_free_range` | 598 | 9 | yes |
| mm/memblock.rs | `memblock_find_in_range` | 607 | 6 |  |
| mm/memblock.rs | `memblock_dump` | 613 | 11 |  |
| mm/memblock.rs | `memblock` | 624 | 11 | yes |
| mm/memblock.rs | `memblock_mut` | 635 | 8 | yes |
| mm/memblock.rs | `memblock_phys_alloc` | 643 | 20 | yes |
| mm/meminfo.rs | `default` | 78 | 26 | yes |
| mm/meminfo.rs | `format` | 104 | 11 |  |
| mm/meminfo.rs | `fmt` | 115 | 34 | yes |
| mm/meminfo.rs | `get_basic_memory_info` | 149 | 27 |  |
| mm/meminfo.rs | `get_memory_info` | 176 | 33 |  |
| mm/meminfo.rs | `print_memory_info` | 209 | 19 |  |
| mm/meminfo.rs | `get_memory_summary` | 228 | 22 |  |
| mm/meminfo.rs | `is_memory_low` | 250 | 12 |  |
| mm/meminfo.rs | `should_trigger_oom` | 262 | 10 |  |
| mm/mm_hooks.rs | `no_policy_handle_mm_fault` | 117 | 3 |  |
| mm/mm_hooks.rs | `no_policy_mmap` | 120 | 11 |  |
| mm/mm_hooks.rs | `no_policy_munmap` | 131 | 3 |  |
| mm/mm_hooks.rs | `no_policy_find_free_area` | 134 | 3 |  |
| mm/mm_hooks.rs | `no_policy_zap_page_range` | 137 | 3 |  |
| mm/mm_hooks.rs | `no_policy_fork` | 140 | 11 |  |
| mm/mm_hooks.rs | `mm_register_hooks` | 151 | 5 |  |
| mm/mm_hooks.rs | `hooks` | 156 | 7 |  |
| mm/mm_hooks.rs | `handle_mm_fault` | 163 | 5 | yes |
| mm/mm_hooks.rs | `mmap` | 168 | 13 |  |
| mm/mm_hooks.rs | `munmap` | 181 | 5 |  |
| mm/mm_hooks.rs | `find_free_area` | 186 | 5 |  |
| mm/mm_hooks.rs | `zap_page_range` | 191 | 9 |  |
| mm/mm_hooks.rs | `fork` | 200 | 36 |  |
| mm/mm_hooks.rs | `make_swap_entry` | 236 | 8 |  |
| mm/mm_hooks.rs | `is_swap_entry` | 244 | 6 |  |
| mm/mm_hooks.rs | `make_migration_entry` | 250 | 6 |  |
| mm/mm_hooks.rs | `is_migration_entry` | 256 | 6 |  |
| mm/mm_hooks.rs | `swap_entry_type` | 262 | 6 |  |
| mm/mm_hooks.rs | `swap_entry_offset` | 268 | 105 |  |
| mm/mm_hooks.rs | `no_policy_kswapd_init` | 373 | 3 |  |
| mm/mm_hooks.rs | `no_policy_kswapd_enable_oom` | 376 | 3 |  |
| mm/mm_hooks.rs | `no_policy_wakeup_kswapd` | 379 | 3 |  |
| mm/mm_hooks.rs | `no_policy_try_to_free_pages` | 382 | 3 |  |
| mm/mm_hooks.rs | `no_policy_drain_file_cache_before_oom` | 385 | 3 |  |
| mm/mm_hooks.rs | `no_policy_vscan_heartbeat` | 388 | 3 |  |
| mm/mm_hooks.rs | `no_policy_vscan_census_line` | 391 | 3 |  |
| mm/mm_hooks.rs | `no_policy_zone_dirty_writeback_state` | 394 | 3 |  |
| mm/mm_hooks.rs | `no_policy_zone_below_dirty_writeback_mark` | 397 | 3 |  |
| mm/mm_hooks.rs | `no_policy_fault_oom_kill_or_retry` | 400 | 3 |  |
| mm/mm_hooks.rs | `no_policy_kernel_fault_oom_kill` | 403 | 3 |  |
| mm/mm_hooks.rs | `no_policy_sysrq_oom_kill` | 406 | 3 |  |
| mm/mm_hooks.rs | `no_policy_oom_badness` | 409 | 3 |  |
| mm/mm_hooks.rs | `no_policy_migration_in_progress` | 412 | 3 |  |
| mm/mm_hooks.rs | `no_policy_compact_zone` | 415 | 3 |  |
| mm/mm_hooks.rs | `no_policy_swap_init` | 418 | 3 |  |
| mm/mm_hooks.rs | `no_policy_swap_activate` | 421 | 3 |  |
| mm/mm_hooks.rs | `no_policy_swap_deactivate` | 424 | 3 |  |
| mm/mm_hooks.rs | `no_policy_swap_boot_disk` | 427 | 3 |  |
| mm/mm_hooks.rs | `no_policy_nr_active_swap` | 430 | 3 |  |
| mm/mm_hooks.rs | `no_policy_swap_free_slot` | 433 | 3 |  |
| mm/mm_hooks.rs | `no_policy_swap_stats` | 436 | 3 |  |
| mm/mm_hooks.rs | `no_policy_swap_write_census` | 439 | 3 |  |
| mm/mm_hooks.rs | `no_policy_page_add_anon_lru` | 442 | 3 |  |
| mm/mm_hooks.rs | `no_policy_page_remove_lru` | 445 | 3 |  |
| mm/mm_hooks.rs | `no_policy_page_add_file_lru` | 448 | 3 |  |
| mm/mm_hooks.rs | `no_policy_page_add_file_lru_young` | 451 | 3 |  |
| mm/mm_hooks.rs | `no_policy_page_add_file_lru_young_batch` | 454 | 3 |  |
| mm/mm_hooks.rs | `no_policy_lru_activate_young` | 457 | 3 |  |
| mm/mm_hooks.rs | `no_policy_lru_tail` | 460 | 3 |  |
| mm/mm_hooks.rs | `no_policy_lru_demote_active_file_oldest` | 463 | 9 |  |
| mm/mm_hooks.rs | `mm_register_policy_hooks` | 472 | 5 |  |
| mm/mm_hooks.rs | `policy_hooks` | 477 | 9 |  |
| mm/mm_hooks.rs | `register_in_tree_mm_policy` | 486 | 45 |  |
| mm/mm_hooks.rs | `kswapd_init` | 531 | 4 |  |
| mm/mm_hooks.rs | `kswapd_enable_oom` | 535 | 4 |  |
| mm/mm_hooks.rs | `wakeup_kswapd` | 539 | 4 | yes |
| mm/mm_hooks.rs | `try_to_free_pages` | 543 | 4 | yes |
| mm/mm_hooks.rs | `drain_file_cache_before_oom` | 547 | 4 | yes |
| mm/mm_hooks.rs | `vscan_heartbeat` | 551 | 4 | yes |
| mm/mm_hooks.rs | `vscan_census_line` | 555 | 4 | yes |
| mm/mm_hooks.rs | `zone_dirty_writeback_state` | 559 | 4 |  |
| mm/mm_hooks.rs | `zone_below_dirty_writeback_mark` | 563 | 4 |  |
| mm/mm_hooks.rs | `fault_oom_kill_or_retry` | 567 | 4 | yes |
| mm/mm_hooks.rs | `kernel_fault_oom_kill` | 571 | 4 | yes |
| mm/mm_hooks.rs | `sysrq_oom_kill` | 575 | 4 |  |
| mm/mm_hooks.rs | `oom_badness` | 579 | 4 |  |
| mm/mm_hooks.rs | `migration_in_progress` | 583 | 4 |  |
| mm/mm_hooks.rs | `compact_zone` | 587 | 4 | yes |
| mm/mm_hooks.rs | `swap_init` | 591 | 4 |  |
| mm/mm_hooks.rs | `swap_activate` | 595 | 4 |  |
| mm/mm_hooks.rs | `swap_deactivate` | 599 | 4 |  |
| mm/mm_hooks.rs | `swap_boot_disk` | 603 | 4 |  |
| mm/mm_hooks.rs | `nr_active_swap` | 607 | 4 | yes |
| mm/mm_hooks.rs | `swap_free_slot` | 611 | 4 |  |
| mm/mm_hooks.rs | `swap_stats` | 615 | 4 |  |
| mm/mm_hooks.rs | `swap_write_census` | 619 | 4 | yes |
| mm/mm_hooks.rs | `page_add_anon_lru` | 623 | 4 |  |
| mm/mm_hooks.rs | `page_remove_lru` | 627 | 4 | yes |
| mm/mm_hooks.rs | `page_add_file_lru` | 631 | 4 |  |
| mm/mm_hooks.rs | `page_add_file_lru_young` | 635 | 4 | yes |
| mm/mm_hooks.rs | `page_add_file_lru_young_batch` | 639 | 4 | yes |
| mm/mm_hooks.rs | `lru_activate_young` | 643 | 4 | yes |
| mm/mm_hooks.rs | `lru_tail` | 647 | 4 | yes |
| mm/mm_hooks.rs | `lru_demote_active_file_oldest` | 651 | 3 | yes |
| mm/mm_struct.rs | `new` | 70 | 6 | yes |
| mm/mm_struct.rs | `lock` | 76 | 6 | yes |
| mm/mm_struct.rs | `lock_irqsave` | 82 | 14 | yes |
| mm/mm_struct.rs | `new` | 96 | 17 | yes |
| mm/mm_struct.rs | `lock` | 113 | 9 | yes |
| mm/mm_struct.rs | `lock_irqsave` | 122 | 260 | yes |
| mm/mm_struct.rs | `new` | 382 | 89 | yes |
| mm/mm_struct.rs | `new_shared` | 471 | 10 |  |
| mm/mm_struct.rs | `new_with_type` | 481 | 5 |  |
| mm/mm_struct.rs | `new_kernel` | 486 | 5 |  |
| mm/mm_struct.rs | `new_user` | 491 | 10 | yes |
| mm/mm_struct.rs | `set_exe_text_key` | 501 | 7 |  |
| mm/mm_struct.rs | `inherit_exe_text` | 508 | 11 |  |
| mm/mm_struct.rs | `pgd` | 519 | 6 | yes |
| mm/mm_struct.rs | `root_ppn` | 525 | 6 | yes |
| mm/mm_struct.rs | `asid` | 531 | 6 | yes |
| mm/mm_struct.rs | `set_asid` | 537 | 9 |  |
| mm/mm_struct.rs | `note_tlb_cpu` | 546 | 24 | yes |
| mm/mm_struct.rs | `clear_tlb_cpu` | 570 | 11 | yes |
| mm/mm_struct.rs | `tlb_cpu_mask` | 581 | 12 |  |
| mm/mm_struct.rs | `alloc_asid` | 593 | 25 | yes |
| mm/mm_struct.rs | `free_asid` | 618 | 18 |  |
| mm/mm_struct.rs | `pte_lock` | 636 | 22 | yes |
| mm/mm_struct.rs | `mm_surgery_guard` | 658 | 8 |  |
| mm/mm_struct.rs | `space_type` | 666 | 8 |  |
| mm/mm_struct.rs | `start_code` | 674 | 6 |  |
| mm/mm_struct.rs | `set_start_code` | 680 | 6 |  |
| mm/mm_struct.rs | `end_code` | 686 | 6 |  |
| mm/mm_struct.rs | `set_end_code` | 692 | 6 |  |
| mm/mm_struct.rs | `start_data` | 698 | 6 |  |
| mm/mm_struct.rs | `set_start_data` | 704 | 6 |  |
| mm/mm_struct.rs | `end_data` | 710 | 6 |  |
| mm/mm_struct.rs | `set_end_data` | 716 | 8 |  |
| mm/mm_struct.rs | `start_brk` | 724 | 6 |  |
| mm/mm_struct.rs | `set_start_brk` | 730 | 13 |  |
| mm/mm_struct.rs | `brk` | 743 | 6 |  |
| mm/mm_struct.rs | `set_brk_val` | 749 | 9 |  |
| mm/mm_struct.rs | `brk_lock` | 758 | 8 |  |
| mm/mm_struct.rs | `start_stack` | 766 | 6 |  |
| mm/mm_struct.rs | `set_start_stack` | 772 | 6 |  |
| mm/mm_struct.rs | `stack_limit` | 778 | 6 |  |
| mm/mm_struct.rs | `set_stack_limit` | 784 | 8 |  |
| mm/mm_struct.rs | `arg_start` | 792 | 6 |  |
| mm/mm_struct.rs | `set_arg_start` | 798 | 6 |  |
| mm/mm_struct.rs | `arg_end` | 804 | 6 |  |
| mm/mm_struct.rs | `set_arg_end` | 810 | 6 |  |
| mm/mm_struct.rs | `env_start` | 816 | 6 |  |
| mm/mm_struct.rs | `set_env_start` | 822 | 6 |  |
| mm/mm_struct.rs | `env_end` | 828 | 6 |  |
| mm/mm_struct.rs | `set_env_end` | 834 | 8 |  |
| mm/mm_struct.rs | `total_vm` | 842 | 6 |  |
| mm/mm_struct.rs | `add_total_vm` | 848 | 11 |  |
| mm/mm_struct.rs | `sub_total_vm` | 859 | 19 |  |
| mm/mm_struct.rs | `release_unmapped_pages` | 878 | 17 |  |
| mm/mm_struct.rs | `locked_vm` | 895 | 6 |  |
| mm/mm_struct.rs | `pinned_vm` | 901 | 6 |  |
| mm/mm_struct.rs | `data_vm` | 907 | 6 |  |
| mm/mm_struct.rs | `exec_vm` | 913 | 6 |  |
| mm/mm_struct.rs | `stack_vm` | 919 | 8 |  |
| mm/mm_struct.rs | `rss` | 927 | 6 |  |
| mm/mm_struct.rs | `add_rss` | 933 | 7 |  |
| mm/mm_struct.rs | `sub_rss` | 940 | 12 |  |
| mm/mm_struct.rs | `mmap_base` | 952 | 6 |  |
| mm/mm_struct.rs | `set_mmap_base` | 958 | 6 |  |
| mm/mm_struct.rs | `highest_vm_end` | 964 | 9 |  |
| mm/mm_struct.rs | `update_highest_vm_end` | 973 | 20 |  |
| mm/mm_struct.rs | `mm_users_inc` | 993 | 12 |  |
| mm/mm_struct.rs | `mm_users_dec` | 1005 | 21 |  |
| mm/mm_struct.rs | `mm_users_dec_and_test` | 1026 | 6 |  |
| mm/mm_struct.rs | `mm_users` | 1032 | 6 |  |
| mm/mm_struct.rs | `mm_count_inc` | 1038 | 8 |  |
| mm/mm_struct.rs | `mm_count_dec` | 1046 | 21 |  |
| mm/mm_struct.rs | `mm_count_dec_and_test` | 1067 | 6 |  |
| mm/mm_struct.rs | `mm_count` | 1073 | 8 |  |
| mm/mm_struct.rs | `flags` | 1081 | 6 | yes |
| mm/mm_struct.rs | `set_flags` | 1087 | 6 | yes |
| mm/mm_struct.rs | `has_flag` | 1093 | 8 |  |
| mm/mm_struct.rs | `owner_pid` | 1101 | 6 |  |
| mm/mm_struct.rs | `set_owner_pid` | 1107 | 11 |  |
| mm/mm_struct.rs | `vma_read` | 1118 | 10 | yes |
| mm/mm_struct.rs | `vma_try_read` | 1128 | 10 |  |
| mm/mm_struct.rs | `vma_rwsem_diag` | 1138 | 7 |  |
| mm/mm_struct.rs | `pin_vma_file` | 1145 | 5 |  |
| mm/mm_struct.rs | `unpin_vma_file` | 1150 | 6 |  |
| mm/mm_struct.rs | `get_vma_file` | 1156 | 7 | yes |
| mm/mm_struct.rs | `try_get_vma_file` | 1163 | 13 |  |
| mm/mm_struct.rs | `vma_write` | 1176 | 5 |  |
| mm/mm_struct.rs | `find_vma` | 1181 | 6 | yes |
| mm/mm_struct.rs | `add_vma` | 1187 | 16 |  |
| mm/mm_struct.rs | `remove_vma` | 1203 | 19 |  |
| mm/mm_struct.rs | `setup_segment_layout` | 1222 | 28 |  |
| mm/mm_struct.rs | `setup_stack` | 1250 | 12 |  |
| mm/mm_struct.rs | `setup_argv` | 1262 | 6 |  |
| mm/mm_struct.rs | `setup_envp` | 1268 | 9 |  |
| mm/mm_struct.rs | `set_saved_auxv` | 1277 | 9 |  |
| mm/mm_struct.rs | `saved_auxv` | 1286 | 22 |  |
| mm/mm_struct.rs | `drop` | 1308 | 179 | yes |
| mm/mod.rs | `alloc_and_map_user_table` | 118 | 20 | yes |
| mm/mod.rs | `alloc_and_map_user_memory` | 138 | 29 |  |
| mm/mod.rs | `switch_address_space` | 167 | 12 | yes |
| mm/mod.rs | `flush_tlb_page_local` | 179 | 62 |  |
| mm/page.rs | `new` | 30 | 4 | yes |
| mm/page.rs | `as_usize` | 34 | 4 | yes |
| mm/page.rs | `is_aligned` | 38 | 4 |  |
| mm/page.rs | `floor` | 42 | 4 |  |
| mm/page.rs | `ceil` | 46 | 4 |  |
| mm/page.rs | `frame_number` | 50 | 5 | yes |
| mm/page.rs | `ppn` | 55 | 6 | yes |
| mm/page.rs | `new` | 61 | 4 | yes |
| mm/page.rs | `as_usize` | 65 | 4 | yes |
| mm/page.rs | `is_aligned` | 69 | 4 |  |
| mm/page.rs | `floor` | 73 | 4 |  |
| mm/page.rs | `ceil` | 77 | 4 |  |
| mm/page.rs | `page_number` | 81 | 11 |  |
| mm/page.rs | `new` | 92 | 4 | yes |
| mm/page.rs | `containing_address` | 96 | 4 |  |
| mm/page.rs | `start_address` | 100 | 4 |  |
| mm/page.rs | `range` | 104 | 13 |  |
| mm/page.rs | `new` | 117 | 4 | yes |
| mm/page.rs | `containing_address` | 121 | 4 |  |
| mm/page.rs | `start_address` | 125 | 4 |  |
| mm/page.rs | `range` | 129 | 11 |  |
| mm/page.rs | `init_page_descriptors` | 140 | 6 |  |
| mm/page.rs | `frame_to_page` | 146 | 5 | yes |
| mm/page.rs | `frame_to_page_mut` | 151 | 3 | yes |
| mm/page_alloc.rs | `reclaim_gate_exit_release` | 56 | 92 | yes |
| mm/page_alloc.rs | `gate_release` | 148 | 26 | yes |
| mm/page_alloc.rs | `gate_holder_is_me` | 174 | 15 | yes |
| mm/page_alloc.rs | `gate_note_phase` | 189 | 11 | yes |
| mm/page_alloc.rs | `gate_note_round_done` | 200 | 10 | yes |
| mm/page_alloc.rs | `gate_note_deep_pass` | 210 | 14 | yes |
| mm/page_alloc.rs | `reclaim_gate_census` | 224 | 68 | yes |
| mm/page_alloc.rs | `gfp_reference_mask` | 292 | 32 | yes |
| mm/page_alloc.rs | `gfp_migratetype` | 324 | 13 | yes |
| mm/page_alloc.rs | `alloc_pages` | 337 | 48 | yes |
| mm/page_alloc.rs | `alloc_pages_inner` | 385 | 288 | yes |
| mm/page_alloc.rs | `alloc_page` | 673 | 22 | yes |
| mm/page_alloc.rs | `oom_forensic_dump` | 695 | 287 | yes |
| mm/page_alloc.rs | `alloc_page_batch` | 982 | 38 | yes |
| mm/page_alloc.rs | `get_zeroed_page` | 1020 | 41 | yes |
| mm/page_alloc.rs | `is_foreign_user_frame` | 1061 | 14 | yes |
| mm/page_alloc.rs | `report_free_refusal` | 1075 | 46 | yes |
| mm/page_alloc.rs | `pcache_leaf_phantom_zero_put` | 1121 | 41 | yes |
| mm/page_alloc.rs | `free_pages` | 1162 | 289 | yes |
| mm/page_alloc.rs | `free_page` | 1451 | 15 | yes |
| mm/page_alloc.rs | `virt_to_page` | 1466 | 5 | yes |
| mm/page_alloc.rs | `virt_to_pfn` | 1471 | 8 | yes |
| mm/page_alloc.rs | `page_to_phys` | 1479 | 5 | yes |
| mm/page_alloc.rs | `page_to_virt` | 1484 | 33 | yes |
| mm/page_alloc.rs | `init_zone_system` | 1517 | 136 | yes |
| mm/page_alloc.rs | `__get_free_pages` | 1653 | 9 | yes |
| mm/page_alloc.rs | `__get_free_page` | 1662 | 9 | yes |
| mm/page_alloc.rs | `__get_zeroed_page` | 1671 | 9 | yes |
| mm/page_alloc.rs | `__free_pages` | 1680 | 9 | yes |
| mm/page_alloc.rs | `__free_page` | 1689 | 3 | yes |
| mm/page_desc.rs | `new` | 113 | 5 | yes |
| mm/page_desc.rs | `from_raw` | 118 | 5 | yes |
| mm/page_desc.rs | `raw` | 123 | 5 | yes |
| mm/page_desc.rs | `test` | 128 | 5 | yes |
| mm/page_desc.rs | `set` | 133 | 5 | yes |
| mm/page_desc.rs | `clear` | 138 | 5 | yes |
| mm/page_desc.rs | `test_and_set` | 143 | 6 | yes |
| mm/page_desc.rs | `test_and_clear` | 149 | 6 | yes |
| mm/page_desc.rs | `clear_all` | 155 | 126 | yes |
| mm/page_desc.rs | `text_stamp_slot` | 281 | 14 | yes |
| mm/page_desc.rs | `text_stamp_insert` | 295 | 30 | yes |
| mm/page_desc.rs | `text_stamp_remove` | 325 | 17 | yes |
| mm/page_desc.rs | `text_stamp_remove_if_owner` | 342 | 21 | yes |
| mm/page_desc.rs | `new` | 363 | 16 | yes |
| mm/page_desc.rs | `text_stamp_note` | 379 | 14 | yes |
| mm/page_desc.rs | `text_stamp_get` | 393 | 19 | yes |
| mm/page_desc.rs | `new` | 412 | 20 | yes |
| mm/page_desc.rs | `init_reserved` | 432 | 6 | yes |
| mm/page_desc.rs | `init_free` | 438 | 28 | yes |
| mm/page_desc.rs | `flags` | 466 | 6 | yes |
| mm/page_desc.rs | `test_flag` | 472 | 6 | yes |
| mm/page_desc.rs | `set_flag` | 478 | 6 | yes |
| mm/page_desc.rs | `clear_all_flags` | 484 | 6 | yes |
| mm/page_desc.rs | `clear_flag` | 490 | 6 | yes |
| mm/page_desc.rs | `test_and_set_flag` | 496 | 6 | yes |
| mm/page_desc.rs | `test_and_clear_flag` | 502 | 6 | yes |
| mm/page_desc.rs | `is_locked` | 508 | 6 | yes |
| mm/page_desc.rs | `is_reserved` | 514 | 6 | yes |
| mm/page_desc.rs | `is_dirty` | 520 | 6 | yes |
| mm/page_desc.rs | `is_cow` | 526 | 6 | yes |
| mm/page_desc.rs | `is_anonymous` | 532 | 6 | yes |
| mm/page_desc.rs | `is_uptodate` | 538 | 8 | yes |
| mm/page_desc.rs | `refcount` | 546 | 7 | yes |
| mm/page_desc.rs | `get_page` | 553 | 12 | yes |
| mm/page_desc.rs | `put_page` | 565 | 28 | yes |
| mm/page_desc.rs | `put_page_site` | 593 | 14 | yes |
| mm/page_desc.rs | `try_get_page` | 607 | 20 | yes |
| mm/page_desc.rs | `set_refcount` | 627 | 8 | yes |
| mm/page_desc.rs | `mapcount` | 635 | 7 | yes |
| mm/page_desc.rs | `add_mapcount` | 642 | 7 | yes |
| mm/page_desc.rs | `sub_mapcount` | 649 | 6 | yes |
| mm/page_desc.rs | `is_mapped` | 655 | 6 | yes |
| mm/page_desc.rs | `inc_mapcount` | 661 | 6 | yes |
| mm/page_desc.rs | `dec_mapcount` | 667 | 6 | yes |
| mm/page_desc.rs | `reset_mapcount` | 673 | 8 | yes |
| mm/page_desc.rs | `private` | 681 | 6 | yes |
| mm/page_desc.rs | `set_private` | 687 | 8 | yes |
| mm/page_desc.rs | `mapping` | 695 | 6 | yes |
| mm/page_desc.rs | `set_mapping` | 701 | 6 | yes |
| mm/page_desc.rs | `index` | 707 | 6 | yes |
| mm/page_desc.rs | `set_index` | 713 | 13 | yes |
| mm/page_desc.rs | `page_type` | 726 | 13 | yes |
| mm/page_desc.rs | `set_page_type` | 739 | 8 | yes |
| mm/page_desc.rs | `lru_next` | 747 | 6 | yes |
| mm/page_desc.rs | `set_lru_next` | 753 | 6 | yes |
| mm/page_desc.rs | `lru_prev` | 759 | 6 | yes |
| mm/page_desc.rs | `set_lru_prev` | 765 | 7 | yes |
| mm/page_desc.rs | `set_lru_list` | 772 | 8 | yes |
| mm/page_desc.rs | `lru_list` | 780 | 6 | yes |
| mm/page_desc.rs | `clear_lru_list` | 786 | 12 | yes |
| mm/page_desc.rs | `rmap_alt_add` | 798 | 14 | yes |
| mm/page_desc.rs | `rmap_alt_remove` | 812 | 13 | yes |
| mm/page_desc.rs | `rmap_alt_clear` | 825 | 8 | yes |
| mm/page_desc.rs | `rmap_alt_for_each` | 833 | 14 |  |
| mm/page_desc.rs | `next_free` | 847 | 6 | yes |
| mm/page_desc.rs | `set_next_free` | 853 | 8 | yes |
| mm/page_desc.rs | `order` | 861 | 6 | yes |
| mm/page_desc.rs | `set_order` | 867 | 6 | yes |
| mm/page_desc.rs | `is_free` | 873 | 16 | yes |
| mm/page_desc.rs | `copy_page_contents` | 889 | 55 | yes |
| mm/page_desc.rs | `descs_published` | 944 | 6 | yes |
| mm/page_desc.rs | `set_nr_descs` | 950 | 10 | yes |
| mm/page_desc.rs | `nr_descs` | 960 | 13 | yes |
| mm/page_desc.rs | `max_pfn` | 973 | 9 | yes |
| mm/page_desc.rs | `pfn_valid` | 982 | 6 | yes |
| mm/page_desc.rs | `phys_valid` | 988 | 13 | yes |
| mm/page_desc.rs | `set_present_pages` | 1001 | 7 | yes |
| mm/page_desc.rs | `present_pages` | 1008 | 19 | yes |
| mm/page_desc.rs | `mark_non_ram_pages` | 1027 | 56 | yes |
| mm/page_desc.rs | `init_mem_map` | 1083 | 65 | yes |
| mm/page_desc.rs | `pfn_to_page` | 1148 | 20 | yes |
| mm/page_desc.rs | `pfn_to_page_mut` | 1168 | 27 | yes |
| mm/page_desc.rs | `page_to_pfn` | 1195 | 9 | yes |
| mm/page_desc.rs | `phys_to_page` | 1204 | 6 | yes |
| mm/page_desc.rs | `phys_to_page_mut` | 1210 | 6 | yes |
| mm/page_desc.rs | `frame_to_page` | 1216 | 6 | yes |
| mm/page_desc.rs | `frame_to_page_mut` | 1222 | 8 | yes |
| mm/page_desc.rs | `total_pages` | 1230 | 6 | yes |
| mm/page_desc.rs | `page_size` | 1236 | 26 | yes |
| mm/page_desc.rs | `page_desc_stats` | 1262 | 67 | yes |
| mm/page_desc.rs | `get_page` | 1329 | 9 | yes |
| mm/page_desc.rs | `get_page_mut` | 1338 | 15 | yes |
| mm/page_desc.rs | `page_desc` | 1353 | 11 | yes |
| mm/pagemap.rs | `from` | 40 | 37 | yes |
| mm/pcp.rs | `new` | 67 | 33 | yes |
| mm/pcp.rs | `with_row` | 100 | 25 |  |
| mm/pcp.rs | `alloc_order0` | 125 | 82 | yes |
| mm/pcp.rs | `alloc_order0_batch` | 207 | 33 | yes |
| mm/pcp.rs | `free_order0` | 240 | 136 | yes |
| mm/pcp.rs | `to_cached` | 376 | 13 |  |
| mm/pcp.rs | `to_in_use` | 389 | 16 |  |
| mm/pcp.rs | `cached_pages_total` | 405 | 12 |  |
| mm/pcp.rs | `cached_pages_per_cpu` | 417 | 16 |  |
| mm/pcp.rs | `forensics_scan_rows` | 433 | 24 |  |
| mm/pglist.rs | `new` | 99 | 32 | yes |
| mm/pglist.rs | `init` | 131 | 8 | yes |
| mm/pglist.rs | `add_zone` | 139 | 9 | yes |
| mm/pglist.rs | `zone` | 148 | 5 | yes |
| mm/pglist.rs | `zone_mut` | 153 | 5 | yes |
| mm/pglist.rs | `zone_at` | 158 | 9 |  |
| mm/pglist.rs | `node_id` | 167 | 5 |  |
| mm/pglist.rs | `nr_zones` | 172 | 5 |  |
| mm/pglist.rs | `start_pfn` | 177 | 5 | yes |
| mm/pglist.rs | `spanned_pages` | 182 | 5 |  |
| mm/pglist.rs | `present_pages` | 187 | 5 | yes |
| mm/pglist.rs | `free_pages` | 192 | 13 | yes |
| mm/pglist.rs | `is_initialized` | 205 | 5 | yes |
| mm/pglist.rs | `stats` | 210 | 46 |  |
| mm/pglist.rs | `init_node_data` | 256 | 9 | yes |
| mm/pglist.rs | `node_data` | 265 | 13 |  |
| mm/pglist.rs | `node_data_mut` | 278 | 10 | yes |
| mm/pglist.rs | `first_online_node` | 288 | 12 | yes |
| mm/pglist.rs | `first_online_node_mut` | 300 | 5 | yes |
| mm/pglist.rs | `num_online_nodes` | 305 | 14 |  |
| mm/pglist.rs | `select_zone` | 319 | 6 |  |
| mm/pglist.rs | `select_zone_mut` | 325 | 8 |  |
| mm/pglist.rs | `print_buddyinfo` | 333 | 29 |  |
| mm/pglist.rs | `print_zoneinfo` | 362 | 28 |  |
| mm/policy/aarch64.rs | `try_expand_stack` | 35 | 155 |  |
| mm/policy/aarch64.rs | `protnone_represent` | 190 | 96 |  |
| mm/policy/aarch64.rs | `handle_mm_fault` | 286 | 308 | yes |
| mm/policy/aarch64.rs | `map_vma` | 594 | 23 |  |
| mm/policy/aarch64.rs | `set_brk` | 617 | 120 |  |
| mm/policy/aarch64.rs | `mmap` | 737 | 165 |  |
| mm/policy/aarch64.rs | `find_free_area` | 902 | 7 |  |
| mm/policy/aarch64.rs | `find_free_area_locked` | 909 | 44 |  |
| mm/policy/aarch64.rs | `munmap` | 953 | 143 |  |
| mm/policy/aarch64.rs | `zap_page_range` | 1096 | 12 |  |
| mm/policy/aarch64.rs | `do_brk` | 1108 | 5 |  |
| mm/policy/aarch64.rs | `allocate_stack` | 1113 | 26 |  |
| mm/policy/aarch64.rs | `fork` | 1139 | 120 |  |
| mm/policy/riscv64.rs | `try_expand_stack` | 47 | 222 |  |
| mm/policy/riscv64.rs | `pte_counts_as_mapped` | 269 | 5 |  |
| mm/policy/riscv64.rs | `handle_mm_fault` | 274 | 770 | yes |
| mm/policy/riscv64.rs | `anon_faultaround_forward` | 1044 | 80 |  |
| mm/policy/riscv64.rs | `is_protnone_marker` | 1124 | 20 |  |
| mm/policy/riscv64.rs | `handle_swap_fault` | 1144 | 158 |  |
| mm/policy/riscv64.rs | `map_vma` | 1302 | 22 |  |
| mm/policy/riscv64.rs | `set_brk` | 1324 | 130 |  |
| mm/policy/riscv64.rs | `mmap` | 1454 | 219 |  |
| mm/policy/riscv64.rs | `find_free_area` | 1673 | 7 |  |
| mm/policy/riscv64.rs | `find_free_area_locked` | 1680 | 45 |  |
| mm/policy/riscv64.rs | `munmap` | 1725 | 175 |  |
| mm/policy/riscv64.rs | `zap_page_range` | 1900 | 14 |  |
| mm/policy/riscv64.rs | `munmap_legacy` | 1914 | 45 |  |
| mm/policy/riscv64.rs | `set_range_permissions` | 1959 | 54 |  |
| mm/policy/riscv64.rs | `do_brk` | 2013 | 5 |  |
| mm/policy/riscv64.rs | `allocate_stack` | 2018 | 23 |  |
| mm/policy/riscv64.rs | `fork` | 2041 | 125 |  |
| mm/policy/x86_64.rs | `try_expand_stack` | 84 | 200 |  |
| mm/policy/x86_64.rs | `pte_counts_as_mapped` | 284 | 5 |  |
| mm/policy/x86_64.rs | `handle_mm_fault` | 289 | 1625 | yes |
| mm/policy/x86_64.rs | `is_protnone_marker` | 1914 | 18 |  |
| mm/policy/x86_64.rs | `handle_swap_fault` | 1932 | 156 |  |
| mm/policy/x86_64.rs | `map_vma` | 2088 | 23 |  |
| mm/policy/x86_64.rs | `set_brk` | 2111 | 123 |  |
| mm/policy/x86_64.rs | `mmap` | 2234 | 323 |  |
| mm/policy/x86_64.rs | `find_free_area_32bit` | 2557 | 8 |  |
| mm/policy/x86_64.rs | `find_free_area_32bit_locked` | 2565 | 37 |  |
| mm/policy/x86_64.rs | `find_free_area` | 2602 | 9 |  |
| mm/policy/x86_64.rs | `find_free_area_locked` | 2611 | 42 |  |
| mm/policy/x86_64.rs | `munmap` | 2653 | 268 |  |
| mm/policy/x86_64.rs | `zap_page_range` | 2921 | 16 |  |
| mm/policy/x86_64.rs | `remove_vma_piece` | 2937 | 71 |  |
| mm/policy/x86_64.rs | `range_is_pte_empty` | 3008 | 155 |  |
| mm/policy/x86_64.rs | `set_range_permissions` | 3163 | 72 |  |
| mm/policy/x86_64.rs | `do_brk` | 3235 | 5 |  |
| mm/policy/x86_64.rs | `allocate_stack` | 3240 | 23 |  |
| mm/policy/x86_64.rs | `fork` | 3263 | 160 |  |
| mm/rmap.rs | `new` | 51 | 10 | yes |
| mm/rmap.rs | `get` | 61 | 6 | yes |
| mm/rmap.rs | `put` | 67 | 5 | yes |
| mm/rmap.rs | `refcount` | 72 | 5 | yes |
| mm/rmap.rs | `set_vma` | 77 | 5 |  |
| mm/rmap.rs | `get_vma` | 82 | 24 |  |
| mm/rmap.rs | `page_add_anon_rmap` | 106 | 30 |  |
| mm/rmap.rs | `page_add_file_rmap` | 136 | 23 |  |
| mm/rmap.rs | `page_remove_rmap` | 159 | 29 |  |
| mm/rmap.rs | `page_mapped` | 188 | 10 |  |
| mm/rmap.rs | `page_get_mappings` | 198 | 15 |  |
| mm/rmap.rs | `page_referenced` | 213 | 6 |  |
| mm/rmap.rs | `page_clear_referenced` | 219 | 11 |  |
| mm/rmap.rs | `try_to_unmap` | 230 | 11 |  |
| mm/rmap.rs | `try_to_unmap_with_swap` | 241 | 13 |  |
| mm/rmap.rs | `page_record_mapping` | 254 | 55 |  |
| mm/rmap.rs | `try_to_unmap_migration` | 309 | 8 |  |
| mm/rmap.rs | `try_to_unmap_inner` | 317 | 203 |  |
| mm/rmap.rs | `rmap_stats` | 520 | 4 |  |
| mm/slab.rs | `new` | 79 | 34 | yes |
| mm/slab.rs | `new` | 113 | 23 | yes |
| mm/slab.rs | `alloc` | 136 | 43 | yes |
| mm/slab.rs | `alloc_from_slab` | 179 | 64 | yes |
| mm/slab.rs | `free` | 243 | 77 | yes |
| mm/slab.rs | `create_slab` | 320 | 43 | yes |
| mm/slab.rs | `move_slab_to_full` | 363 | 25 | yes |
| mm/slab.rs | `move_slab_from_full` | 388 | 49 | yes |
| mm/slab.rs | `new` | 437 | 9 | yes |
| mm/slab.rs | `base_addr` | 446 | 5 | yes |
| mm/slab.rs | `max_pages` | 451 | 5 | yes |
| mm/slab.rs | `alloc_page` | 456 | 10 | yes |
| mm/slab.rs | `get_page_addr` | 466 | 5 | yes |
| mm/slab.rs | `get_header_mut` | 471 | 9 | yes |
| mm/slab.rs | `get_next` | 480 | 5 | yes |
| mm/slab.rs | `set_next` | 485 | 5 | yes |
| mm/slab.rs | `get_prev` | 490 | 5 | yes |
| mm/slab.rs | `set_prev` | 495 | 5 | yes |
| mm/slab.rs | `find_slab_by_addr` | 500 | 26 | yes |
| mm/slab.rs | `slab_pages_cell` | 526 | 41 | yes |
| mm/slab.rs | `init` | 567 | 8 | yes |
| mm/slab.rs | `is_initialized` | 575 | 5 | yes |
| mm/slab.rs | `pages` | 580 | 7 | yes |
| mm/slab.rs | `find_cache_index` | 587 | 23 | yes |
| mm/slab.rs | `audit_cache_locks` | 610 | 10 | yes |
| mm/slab.rs | `kmalloc` | 620 | 36 | yes |
| mm/slab.rs | `kfree` | 656 | 46 | yes |
| mm/slab.rs | `kzalloc` | 702 | 15 | yes |
| mm/slab.rs | `init_slab` | 717 | 6 | yes |
| mm/slab.rs | `is_slab_initialized` | 723 | 10 | yes |
| mm/slab.rs | `slab_region` | 733 | 11 | yes |
| mm/slab.rs | `slab_stats` | 744 | 40 | yes |
| mm/small_alloc.rs | `hdr_of` | 86 | 10 | yes |
| mm/small_alloc.rs | `first_off` | 96 | 5 | yes |
| mm/small_alloc.rs | `class_of` | 101 | 14 | yes |
| mm/small_alloc.rs | `effective_size` | 115 | 15 | yes |
| mm/small_alloc.rs | `bmp_set` | 130 | 6 | yes |
| mm/small_alloc.rs | `bmp_clear` | 136 | 7 | yes |
| mm/small_alloc.rs | `is_slab_ptr` | 143 | 16 | yes |
| mm/small_alloc.rs | `object_span` | 159 | 53 | yes |
| mm/small_alloc.rs | `new` | 212 | 99 | yes |
| mm/small_alloc.rs | `alloc` | 311 | 128 | yes |
| mm/small_alloc.rs | `alloc_locked` | 439 | 116 | yes |
| mm/small_alloc.rs | `free` | 555 | 160 | yes |
| mm/small_alloc.rs | `flush_batch` | 715 | 108 | yes |
| mm/small_alloc.rs | `live_stats_kib` | 823 | 25 | yes |
| mm/small_alloc.rs | `audit_section_peek` | 848 | 16 | yes |
| mm/small_alloc.rs | `audit_class_locks` | 864 | 10 | yes |
| mm/small_alloc.rs | `census_peek` | 874 | 97 | yes |
| mm/small_alloc.rs | `census_peek3` | 971 | 16 | yes |
| mm/small_alloc.rs | `is_kheap` | 987 | 7 | yes |
| mm/small_alloc.rs | `is_userva` | 994 | 78 | yes |
| mm/small_alloc.rs | `census_line` | 1072 | 31 | yes |
| mm/unwatch.rs | `note` | 72 | 16 |  |
| mm/unwatch.rs | `dump_tail` | 88 | 36 | yes |
| mm/unwatch.rs | `covered_by_unmap` | 124 | 32 | yes |
| mm/vdso.rs | `vdso_ehdr` | 80 | 28 |  |
| mm/vdso.rs | `vdso_seq` | 108 | 9 |  |
| mm/vdso.rs | `vdso_data_tick` | 117 | 89 | yes |
| mm/vdso.rs | `bump` | 206 | 9 | yes |
| mm/vdso.rs | `bump_pub` | 215 | 10 |  |
| mm/vdso.rs | `rd` | 225 | 8 |  |
| mm/vdso.rs | `wr` | 233 | 14 |  |
| mm/vdso.rs | `read_tsc_ordered` | 247 | 30 |  |
| mm/vdso.rs | `tsc_delta_nominal` | 277 | 11 |  |
| mm/vdso.rs | `seq_try_begin` | 288 | 12 |  |
| mm/vdso.rs | `seq_end` | 300 | 8 |  |
| mm/vdso.rs | `publish_tsc_anchor` | 308 | 19 |  |
| mm/vdso.rs | `tsc_anchor_step_forward` | 327 | 20 |  |
| mm/vdso.rs | `tsc_anchor_step_back` | 347 | 18 |  |
| mm/vdso.rs | `tsc_anchor_peek` | 365 | 41 |  |
| mm/vdso.rs | `tsc_track_now_nominal` | 406 | 42 |  |
| mm/vdso.rs | `vdso_data_tick` | 448 | 47 | yes |
| mm/vdso.rs | `vdso_pages_phys` | 495 | 25 |  |
| mm/vdso.rs | `vdso_pages_phys` | 520 | 10 |  |
| mm/vdso.rs | `is_vdso_phys` | 530 | 31 |  |
| mm/vdso.rs | `new` | 561 | 7 | yes |
| mm/vdso.rs | `label` | 568 | 3 |  |
| mm/vdso.rs | `emit` | 571 | 4 | yes |
| mm/vdso.rs | `br` | 575 | 5 |  |
| mm/vdso.rs | `jal_rel` | 580 | 4 |  |
| mm/vdso.rs | `finish` | 584 | 12 |  |
| mm/vdso.rs | `r_type` | 596 | 3 |  |
| mm/vdso.rs | `i_type` | 599 | 3 |  |
| mm/vdso.rs | `s_type` | 602 | 5 |  |
| mm/vdso.rs | `b_imm` | 607 | 4 |  |
| mm/vdso.rs | `j_imm` | 611 | 3 |  |
| mm/vdso.rs | `u_type` | 614 | 3 |  |
| mm/vdso.rs | `beq_skel` | 617 | 3 |  |
| mm/vdso.rs | `bne_skel` | 620 | 36 |  |
| mm/vdso.rs | `emit_li` | 656 | 19 |  |
| mm/vdso.rs | `emit_data_base` | 675 | 8 |  |
| mm/vdso.rs | `emit_seqlock_read` | 683 | 21 |  |
| mm/vdso.rs | `emit_interpolate` | 704 | 8 |  |
| mm/vdso.rs | `emit_ret` | 712 | 5 |  |
| mm/vdso.rs | `emit_clock_gettime` | 717 | 46 |  |
| mm/vdso.rs | `emit_gettimeofday` | 763 | 33 |  |
| mm/vdso.rs | `emit_clock_getres` | 796 | 69 |  |
| mm/vdso.rs | `new` | 865 | 8 | yes |
| mm/vdso.rs | `b` | 873 | 3 |  |
| mm/vdso.rs | `label` | 876 | 3 |  |
| mm/vdso.rs | `jcc` | 879 | 5 |  |
| mm/vdso.rs | `jmp32` | 884 | 5 |  |
| mm/vdso.rs | `jcc32` | 889 | 4 |  |
| mm/vdso.rs | `finish` | 893 | 25 |  |
| mm/vdso.rs | `lea_data_base` | 918 | 26 |  |
| mm/vdso.rs | `seqlock_read` | 944 | 28 |  |
| mm/vdso.rs | `interpolate` | 972 | 11 |  |
| mm/vdso.rs | `split_store` | 983 | 23 |  |
| mm/vdso.rs | `dispatch` | 1006 | 25 |  |
| mm/vdso.rs | `syscall_fallback` | 1031 | 7 |  |
| mm/vdso.rs | `emit_clock_gettime` | 1038 | 34 |  |
| mm/vdso.rs | `emit_gettimeofday` | 1072 | 40 |  |
| mm/vdso.rs | `emit_clock_getres` | 1112 | 32 |  |
| mm/vdso.rs | `elf_hash` | 1144 | 15 |  |
| mm/vdso.rs | `build_code_page` | 1159 | 327 |  |
| mm/vdso.rs | `vdso_init` | 1486 | 10 |  |
| mm/vma.rs | `new` | 70 | 5 | yes |
| mm/vma.rs | `from_bits` | 75 | 5 | yes |
| mm/vma.rs | `bits` | 80 | 5 | yes |
| mm/vma.rs | `contains` | 85 | 5 | yes |
| mm/vma.rs | `insert` | 90 | 5 | yes |
| mm/vma.rs | `remove` | 95 | 6 | yes |
| mm/vma.rs | `is_readable` | 101 | 6 | yes |
| mm/vma.rs | `is_writable` | 107 | 6 | yes |
| mm/vma.rs | `is_executable` | 113 | 6 | yes |
| mm/vma.rs | `is_shared` | 119 | 6 | yes |
| mm/vma.rs | `is_private` | 125 | 16 |  |
| mm/vma.rs | `to_page_perm` | 141 | 21 |  |
| mm/vma.rs | `default` | 162 | 95 | yes |
| mm/vma.rs | `from_bits` | 257 | 6 | yes |
| mm/vma.rs | `set_creator` | 263 | 6 |  |
| mm/vma.rs | `mark_forked` | 269 | 5 |  |
| mm/vma.rs | `bits` | 274 | 6 | yes |
| mm/vma.rs | `creator_name` | 280 | 18 | yes |
| mm/vma.rs | `is_forked` | 298 | 6 | yes |
| mm/vma.rs | `default` | 304 | 7 | yes |
| mm/vma.rs | `new` | 311 | 19 | yes |
| mm/vma.rs | `start` | 330 | 6 | yes |
| mm/vma.rs | `end` | 336 | 6 | yes |
| mm/vma.rs | `size` | 342 | 6 | yes |
| mm/vma.rs | `page_count` | 348 | 6 |  |
| mm/vma.rs | `flags` | 354 | 5 | yes |
| mm/vma.rs | `set_flags` | 359 | 6 | yes |
| mm/vma.rs | `vma_type` | 365 | 5 | yes |
| mm/vma.rs | `set_type` | 370 | 6 |  |
| mm/vma.rs | `contains` | 376 | 5 | yes |
| mm/vma.rs | `overlaps` | 381 | 6 |  |
| mm/vma.rs | `set_offset` | 387 | 6 |  |
| mm/vma.rs | `offset` | 393 | 5 | yes |
| mm/vma.rs | `set_file_fd` | 398 | 6 |  |
| mm/vma.rs | `file_fd` | 404 | 5 | yes |
| mm/vma.rs | `set_file_size` | 409 | 6 |  |
| mm/vma.rs | `file_size` | 415 | 6 |  |
| mm/vma.rs | `origin` | 421 | 6 | yes |
| mm/vma.rs | `set_origin` | 427 | 7 |  |
| mm/vma.rs | `split` | 434 | 48 | yes |
| mm/vma.rs | `can_merge` | 482 | 24 |  |
| mm/vma.rs | `merge` | 506 | 10 |  |
| mm/vma.rs | `merge_at_end` | 516 | 6 |  |
| mm/vma.rs | `fmt` | 522 | 26 | yes |
| mm/vma.rs | `new` | 548 | 23 | yes |
| mm/vma.rs | `add` | 571 | 107 | yes |
| mm/vma.rs | `find` | 678 | 20 | yes |
| mm/vma.rs | `find_mut` | 698 | 26 |  |
| mm/vma.rs | `remove` | 724 | 16 | yes |
| mm/vma.rs | `iter` | 740 | 7 | yes |
| mm/vma.rs | `iter_after` | 747 | 14 |  |
| mm/vma.rs | `last_at_or_before` | 761 | 10 |  |
| mm/vma.rs | `has_vma_starting_within` | 771 | 6 |  |
| mm/vma.rs | `count` | 777 | 5 | yes |
| mm/vma.rs | `get` | 782 | 5 | yes |
| mm/vma.rs | `get_mut` | 787 | 5 | yes |
| mm/vma.rs | `first` | 792 | 5 |  |
| mm/vma.rs | `last` | 797 | 5 | yes |
| mm/vma.rs | `find_vma_after` | 802 | 5 |  |
| mm/vma.rs | `clear` | 807 | 7 | yes |
| mm/vma.rs | `max_end` | 814 | 18 |  |
| mm/vma.rs | `expand_downwards` | 832 | 61 |  |
| mm/vma.rs | `find_stack_vma` | 893 | 23 | yes |
| mm/vma.rs | `default` | 916 | 31 | yes |
| mm/vma.rs | `user_start` | 947 | 3 |  |
| mm/vma.rs | `user_end` | 950 | 3 |  |
| mm/vma.rs | `default_stack_size` | 953 | 3 |  |
| mm/vma.rs | `default_stack_top` | 956 | 3 |  |
| mm/vma.rs | `heap_start` | 959 | 3 |  |
| mm/vma.rs | `heap_end` | 962 | 25 |  |
| mm/vma.rs | `user_start` | 987 | 6 |  |
| mm/vma.rs | `user_end` | 993 | 6 |  |
| mm/vma.rs | `default_stack_size` | 999 | 6 |  |
| mm/vma.rs | `default_stack_top` | 1005 | 6 |  |
| mm/vma.rs | `heap_start` | 1011 | 6 |  |
| mm/vma.rs | `heap_end` | 1017 | 4 |  |
| mm/vmemmap.rs | `pfn_to_vmemmap` | 54 | 14 |  |
| mm/vmemmap.rs | `vmemmap_to_pfn` | 68 | 10 |  |
| mm/vmemmap.rs | `is_vmemmap_initialized` | 78 | 16 |  |
| mm/vmemmap.rs | `init_vmemmap` | 94 | 116 |  |
| mm/vmemmap.rs | `vmemmap_stats` | 210 | 16 | yes |
| mm/vmstat.rs | `note_fault` | 48 | 8 | yes |
| mm/vmstat.rs | `note_major_fault` | 56 | 5 |  |
| mm/vmstat.rs | `note_free` | 61 | 5 | yes |
| mm/vmstat.rs | `note_alloc` | 66 | 11 | yes |
| mm/zone.rs | `name` | 71 | 11 | yes |
| mm/zone.rs | `index` | 82 | 22 | yes |
| mm/zone.rs | `new` | 104 | 8 | yes |
| mm/zone.rs | `is_empty` | 112 | 5 | yes |
| mm/zone.rs | `nr_free` | 117 | 5 | yes |
| mm/zone.rs | `inc_free` | 122 | 5 |  |
| mm/zone.rs | `dec_free` | 127 | 62 |  |
| mm/zone.rs | `new_free_areas` | 189 | 11 |  |
| mm/zone.rs | `new` | 200 | 26 | yes |
| mm/zone.rs | `init` | 226 | 24 | yes |
| mm/zone.rs | `is_initialized` | 250 | 5 | yes |
| mm/zone.rs | `zone_type` | 255 | 5 | yes |
| mm/zone.rs | `zone_id` | 260 | 5 |  |
| mm/zone.rs | `node_id` | 265 | 5 |  |
| mm/zone.rs | `start_pfn` | 270 | 5 | yes |
| mm/zone.rs | `end_pfn` | 275 | 5 | yes |
| mm/zone.rs | `spanned_pages` | 280 | 5 |  |
| mm/zone.rs | `present_pages` | 285 | 5 | yes |
| mm/zone.rs | `managed_pages` | 290 | 5 | yes |
| mm/zone.rs | `nr_free` | 295 | 6 | yes |
| mm/zone.rs | `free_pages_add` | 301 | 5 |  |
| mm/zone.rs | `free_pages_sub` | 306 | 7 |  |
| mm/zone.rs | `set_managed_pages` | 313 | 5 | yes |
| mm/zone.rs | `watermark` | 318 | 5 | yes |
| mm/zone.rs | `set_watermark` | 323 | 10 |  |
| mm/zone.rs | `watermark_ok` | 333 | 7 | yes |
| mm/zone.rs | `free_pages_order` | 340 | 14 |  |
| mm/zone.rs | `alloc_pages` | 354 | 37 | yes |
| mm/zone.rs | `alloc_pages_order0_batch` | 391 | 189 | yes |
| mm/zone.rs | `alloc_pages_locked` | 580 | 67 |  |
| mm/zone.rs | `alloc_from_order` | 647 | 67 |  |
| mm/zone.rs | `free_pages` | 714 | 162 | yes |
| mm/zone.rs | `find_buddy` | 876 | 13 |  |
| mm/zone.rs | `is_buddy_free` | 889 | 28 |  |
| mm/zone.rs | `add_to_free_list` | 917 | 29 | yes |
| mm/zone.rs | `add_to_free_list_init` | 946 | 20 | yes |
| mm/zone.rs | `remove_from_free_list` | 966 | 52 | yes |
| mm/zone.rs | `alloc_single_page` | 1018 | 8 |  |
| mm/zone.rs | `has_free_block` | 1026 | 16 |  |
| mm/zone.rs | `forensic_free_report` | 1042 | 31 | yes |
| mm/zone.rs | `stats` | 1073 | 77 |  |
| mm/zone.rs | `zone_type` | 1150 | 17 | yes |
| mm/zone.rs | `int_sqrt` | 1167 | 19 |  |
| mm/zone.rs | `setup_per_zone_wmarks` | 1186 | 53 | yes |
| mm/zone.rs | `pfn_to_phys` | 1239 | 5 | yes |
| mm/zone.rs | `phys_to_pfn` | 1244 | 5 | yes |
| mm/zone.rs | `print_zone_info` | 1249 | 40 |  |
