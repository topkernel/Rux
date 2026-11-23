# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch/aarch64 — hot 4068 lines (stay) / cold 2874 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/aarch64/boot.rs | `get_core_id` | 21 | 5 | yes |
| arch/aarch64/boot.rs | `get_dtb_pointer` | 26 | 5 | yes |
| arch/aarch64/context.rs | `set_prev_task` | 66 | 29 | yes |
| arch/aarch64/context.rs | `__switch_mm_linear` | 95 | 20 | yes |
| arch/aarch64/context.rs | `switch_mm` | 115 | 28 | yes |
| arch/aarch64/context.rs | `get_current_satp` | 143 | 13 | yes |
| arch/aarch64/context.rs | `__switch_to` | 156 | 8 | yes |
| arch/aarch64/context.rs | `context_switch` | 164 | 62 | yes |
| arch/aarch64/cpu.rs | `get_core_id` | 12 | 8 | yes |
| arch/aarch64/cpu.rs | `get_thread_id_of` | 20 | 10 | yes |
| arch/aarch64/cpu.rs | `get_thread_id` | 30 | 11 | yes |
| arch/aarch64/cpu.rs | `set_thread_id` | 41 | 11 | yes |
| arch/aarch64/cpu.rs | `get_counter_freq` | 52 | 11 | yes |
| arch/aarch64/cpu.rs | `read_counter` | 63 | 12 | yes |
| arch/aarch64/cpu.rs | `read_time` | 75 | 7 | yes |
| arch/aarch64/cpu.rs | `enable_irq` | 82 | 12 | yes |
| arch/aarch64/cpu.rs | `disable_irq` | 94 | 11 | yes |
| arch/aarch64/cpu.rs | `wfi` | 105 | 11 | yes |
| arch/aarch64/cpu.rs | `wfe` | 116 | 9 | yes |
| arch/aarch64/cpu.rs | `sev` | 125 | 9 | yes |
| arch/aarch64/cpu.rs | `isb` | 134 | 9 | yes |
| arch/aarch64/cpu.rs | `dsb` | 143 | 9 | yes |
| arch/aarch64/cpu.rs | `dmb` | 152 | 9 | yes |
| arch/aarch64/cpu.rs | `local_flush_tlb_all` | 161 | 15 | yes |
| arch/aarch64/cpu.rs | `get_interrupts_state` | 176 | 13 | yes |
| arch/aarch64/cpu.rs | `save_and_disable_irq` | 189 | 8 | yes |
| arch/aarch64/cpu.rs | `restore_irq` | 197 | 5 | yes |
| arch/aarch64/ipi.rs | `bit` | 48 | 20 |  |
| arch/aarch64/ipi.rs | `request_ipi` | 68 | 24 |  |
| arch/aarch64/ipi.rs | `send_ipi_type` | 92 | 20 | yes |
| arch/aarch64/ipi.rs | `send_reschedule_ipi` | 112 | 8 | yes |
| arch/aarch64/ipi.rs | `flush_tlb_others` | 120 | 19 | yes |
| arch/aarch64/ipi.rs | `handle_software_ipi` | 139 | 46 | yes |
| arch/aarch64/ipi.rs | `new` | 185 | 19 | yes |
| arch/aarch64/ipi.rs | `csd_init` | 204 | 14 |  |
| arch/aarch64/ipi.rs | `smp_call_function` | 218 | 53 | yes |
| arch/aarch64/ipi.rs | `csd_flush_queue` | 271 | 42 |  |
| arch/aarch64/ipi.rs | `ipi_reschedule_handler` | 313 | 7 |  |
| arch/aarch64/ipi.rs | `ipi_call_function_handler` | 320 | 4 |  |
| arch/aarch64/ipi.rs | `ipi_stop_handler` | 324 | 8 |  |
| arch/aarch64/ipi.rs | `ipi_wakeup_handler` | 332 | 4 |  |
| arch/aarch64/ipi.rs | `ipi_tlb_flush_handler` | 336 | 12 |  |
| arch/aarch64/ipi.rs | `handle_ipi` | 348 | 7 |  |
| arch/aarch64/ipi.rs | `register_irq_handlers` | 355 | 3 |  |
| arch/aarch64/ipi.rs | `init` | 358 | 13 | yes |
| arch/aarch64/mm/asid.rs | `alloc_asid` | 64 | 34 | yes |
| arch/aarch64/mm/asid.rs | `free_asid` | 98 | 10 |  |
| arch/aarch64/mm/asid.rs | `asid_usage_count` | 108 | 11 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_all` | 119 | 14 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_asid` | 133 | 19 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_page` | 152 | 26 | yes |
| arch/aarch64/mm/asid.rs | `flush_tlb_range` | 178 | 29 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_kernel` | 207 | 17 |  |
| arch/aarch64/mm/asid.rs | `build_satp` | 224 | 5 | yes |
| arch/aarch64/mm/asid.rs | `satp_to_asid` | 229 | 5 |  |
| arch/aarch64/mm/asid.rs | `satp_to_ppn` | 234 | 5 | yes |
| arch/aarch64/mm/asid.rs | `read_satp` | 239 | 8 | yes |
| arch/aarch64/mm/asid.rs | `write_satp` | 247 | 29 | yes |
| arch/aarch64/mm/asid.rs | `new` | 276 | 7 | yes |
| arch/aarch64/mm/asid.rs | `asid` | 283 | 4 | yes |
| arch/aarch64/mm/asid.rs | `alloc` | 287 | 7 | yes |
| arch/aarch64/mm/asid.rs | `free` | 294 | 13 | yes |
| arch/aarch64/mm/asid.rs | `generation` | 307 | 6 |  |
| arch/aarch64/mm/asid.rs | `print_asid_status` | 313 | 8 |  |
| arch/aarch64/mm/exception.rs | `fixup_exception` | 40 | 19 | yes |
| arch/aarch64/mm/exception.rs | `exception_table_empty` | 59 | 7 | yes |
| arch/aarch64/mm/exception.rs | `exception_table_count` | 66 | 10 | yes |
| arch/aarch64/mm/exception.rs | `send_signal` | 76 | 15 | yes |
| arch/aarch64/mm/exception.rs | `bad_area` | 91 | 13 | yes |
| arch/aarch64/mm/exception.rs | `no_context` | 104 | 20 | yes |
| arch/aarch64/mm/exception.rs | `do_page_fault` | 124 | 182 | yes |
| arch/aarch64/mm/fixmap.rs | `fix_to_virt` | 46 | 6 |  |
| arch/aarch64/mm/fixmap.rs | `virt_to_fix` | 52 | 10 |  |
| arch/aarch64/mm/fixmap.rs | `is_fixmap_addr` | 62 | 13 |  |
| arch/aarch64/mm/fixmap.rs | `init_uart_fixmap` | 75 | 13 |  |
| arch/aarch64/mm/fixmap.rs | `uart_virt_addr` | 88 | 11 |  |
| arch/aarch64/mm/fixmap.rs | `is_uart_fixmap_initialized` | 99 | 5 |  |
| arch/aarch64/mm/fixmap.rs | `print_fixmap_status` | 104 | 9 |  |
| arch/aarch64/mm/memory_layout.rs | `is_linear_mapping` | 131 | 80 |  |
| arch/aarch64/mm/memory_layout.rs | `mmio_alias` | 211 | 11 |  |
| arch/aarch64/mm/memory_layout.rs | `device_alias` | 222 | 111 |  |
| arch/aarch64/mm/memory_layout.rs | `new` | 333 | 12 | yes |
| arch/aarch64/mm/memory_layout.rs | `bits` | 345 | 5 | yes |
| arch/aarch64/mm/memory_layout.rs | `is_aligned` | 350 | 5 |  |
| arch/aarch64/mm/memory_layout.rs | `floor` | 355 | 5 |  |
| arch/aarch64/mm/memory_layout.rs | `ceil` | 360 | 5 |  |
| arch/aarch64/mm/memory_layout.rs | `page_offset` | 365 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `vpn` | 371 | 5 |  |
| arch/aarch64/mm/memory_layout.rs | `as_u64` | 376 | 5 | yes |
| arch/aarch64/mm/memory_layout.rs | `as_usize` | 381 | 12 | yes |
| arch/aarch64/mm/memory_layout.rs | `new` | 393 | 5 | yes |
| arch/aarch64/mm/memory_layout.rs | `bits` | 398 | 5 | yes |
| arch/aarch64/mm/memory_layout.rs | `is_aligned` | 403 | 5 |  |
| arch/aarch64/mm/memory_layout.rs | `floor` | 408 | 5 |  |
| arch/aarch64/mm/memory_layout.rs | `ceil` | 413 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `ppn` | 419 | 7 | yes |
| arch/aarch64/mm/memory_layout.rs | `phys_to_virt` | 426 | 9 | yes |
| arch/aarch64/mm/memory_layout.rs | `virt_to_phys` | 435 | 25 | yes |
| arch/aarch64/mm/mm_ops.rs | `walk` | 70 | 29 | yes |
| arch/aarch64/mm/mm_ops.rs | `walk_level` | 99 | 29 |  |
| arch/aarch64/mm/mm_ops.rs | `enable` | 128 | 9 | yes |
| arch/aarch64/mm/mm_ops.rs | `disable` | 137 | 9 | yes |
| arch/aarch64/mm/mm_ops.rs | `flush_tlb` | 146 | 8 |  |
| arch/aarch64/mm/mm_ops.rs | `flush_tlb_addr_page` | 154 | 8 |  |
| arch/aarch64/mm/mm_ops.rs | `map_single_page` | 162 | 43 |  |
| arch/aarch64/mm/mm_ops.rs | `unmap_vma` | 205 | 11 |  |
| arch/aarch64/mm/mm_ops.rs | `unmap_pages` | 216 | 97 |  |
| arch/aarch64/mm/mm_ops.rs | `unmap_one_user_leaf_locked` | 313 | 45 |  |
| arch/aarch64/mm/mm_ops.rs | `clear_pte` | 358 | 14 |  |
| arch/aarch64/mm/mm_ops.rs | `perm_to_flags` | 372 | 43 |  |
| arch/aarch64/mm/mm_ops.rs | `map_user_page` | 415 | 20 |  |
| arch/aarch64/mm/mm_ops.rs | `map_user_page_noflush` | 435 | 53 |  |
| arch/aarch64/mm/mm_ops.rs | `map_user_region` | 488 | 37 | yes |
| arch/aarch64/mm/mm_ops.rs | `alloc_user_phys_page` | 525 | 16 |  |
| arch/aarch64/mm/mm_ops.rs | `create_user_address_space` | 541 | 21 |  |
| arch/aarch64/mm/mm_ops.rs | `pte_lock_for` | 562 | 27 |  |
| arch/aarch64/mm/mm_ops.rs | `pte_flag_bits` | 589 | 38 |  |
| arch/aarch64/mm/mm_ops.rs | `leaf_pte_replace` | 627 | 51 |  |
| arch/aarch64/mm/mm_ops.rs | `leaf_pte_install` | 678 | 47 |  |
| arch/aarch64/mm/mm_ops.rs | `alloc_and_map_user_memory` | 725 | 100 |  |
| arch/aarch64/mm/mm_ops.rs | `alloc_and_map_to_user_table` | 825 | 68 |  |
| arch/aarch64/mm/mm_ops.rs | `pte_is_shared_cache` | 893 | 7 |  |
| arch/aarch64/mm/mm_ops.rs | `release_cache_mapping_pin` | 900 | 27 |  |
| arch/aarch64/mm/mm_ops.rs | `copy_page_table_cow` | 927 | 162 |  |
| arch/aarch64/mm/mm_ops.rs | `cow_share_block` | 1089 | 43 |  |
| arch/aarch64/mm/mm_ops.rs | `handle_cow_fault` | 1132 | 14 | yes |
| arch/aarch64/mm/mm_ops.rs | `drop` | 1146 | 122 | yes |
| arch/aarch64/mm/mm_ops.rs | `write_user_leaf` | 1268 | 13 |  |
| arch/aarch64/mm/mm_ops.rs | `is_cow_page` | 1281 | 10 |  |
| arch/aarch64/mm/mm_ops.rs | `check_pte_permissions` | 1291 | 19 |  |
| arch/aarch64/mm/mm_ops.rs | `is_protnone_marker` | 1310 | 27 |  |
| arch/aarch64/mm/mm_ops.rs | `mprotect_pte_range` | 1337 | 72 | yes |
| arch/aarch64/mm/mmu_init.rs | `adopt_boot_ttbr1` | 60 | 26 |  |
| arch/aarch64/mm/mmu_init.rs | `root_page_table_ppn` | 86 | 11 | yes |
| arch/aarch64/mm/mmu_init.rs | `get_kernel_page_table_ppn` | 97 | 9 |  |
| arch/aarch64/mm/mmu_init.rs | `root_static_phys` | 106 | 22 | yes |
| arch/aarch64/mm/mmu_init.rs | `from_u8` | 128 | 12 |  |
| arch/aarch64/mm/mmu_init.rs | `get_alloc_stage` | 140 | 5 |  |
| arch/aarch64/mm/mmu_init.rs | `alloc_stage_is_late` | 145 | 5 | yes |
| arch/aarch64/mm/mmu_init.rs | `pt_ops_set_fixmap` | 150 | 5 |  |
| arch/aarch64/mm/mmu_init.rs | `pt_ops_set_late` | 155 | 30 |  |
| arch/aarch64/mm/mmu_init.rs | `alloc_page_table` | 185 | 67 |  |
| arch/aarch64/mm/mmu_init.rs | `free_page_table` | 252 | 11 |  |
| arch/aarch64/mm/mmu_init.rs | `free_page_table_checked` | 263 | 30 |  |
| arch/aarch64/mm/mmu_init.rs | `register_fork_root` | 293 | 19 |  |
| arch/aarch64/mm/mmu_init.rs | `pt_pending_drain` | 312 | 2 |  |
| arch/aarch64/mm/mmu_init.rs | `free_user_page_tables` | 314 | 98 | yes |
| arch/aarch64/mm/mmu_init.rs | `release_user_leaf_mm` | 412 | 36 |  |
| arch/aarch64/mm/mmu_init.rs | `set_user_leaf_flags` | 448 | 14 | yes |
| arch/aarch64/mm/mmu_init.rs | `user_l3_table_for` | 462 | 30 |  |
| arch/aarch64/mm/mmu_init.rs | `new` | 492 | 8 | yes |
| arch/aarch64/mm/mmu_init.rs | `live_count` | 500 | 6 |  |
| arch/aarch64/mm/mmu_init.rs | `index` | 506 | 13 | yes |
| arch/aarch64/mm/mmu_init.rs | `stamp` | 519 | 12 |  |
| arch/aarch64/mm/mmu_init.rs | `take` | 531 | 17 | yes |
| arch/aarch64/mm/mmu_init.rs | `is_live_table` | 548 | 11 | yes |
| arch/aarch64/mm/mmu_init.rs | `poison_table` | 559 | 10 | yes |
| arch/aarch64/mm/mmu_init.rs | `is_boot` | 569 | 5 |  |
| arch/aarch64/mm/mmu_init.rs | `is_poisoned` | 574 | 55 | yes |
| arch/aarch64/mm/mmu_init.rs | `kernel_table_for` | 629 | 29 |  |
| arch/aarch64/mm/mmu_init.rs | `map_kernel_page_noflush` | 658 | 17 |  |
| arch/aarch64/mm/mmu_init.rs | `map_kernel_page` | 675 | 12 | yes |
| arch/aarch64/mm/mmu_init.rs | `map_kernel_region` | 687 | 18 |  |
| arch/aarch64/mm/mmu_init.rs | `map_pmd_huge_page` | 705 | 16 |  |
| arch/aarch64/mm/mmu_init.rs | `map_pud_huge_page` | 721 | 21 |  |
| arch/aarch64/mm/mmu_init.rs | `map_device_page` | 742 | 9 |  |
| arch/aarch64/mm/mmu_init.rs | `best_map_size` | 751 | 17 |  |
| arch/aarch64/mm/mmu_init.rs | `init` | 768 | 49 | yes |
| arch/aarch64/mm/mmu_init.rs | `teardown_early_identity_map` | 817 | 54 |  |
| arch/aarch64/mm/mmu_init.rs | `setup_linear_mapping` | 871 | 35 |  |
| arch/aarch64/mm/mmu_init.rs | `setup_device_mappings` | 906 | 52 |  |
| arch/aarch64/mm/mmu_init.rs | `enable` | 958 | 5 | yes |
| arch/aarch64/mm/mmu_init.rs | `get_current_satp_pub` | 963 | 8 |  |
| arch/aarch64/mm/mmu_init.rs | `map_identity` | 971 | 14 |  |
| arch/aarch64/mm/mmu_init.rs | `map_page` | 985 | 9 |  |
| arch/aarch64/mm/mmu_init.rs | `kernel_device_window_pte` | 994 | 7 |  |
| arch/aarch64/mm/mmu_init.rs | `read_satp_view` | 1001 | 3 |  |
| arch/aarch64/mm/mod.rs | `register_in_tree_mm_policy` | 54 | 5 |  |
| arch/aarch64/mm/page_fault.rs | `get_user_phys` | 81 | 4 |  |
| arch/aarch64/mm/pagetable.rs | `new` | 116 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `from_bits` | 122 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `bits` | 128 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `set_bits` | 134 | 8 | yes |
| arch/aarch64/mm/pagetable.rs | `is_valid` | 142 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `is_readable` | 148 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `is_writable` | 154 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `is_executable` | 160 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `is_user` | 166 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `is_accessed` | 172 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `is_dirty` | 178 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `is_leaf` | 184 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `is_page` | 190 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `is_table` | 196 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `ppn` | 202 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `output_addr` | 208 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `ppn_for_2mb_page` | 216 | 11 |  |
| arch/aarch64/mm/pagetable.rs | `hw_attrs_from_flags` | 227 | 30 |  |
| arch/aarch64/mm/pagetable.rs | `new_table` | 257 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_kernel_flags` | 264 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `new_block_kernel_flags` | 272 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `new_block_device` | 280 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_kernel` | 289 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_user` | 295 | 14 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_user_flags` | 309 | 13 |  |
| arch/aarch64/mm/pagetable.rs | `rebuild_user_leaf` | 322 | 16 |  |
| arch/aarch64/mm/pagetable.rs | `rebuild_user_leaf_protnone` | 338 | 11 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_ro` | 349 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_device` | 356 | 35 |  |
| arch/aarch64/mm/pagetable.rs | `bitor` | 391 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `bitor_assign` | 398 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `bitand` | 406 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `not` | 414 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `default` | 420 | 16 | yes |
| arch/aarch64/mm/pagetable.rs | `new` | 436 | 8 | yes |
| arch/aarch64/mm/pagetable.rs | `get` | 444 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `set` | 450 | 5 | yes |
| arch/aarch64/mm/pagetable.rs | `zero` | 455 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `default` | 463 | 34 | yes |
| arch/aarch64/mm/pagetable.rs | `new` | 497 | 5 | yes |
| arch/aarch64/mm/pagetable.rs | `long` | 502 | 5 |  |
| arch/aarch64/mm/pagetable.rs | `bits` | 507 | 5 | yes |
| arch/aarch64/mm/pagetable.rs | `mode` | 512 | 5 |  |
| arch/aarch64/mm/pagetable.rs | `asid` | 517 | 5 | yes |
| arch/aarch64/mm/pagetable.rs | `ppn` | 522 | 5 | yes |
| arch/aarch64/mm/pagetable.rs | `is_bare` | 527 | 5 |  |
| arch/aarch64/mm/pagetable.rs | `is_long` | 532 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `get_satp` | 541 | 14 | yes |
| arch/aarch64/mm/unsafe_edges.rs | `get_page_table_virt` | 43 | 26 | yes |
| arch/aarch64/mm/unsafe_edges.rs | `from_phys` | 69 | 22 | yes |
| arch/aarch64/mm/unsafe_edges.rs | `from_phys_live` | 91 | 5 |  |
| arch/aarch64/mm/unsafe_edges.rs | `as_phys_base` | 96 | 6 |  |
| arch/aarch64/mm/unsafe_edges.rs | `get_raw` | 102 | 8 |  |
| arch/aarch64/mm/unsafe_edges.rs | `get` | 110 | 6 | yes |
| arch/aarch64/mm/unsafe_edges.rs | `set` | 116 | 11 | yes |
| arch/aarch64/mm/unsafe_edges.rs | `all_zero` | 127 | 11 |  |
| arch/aarch64/mm/unsafe_edges.rs | `zero_entries` | 138 | 12 |  |
| arch/aarch64/mm/unsafe_edges.rs | `copy_page` | 150 | 8 |  |
| arch/aarch64/mod.rs | `arch_init` | 33 | 4 |  |
| arch/aarch64/mod.rs | `init` | 37 | 20 | yes |
| arch/aarch64/mod.rs | `enable_interrupts` | 57 | 7 |  |
| arch/aarch64/mod.rs | `cpu_id` | 64 | 3 | yes |
| arch/aarch64/process.rs | `start_thread` | 20 | 33 |  |
| arch/aarch64/process.rs | `flush_thread` | 53 | 29 |  |
| arch/aarch64/process.rs | `current_pt_regs` | 82 | 6 | yes |
| arch/aarch64/process.rs | `task_pt_regs` | 88 | 9 |  |
| arch/aarch64/process.rs | `user_stack_pointer` | 97 | 6 |  |
| arch/aarch64/process.rs | `set_user_stack_pointer` | 103 | 6 |  |
| arch/aarch64/process.rs | `instruction_pointer` | 109 | 6 | yes |
| arch/aarch64/process.rs | `set_instruction_pointer` | 115 | 6 |  |
| arch/aarch64/process.rs | `is_user_address` | 121 | 6 |  |
| arch/aarch64/process.rs | `sys_arch_prctl` | 127 | 3 |  |
| arch/aarch64/pt_regs.rs | `mode` | 61 | 5 |  |
| arch/aarch64/pt_regs.rs | `is_el0` | 66 | 59 |  |
| arch/aarch64/pt_regs.rs | `default` | 125 | 13 | yes |
| arch/aarch64/pt_regs.rs | `new` | 138 | 16 | yes |
| arch/aarch64/pt_regs.rs | `user_mode` | 154 | 6 | yes |
| arch/aarch64/pt_regs.rs | `kernel_mode` | 160 | 6 | yes |
| arch/aarch64/pt_regs.rs | `syscall_nr` | 166 | 6 | yes |
| arch/aarch64/pt_regs.rs | `syscall_args` | 172 | 13 |  |
| arch/aarch64/pt_regs.rs | `set_return_value` | 185 | 6 |  |
| arch/aarch64/pt_regs.rs | `set_return_error` | 191 | 6 |  |
| arch/aarch64/pt_regs.rs | `syscall_rollback` | 197 | 6 |  |
| arch/aarch64/pt_regs.rs | `instruction_pointer` | 203 | 6 | yes |
| arch/aarch64/pt_regs.rs | `set_instruction_pointer` | 209 | 6 |  |
| arch/aarch64/pt_regs.rs | `user_stack_pointer` | 215 | 6 |  |
| arch/aarch64/pt_regs.rs | `set_user_stack_pointer` | 221 | 6 |  |
| arch/aarch64/pt_regs.rs | `frame_pointer` | 227 | 7 |  |
| arch/aarch64/pt_regs.rs | `irqs_disabled` | 234 | 7 |  |
| arch/aarch64/pt_regs.rs | `mark_user_frame` | 241 | 6 |  |
| arch/aarch64/pt_regs.rs | `mark_kernel_frame` | 247 | 51 |  |
| arch/aarch64/pt_regs.rs | `from_cause` | 298 | 32 | yes |
| arch/aarch64/pt_regs.rs | `is_interrupt` | 330 | 5 |  |
| arch/aarch64/pt_regs.rs | `is_exception` | 335 | 6 |  |
| arch/aarch64/pt_regs.rs | `is_page_fault` | 341 | 13 |  |
| arch/aarch64/pt_regs.rs | `in_interrupt` | 354 | 6 | yes |
| arch/aarch64/pt_regs.rs | `in_task` | 360 | 25 | yes |
| arch/aarch64/smp.rs | `boot_hart_id` | 52 | 5 | yes |
| arch/aarch64/smp.rs | `boot_cpu_id` | 57 | 4 | yes |
| arch/aarch64/smp.rs | `mark_cpu_started` | 61 | 7 | yes |
| arch/aarch64/smp.rs | `cpu_started` | 68 | 12 | yes |
| arch/aarch64/smp.rs | `cpu_id` | 80 | 32 | yes |
| arch/aarch64/smp.rs | `is_boot_hart` | 112 | 12 | yes |
| arch/aarch64/smp.rs | `init` | 124 | 11 | yes |
| arch/aarch64/smp.rs | `read_mpidr` | 135 | 15 | yes |
| arch/aarch64/smp.rs | `mpidr_to_index` | 150 | 11 | yes |
| arch/aarch64/smp.rs | `index_to_mpidr` | 161 | 28 | yes |
| arch/aarch64/smp.rs | `psci_call` | 189 | 41 | yes |
| arch/aarch64/smp.rs | `detect_psci_from_dtb` | 230 | 112 | yes |
| arch/aarch64/smp.rs | `signal_boot_complete` | 342 | 16 | yes |
| arch/aarch64/smp.rs | `is_boot_complete` | 358 | 23 | yes |
| arch/aarch64/smp.rs | `secondary_start` | 381 | 17 | yes |
| arch/aarch64/smp.rs | `start_secondaries` | 398 | 163 | yes |
| arch/aarch64/smp.rs | `num_started_cpus` | 561 | 21 | yes |
| arch/aarch64/smp.rs | `as_ptr` | 582 | 12 | yes |
| arch/aarch64/smp.rs | `init_per_cpu_intr_stacks` | 594 | 29 | yes |
| arch/aarch64/thread.rs | `default` | 35 | 7 | yes |
| arch/aarch64/thread.rs | `zeroed` | 42 | 73 | yes |
| arch/aarch64/thread.rs | `new` | 115 | 21 | yes |
| arch/aarch64/thread.rs | `tp` | 136 | 6 | yes |
| arch/aarch64/thread.rs | `set_tp` | 142 | 6 | yes |
| arch/aarch64/thread.rs | `mark_fpu_clean` | 148 | 6 | yes |
| arch/aarch64/thread.rs | `get_fs` | 154 | 12 | yes |
| arch/aarch64/thread.rs | `fpu_save` | 166 | 39 | yes |
| arch/aarch64/thread.rs | `fpu_restore` | 205 | 37 | yes |
| arch/aarch64/thread.rs | `mark_vstate_clean` | 242 | 4 | yes |
| arch/aarch64/thread.rs | `default` | 246 | 33 | yes |
| arch/aarch64/trap.rs | `esr_ec` | 73 | 6 | yes |
| arch/aarch64/trap.rs | `esr_iss` | 79 | 45 | yes |
| arch/aarch64/trap.rs | `dfsc_class` | 124 | 19 | yes |
| arch/aarch64/trap.rs | `current_pt_regs` | 143 | 6 | yes |
| arch/aarch64/trap.rs | `current_task_pt_regs` | 149 | 15 | yes |
| arch/aarch64/trap.rs | `pt_regs_rip_for_cpu` | 164 | 227 | yes |
| arch/aarch64/trap.rs | `oops_show_regs` | 391 | 39 | yes |
| arch/aarch64/trap.rs | `handle_syscall` | 430 | 36 | yes |
| arch/aarch64/trap.rs | `handle_external_interrupt` | 466 | 38 | yes |
| arch/aarch64/trap.rs | `handle_timer_interrupt` | 504 | 63 | yes |
| arch/aarch64/trap.rs | `handle_illegal_instruction` | 567 | 23 | yes |
| arch/aarch64/trap.rs | `sig_has_handler` | 590 | 11 | yes |
| arch/aarch64/trap.rs | `sig_blocked_current` | 601 | 6 | yes |
| arch/aarch64/trap.rs | `sigsegv_has_handler` | 607 | 4 | yes |
| arch/aarch64/trap.rs | `sigsegv_blocked_current` | 611 | 4 | yes |
| arch/aarch64/trap.rs | `sigbus_has_handler` | 615 | 4 | yes |
| arch/aarch64/trap.rs | `sigill_has_handler` | 619 | 10 | yes |
| arch/aarch64/trap.rs | `fault_sigsegv_route` | 629 | 62 | yes |
| arch/aarch64/trap.rs | `handle_page_fault` | 691 | 157 | yes |
| arch/aarch64/trap.rs | `init` | 848 | 2 | yes |
| arch/aarch64/trap.rs | `aarch64_vector_table` | 850 | 14 | yes |
| arch/aarch64/trap.rs | `init_syscall` | 864 | 8 | yes |
| arch/aarch64/trap.rs | `enable_timer_interrupt` | 872 | 22 | yes |
| arch/aarch64/trap.rs | `disable_timer_interrupt` | 894 | 13 | yes |
| arch/aarch64/trap.rs | `enable_external_interrupt` | 907 | 13 | yes |
| arch/aarch64/uaccess.rs | `access_ok` | 43 | 21 | yes |
| arch/aarch64/uaccess.rs | `__aa64_copy_to_user` | 64 | 1 | yes |
| arch/aarch64/uaccess.rs | `__aa64_copy_from_user` | 65 | 1 | yes |
| arch/aarch64/uaccess.rs | `__aa64_clear_user` | 66 | 1 | yes |
| arch/aarch64/uaccess.rs | `__aa64_futex_cmpxchg_u32` | 67 | 10 | yes |
| arch/aarch64/uaccess.rs | `futex_cmpxchg_u32_user` | 77 | 30 | yes |
| arch/aarch64/uaccess.rs | `copy_to_user` | 107 | 21 | yes |
| arch/aarch64/uaccess.rs | `copy_from_user` | 128 | 29 | yes |
| arch/aarch64/uaccess.rs | `clear_user` | 157 | 16 | yes |
| arch/aarch64/uaccess.rs | `put_user` | 173 | 15 | yes |
| arch/aarch64/uaccess.rs | `get_user` | 188 | 25 | yes |
| arch/aarch64/uaccess.rs | `strncpy_from_user` | 213 | 45 | yes |
| arch/aarch64/uaccess.rs | `strnlen_user` | 258 | 38 | yes |
| arch/aarch64/uaccess.rs | `selftest` | 296 | 3 | yes |
| arch/aarch64/uaccess.rs | `emit` | 299 | 108 | yes |
