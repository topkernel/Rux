# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch/aarch64 — hot 3084 lines (stay) / cold 4204 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/aarch64/boot.rs | `get_core_id` | 21 | 6 | yes |
| arch/aarch64/boot.rs | `get_dtb_pointer` | 26 | 6 | yes |
| arch/aarch64/context.rs | `set_prev_task` | 66 | 30 | yes |
| arch/aarch64/context.rs | `__switch_mm_linear` | 95 | 21 | yes |
| arch/aarch64/context.rs | `switch_mm` | 115 | 29 | yes |
| arch/aarch64/context.rs | `get_current_satp` | 143 | 14 | yes |
| arch/aarch64/context.rs | `__switch_to` | 156 | 9 | yes |
| arch/aarch64/context.rs | `context_switch` | 164 | 63 | yes |
| arch/aarch64/cpu.rs | `get_core_id` | 12 | 9 | yes |
| arch/aarch64/cpu.rs | `get_thread_id_of` | 20 | 11 | yes |
| arch/aarch64/cpu.rs | `get_thread_id` | 30 | 12 | yes |
| arch/aarch64/cpu.rs | `set_thread_id` | 41 | 12 | yes |
| arch/aarch64/cpu.rs | `get_counter_freq` | 52 | 12 | yes |
| arch/aarch64/cpu.rs | `read_counter` | 63 | 13 | yes |
| arch/aarch64/cpu.rs | `read_time` | 75 | 8 | yes |
| arch/aarch64/cpu.rs | `enable_irq` | 82 | 13 | yes |
| arch/aarch64/cpu.rs | `disable_irq` | 94 | 12 | yes |
| arch/aarch64/cpu.rs | `wfi` | 105 | 12 | yes |
| arch/aarch64/cpu.rs | `wfe` | 116 | 10 | yes |
| arch/aarch64/cpu.rs | `sev` | 125 | 10 | yes |
| arch/aarch64/cpu.rs | `isb` | 134 | 10 | yes |
| arch/aarch64/cpu.rs | `dsb` | 143 | 10 | yes |
| arch/aarch64/cpu.rs | `dmb` | 152 | 10 | yes |
| arch/aarch64/cpu.rs | `local_flush_tlb_all` | 161 | 16 | yes |
| arch/aarch64/cpu.rs | `get_interrupts_state` | 176 | 14 | yes |
| arch/aarch64/cpu.rs | `save_and_disable_irq` | 189 | 9 | yes |
| arch/aarch64/cpu.rs | `restore_irq` | 197 | 6 | yes |
| arch/aarch64/ipi.rs | `bit` | 48 | 21 |  |
| arch/aarch64/ipi.rs | `request_ipi` | 68 | 25 |  |
| arch/aarch64/ipi.rs | `send_ipi_type` | 92 | 21 |  |
| arch/aarch64/ipi.rs | `send_reschedule_ipi` | 112 | 9 |  |
| arch/aarch64/ipi.rs | `flush_tlb_others` | 120 | 20 |  |
| arch/aarch64/ipi.rs | `handle_software_ipi` | 139 | 47 |  |
| arch/aarch64/ipi.rs | `new` | 185 | 20 | yes |
| arch/aarch64/ipi.rs | `csd_init` | 204 | 15 |  |
| arch/aarch64/ipi.rs | `smp_call_function` | 218 | 54 |  |
| arch/aarch64/ipi.rs | `csd_flush_queue` | 271 | 43 |  |
| arch/aarch64/ipi.rs | `ipi_reschedule_handler` | 313 | 8 |  |
| arch/aarch64/ipi.rs | `ipi_call_function_handler` | 320 | 5 |  |
| arch/aarch64/ipi.rs | `ipi_stop_handler` | 324 | 9 |  |
| arch/aarch64/ipi.rs | `ipi_wakeup_handler` | 332 | 5 |  |
| arch/aarch64/ipi.rs | `ipi_tlb_flush_handler` | 336 | 13 |  |
| arch/aarch64/ipi.rs | `handle_ipi` | 348 | 8 |  |
| arch/aarch64/ipi.rs | `register_irq_handlers` | 355 | 4 |  |
| arch/aarch64/ipi.rs | `init` | 358 | 14 | yes |
| arch/aarch64/mm/asid.rs | `alloc_asid` | 64 | 35 |  |
| arch/aarch64/mm/asid.rs | `free_asid` | 98 | 11 |  |
| arch/aarch64/mm/asid.rs | `asid_usage_count` | 108 | 12 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_all` | 119 | 15 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_asid` | 133 | 20 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_page` | 152 | 27 | yes |
| arch/aarch64/mm/asid.rs | `flush_tlb_range` | 178 | 30 |  |
| arch/aarch64/mm/asid.rs | `flush_tlb_kernel` | 207 | 18 |  |
| arch/aarch64/mm/asid.rs | `build_satp` | 224 | 6 | yes |
| arch/aarch64/mm/asid.rs | `satp_to_asid` | 229 | 6 |  |
| arch/aarch64/mm/asid.rs | `satp_to_ppn` | 234 | 6 | yes |
| arch/aarch64/mm/asid.rs | `read_satp` | 239 | 9 |  |
| arch/aarch64/mm/asid.rs | `write_satp` | 247 | 30 |  |
| arch/aarch64/mm/asid.rs | `new` | 276 | 8 | yes |
| arch/aarch64/mm/asid.rs | `asid` | 283 | 5 |  |
| arch/aarch64/mm/asid.rs | `alloc` | 287 | 8 |  |
| arch/aarch64/mm/asid.rs | `free` | 294 | 14 |  |
| arch/aarch64/mm/asid.rs | `generation` | 307 | 7 |  |
| arch/aarch64/mm/asid.rs | `print_asid_status` | 313 | 9 |  |
| arch/aarch64/mm/exception.rs | `fixup_exception` | 40 | 20 | yes |
| arch/aarch64/mm/exception.rs | `exception_table_empty` | 59 | 8 | yes |
| arch/aarch64/mm/exception.rs | `exception_table_count` | 66 | 11 | yes |
| arch/aarch64/mm/exception.rs | `send_signal` | 76 | 16 | yes |
| arch/aarch64/mm/exception.rs | `bad_area` | 91 | 14 | yes |
| arch/aarch64/mm/exception.rs | `no_context` | 104 | 21 | yes |
| arch/aarch64/mm/exception.rs | `do_page_fault` | 124 | 183 | yes |
| arch/aarch64/mm/fixmap.rs | `fix_to_virt` | 46 | 7 |  |
| arch/aarch64/mm/fixmap.rs | `virt_to_fix` | 52 | 11 |  |
| arch/aarch64/mm/fixmap.rs | `is_fixmap_addr` | 62 | 14 |  |
| arch/aarch64/mm/fixmap.rs | `init_uart_fixmap` | 75 | 14 |  |
| arch/aarch64/mm/fixmap.rs | `uart_virt_addr` | 88 | 12 |  |
| arch/aarch64/mm/fixmap.rs | `is_uart_fixmap_initialized` | 99 | 6 |  |
| arch/aarch64/mm/fixmap.rs | `print_fixmap_status` | 104 | 10 |  |
| arch/aarch64/mm/memory_layout.rs | `is_linear_mapping` | 131 | 81 |  |
| arch/aarch64/mm/memory_layout.rs | `mmio_alias` | 211 | 12 |  |
| arch/aarch64/mm/memory_layout.rs | `device_alias` | 222 | 112 |  |
| arch/aarch64/mm/memory_layout.rs | `new` | 333 | 13 | yes |
| arch/aarch64/mm/memory_layout.rs | `bits` | 345 | 6 | yes |
| arch/aarch64/mm/memory_layout.rs | `is_aligned` | 350 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `floor` | 355 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `ceil` | 360 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `page_offset` | 365 | 7 |  |
| arch/aarch64/mm/memory_layout.rs | `vpn` | 371 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `as_u64` | 376 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `as_usize` | 381 | 13 | yes |
| arch/aarch64/mm/memory_layout.rs | `new` | 393 | 6 | yes |
| arch/aarch64/mm/memory_layout.rs | `bits` | 398 | 6 | yes |
| arch/aarch64/mm/memory_layout.rs | `is_aligned` | 403 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `floor` | 408 | 6 |  |
| arch/aarch64/mm/memory_layout.rs | `ceil` | 413 | 7 |  |
| arch/aarch64/mm/memory_layout.rs | `ppn` | 419 | 8 |  |
| arch/aarch64/mm/memory_layout.rs | `phys_to_virt` | 426 | 10 |  |
| arch/aarch64/mm/memory_layout.rs | `virt_to_phys` | 435 | 26 |  |
| arch/aarch64/mm/mm_ops.rs | `walk` | 70 | 30 |  |
| arch/aarch64/mm/mm_ops.rs | `walk_level` | 99 | 30 |  |
| arch/aarch64/mm/mm_ops.rs | `enable` | 128 | 10 |  |
| arch/aarch64/mm/mm_ops.rs | `disable` | 137 | 10 |  |
| arch/aarch64/mm/mm_ops.rs | `flush_tlb` | 146 | 9 |  |
| arch/aarch64/mm/mm_ops.rs | `flush_tlb_addr_page` | 154 | 9 |  |
| arch/aarch64/mm/mm_ops.rs | `map_single_page` | 162 | 44 |  |
| arch/aarch64/mm/mm_ops.rs | `unmap_vma` | 205 | 12 |  |
| arch/aarch64/mm/mm_ops.rs | `unmap_pages` | 216 | 98 |  |
| arch/aarch64/mm/mm_ops.rs | `unmap_one_user_leaf_locked` | 313 | 46 |  |
| arch/aarch64/mm/mm_ops.rs | `clear_pte` | 358 | 15 |  |
| arch/aarch64/mm/mm_ops.rs | `perm_to_flags` | 372 | 44 |  |
| arch/aarch64/mm/mm_ops.rs | `map_user_page` | 415 | 21 |  |
| arch/aarch64/mm/mm_ops.rs | `map_user_page_noflush` | 435 | 54 |  |
| arch/aarch64/mm/mm_ops.rs | `map_user_region` | 488 | 38 |  |
| arch/aarch64/mm/mm_ops.rs | `alloc_user_phys_page` | 525 | 17 |  |
| arch/aarch64/mm/mm_ops.rs | `create_user_address_space` | 541 | 22 |  |
| arch/aarch64/mm/mm_ops.rs | `pte_lock_for` | 562 | 28 |  |
| arch/aarch64/mm/mm_ops.rs | `pte_flag_bits` | 589 | 39 |  |
| arch/aarch64/mm/mm_ops.rs | `leaf_pte_replace` | 627 | 52 |  |
| arch/aarch64/mm/mm_ops.rs | `leaf_pte_install` | 678 | 48 |  |
| arch/aarch64/mm/mm_ops.rs | `alloc_and_map_user_memory` | 725 | 101 |  |
| arch/aarch64/mm/mm_ops.rs | `alloc_and_map_to_user_table` | 825 | 69 |  |
| arch/aarch64/mm/mm_ops.rs | `pte_is_shared_cache` | 893 | 8 |  |
| arch/aarch64/mm/mm_ops.rs | `release_cache_mapping_pin` | 900 | 28 |  |
| arch/aarch64/mm/mm_ops.rs | `copy_page_table_cow` | 927 | 163 |  |
| arch/aarch64/mm/mm_ops.rs | `cow_share_block` | 1089 | 44 |  |
| arch/aarch64/mm/mm_ops.rs | `handle_cow_fault` | 1132 | 15 | yes |
| arch/aarch64/mm/mm_ops.rs | `drop` | 1146 | 123 |  |
| arch/aarch64/mm/mm_ops.rs | `write_user_leaf` | 1268 | 14 |  |
| arch/aarch64/mm/mm_ops.rs | `is_cow_page` | 1281 | 11 |  |
| arch/aarch64/mm/mm_ops.rs | `check_pte_permissions` | 1291 | 20 |  |
| arch/aarch64/mm/mm_ops.rs | `is_protnone_marker` | 1310 | 28 |  |
| arch/aarch64/mm/mm_ops.rs | `mprotect_pte_range` | 1337 | 73 |  |
| arch/aarch64/mm/mmu_init.rs | `adopt_boot_ttbr1` | 60 | 27 |  |
| arch/aarch64/mm/mmu_init.rs | `root_page_table_ppn` | 86 | 12 | yes |
| arch/aarch64/mm/mmu_init.rs | `get_kernel_page_table_ppn` | 97 | 10 |  |
| arch/aarch64/mm/mmu_init.rs | `root_static_phys` | 106 | 23 |  |
| arch/aarch64/mm/mmu_init.rs | `from_u8` | 128 | 13 |  |
| arch/aarch64/mm/mmu_init.rs | `get_alloc_stage` | 140 | 6 |  |
| arch/aarch64/mm/mmu_init.rs | `alloc_stage_is_late` | 145 | 6 |  |
| arch/aarch64/mm/mmu_init.rs | `pt_ops_set_fixmap` | 150 | 6 |  |
| arch/aarch64/mm/mmu_init.rs | `pt_ops_set_late` | 155 | 31 |  |
| arch/aarch64/mm/mmu_init.rs | `alloc_page_table` | 185 | 68 |  |
| arch/aarch64/mm/mmu_init.rs | `free_page_table` | 252 | 12 |  |
| arch/aarch64/mm/mmu_init.rs | `free_page_table_checked` | 263 | 31 |  |
| arch/aarch64/mm/mmu_init.rs | `register_fork_root` | 293 | 20 |  |
| arch/aarch64/mm/mmu_init.rs | `pt_pending_drain` | 312 | 3 |  |
| arch/aarch64/mm/mmu_init.rs | `free_user_page_tables` | 314 | 99 |  |
| arch/aarch64/mm/mmu_init.rs | `release_user_leaf_mm` | 412 | 37 |  |
| arch/aarch64/mm/mmu_init.rs | `set_user_leaf_flags` | 448 | 15 |  |
| arch/aarch64/mm/mmu_init.rs | `user_l3_table_for` | 462 | 31 |  |
| arch/aarch64/mm/mmu_init.rs | `new` | 492 | 9 | yes |
| arch/aarch64/mm/mmu_init.rs | `live_count` | 500 | 7 |  |
| arch/aarch64/mm/mmu_init.rs | `index` | 506 | 14 |  |
| arch/aarch64/mm/mmu_init.rs | `stamp` | 519 | 13 |  |
| arch/aarch64/mm/mmu_init.rs | `take` | 531 | 18 |  |
| arch/aarch64/mm/mmu_init.rs | `is_live_table` | 548 | 12 |  |
| arch/aarch64/mm/mmu_init.rs | `poison_table` | 559 | 11 |  |
| arch/aarch64/mm/mmu_init.rs | `is_boot` | 569 | 6 |  |
| arch/aarch64/mm/mmu_init.rs | `is_poisoned` | 574 | 56 |  |
| arch/aarch64/mm/mmu_init.rs | `kernel_table_for` | 629 | 30 |  |
| arch/aarch64/mm/mmu_init.rs | `map_kernel_page_noflush` | 658 | 18 |  |
| arch/aarch64/mm/mmu_init.rs | `map_kernel_page` | 675 | 13 |  |
| arch/aarch64/mm/mmu_init.rs | `map_kernel_region` | 687 | 19 |  |
| arch/aarch64/mm/mmu_init.rs | `map_pmd_huge_page` | 705 | 17 |  |
| arch/aarch64/mm/mmu_init.rs | `map_pud_huge_page` | 721 | 22 |  |
| arch/aarch64/mm/mmu_init.rs | `map_device_page` | 742 | 10 |  |
| arch/aarch64/mm/mmu_init.rs | `best_map_size` | 751 | 18 |  |
| arch/aarch64/mm/mmu_init.rs | `init` | 768 | 50 |  |
| arch/aarch64/mm/mmu_init.rs | `teardown_early_identity_map` | 817 | 55 |  |
| arch/aarch64/mm/mmu_init.rs | `setup_linear_mapping` | 871 | 36 |  |
| arch/aarch64/mm/mmu_init.rs | `setup_device_mappings` | 906 | 53 |  |
| arch/aarch64/mm/mmu_init.rs | `enable` | 958 | 6 |  |
| arch/aarch64/mm/mmu_init.rs | `get_current_satp_pub` | 963 | 9 |  |
| arch/aarch64/mm/mmu_init.rs | `map_identity` | 971 | 15 |  |
| arch/aarch64/mm/mmu_init.rs | `map_page` | 985 | 10 |  |
| arch/aarch64/mm/mmu_init.rs | `kernel_device_window_pte` | 994 | 8 |  |
| arch/aarch64/mm/mmu_init.rs | `read_satp_view` | 1001 | 4 |  |
| arch/aarch64/mm/mod.rs | `register_in_tree_mm_policy` | 54 | 6 |  |
| arch/aarch64/mm/page_fault.rs | `get_user_phys` | 81 | 5 |  |
| arch/aarch64/mm/pagetable.rs | `new` | 116 | 7 | yes |
| arch/aarch64/mm/pagetable.rs | `from_bits` | 122 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `bits` | 128 | 7 | yes |
| arch/aarch64/mm/pagetable.rs | `set_bits` | 134 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `is_valid` | 142 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_readable` | 148 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_writable` | 154 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_executable` | 160 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_user` | 166 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_accessed` | 172 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_dirty` | 178 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_leaf` | 184 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_page` | 190 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `is_table` | 196 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `ppn` | 202 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `output_addr` | 208 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `ppn_for_2mb_page` | 216 | 12 |  |
| arch/aarch64/mm/pagetable.rs | `hw_attrs_from_flags` | 227 | 31 |  |
| arch/aarch64/mm/pagetable.rs | `new_table` | 257 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_kernel_flags` | 264 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `new_block_kernel_flags` | 272 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `new_block_device` | 280 | 10 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_kernel` | 289 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_user` | 295 | 15 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_user_flags` | 309 | 14 |  |
| arch/aarch64/mm/pagetable.rs | `rebuild_user_leaf` | 322 | 17 |  |
| arch/aarch64/mm/pagetable.rs | `rebuild_user_leaf_protnone` | 338 | 12 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_ro` | 349 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `new_page_device` | 356 | 36 |  |
| arch/aarch64/mm/pagetable.rs | `bitor` | 391 | 8 |  |
| arch/aarch64/mm/pagetable.rs | `bitor_assign` | 398 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `bitand` | 406 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `not` | 414 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `default` | 420 | 17 |  |
| arch/aarch64/mm/pagetable.rs | `new` | 436 | 9 | yes |
| arch/aarch64/mm/pagetable.rs | `get` | 444 | 7 |  |
| arch/aarch64/mm/pagetable.rs | `set` | 450 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `zero` | 455 | 9 |  |
| arch/aarch64/mm/pagetable.rs | `default` | 463 | 35 |  |
| arch/aarch64/mm/pagetable.rs | `new` | 497 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `long` | 502 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `bits` | 507 | 6 | yes |
| arch/aarch64/mm/pagetable.rs | `mode` | 512 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `asid` | 517 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `ppn` | 522 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `is_bare` | 527 | 6 |  |
| arch/aarch64/mm/pagetable.rs | `is_long` | 532 | 10 |  |
| arch/aarch64/mm/pagetable.rs | `get_satp` | 541 | 15 | yes |
| arch/aarch64/mm/unsafe_edges.rs | `get_page_table_virt` | 43 | 27 |  |
| arch/aarch64/mm/unsafe_edges.rs | `from_phys` | 69 | 23 |  |
| arch/aarch64/mm/unsafe_edges.rs | `from_phys_live` | 91 | 6 |  |
| arch/aarch64/mm/unsafe_edges.rs | `as_phys_base` | 96 | 7 |  |
| arch/aarch64/mm/unsafe_edges.rs | `get_raw` | 102 | 9 |  |
| arch/aarch64/mm/unsafe_edges.rs | `get` | 110 | 7 |  |
| arch/aarch64/mm/unsafe_edges.rs | `set` | 116 | 12 |  |
| arch/aarch64/mm/unsafe_edges.rs | `all_zero` | 127 | 12 |  |
| arch/aarch64/mm/unsafe_edges.rs | `zero_entries` | 138 | 13 |  |
| arch/aarch64/mm/unsafe_edges.rs | `copy_page` | 150 | 9 |  |
| arch/aarch64/mod.rs | `arch_init` | 33 | 5 |  |
| arch/aarch64/mod.rs | `init` | 37 | 21 | yes |
| arch/aarch64/mod.rs | `enable_interrupts` | 57 | 8 |  |
| arch/aarch64/mod.rs | `cpu_id` | 64 | 4 | yes |
| arch/aarch64/process.rs | `start_thread` | 20 | 34 |  |
| arch/aarch64/process.rs | `flush_thread` | 53 | 30 |  |
| arch/aarch64/process.rs | `current_pt_regs` | 82 | 7 | yes |
| arch/aarch64/process.rs | `task_pt_regs` | 88 | 10 |  |
| arch/aarch64/process.rs | `user_stack_pointer` | 97 | 7 |  |
| arch/aarch64/process.rs | `set_user_stack_pointer` | 103 | 7 |  |
| arch/aarch64/process.rs | `instruction_pointer` | 109 | 7 |  |
| arch/aarch64/process.rs | `set_instruction_pointer` | 115 | 7 |  |
| arch/aarch64/process.rs | `is_user_address` | 121 | 7 |  |
| arch/aarch64/process.rs | `sys_arch_prctl` | 127 | 4 |  |
| arch/aarch64/pt_regs.rs | `mode` | 61 | 6 |  |
| arch/aarch64/pt_regs.rs | `is_el0` | 66 | 60 |  |
| arch/aarch64/pt_regs.rs | `default` | 125 | 14 | yes |
| arch/aarch64/pt_regs.rs | `new` | 138 | 17 | yes |
| arch/aarch64/pt_regs.rs | `user_mode` | 154 | 7 | yes |
| arch/aarch64/pt_regs.rs | `kernel_mode` | 160 | 7 | yes |
| arch/aarch64/pt_regs.rs | `syscall_nr` | 166 | 7 | yes |
| arch/aarch64/pt_regs.rs | `syscall_args` | 172 | 14 |  |
| arch/aarch64/pt_regs.rs | `set_return_value` | 185 | 7 |  |
| arch/aarch64/pt_regs.rs | `set_return_error` | 191 | 7 |  |
| arch/aarch64/pt_regs.rs | `syscall_rollback` | 197 | 7 |  |
| arch/aarch64/pt_regs.rs | `instruction_pointer` | 203 | 7 |  |
| arch/aarch64/pt_regs.rs | `set_instruction_pointer` | 209 | 7 |  |
| arch/aarch64/pt_regs.rs | `user_stack_pointer` | 215 | 7 |  |
| arch/aarch64/pt_regs.rs | `set_user_stack_pointer` | 221 | 7 |  |
| arch/aarch64/pt_regs.rs | `frame_pointer` | 227 | 8 |  |
| arch/aarch64/pt_regs.rs | `irqs_disabled` | 234 | 8 |  |
| arch/aarch64/pt_regs.rs | `mark_user_frame` | 241 | 7 |  |
| arch/aarch64/pt_regs.rs | `mark_kernel_frame` | 247 | 52 |  |
| arch/aarch64/pt_regs.rs | `from_cause` | 298 | 33 | yes |
| arch/aarch64/pt_regs.rs | `is_interrupt` | 330 | 6 |  |
| arch/aarch64/pt_regs.rs | `is_exception` | 335 | 7 |  |
| arch/aarch64/pt_regs.rs | `is_page_fault` | 341 | 14 |  |
| arch/aarch64/pt_regs.rs | `in_interrupt` | 354 | 7 |  |
| arch/aarch64/pt_regs.rs | `in_task` | 360 | 26 |  |
| arch/aarch64/smp.rs | `boot_hart_id` | 52 | 6 | yes |
| arch/aarch64/smp.rs | `boot_cpu_id` | 57 | 5 | yes |
| arch/aarch64/smp.rs | `mark_cpu_started` | 61 | 8 | yes |
| arch/aarch64/smp.rs | `cpu_started` | 68 | 13 | yes |
| arch/aarch64/smp.rs | `cpu_id` | 80 | 33 | yes |
| arch/aarch64/smp.rs | `is_boot_hart` | 112 | 13 | yes |
| arch/aarch64/smp.rs | `init` | 124 | 12 | yes |
| arch/aarch64/smp.rs | `read_mpidr` | 135 | 16 | yes |
| arch/aarch64/smp.rs | `mpidr_to_index` | 150 | 12 | yes |
| arch/aarch64/smp.rs | `index_to_mpidr` | 161 | 29 | yes |
| arch/aarch64/smp.rs | `psci_call` | 189 | 42 | yes |
| arch/aarch64/smp.rs | `detect_psci_from_dtb` | 230 | 113 | yes |
| arch/aarch64/smp.rs | `signal_boot_complete` | 342 | 17 | yes |
| arch/aarch64/smp.rs | `is_boot_complete` | 358 | 24 | yes |
| arch/aarch64/smp.rs | `secondary_start` | 381 | 18 | yes |
| arch/aarch64/smp.rs | `start_secondaries` | 398 | 164 | yes |
| arch/aarch64/smp.rs | `num_started_cpus` | 561 | 22 | yes |
| arch/aarch64/smp.rs | `as_ptr` | 582 | 13 | yes |
| arch/aarch64/smp.rs | `init_per_cpu_intr_stacks` | 594 | 30 | yes |
| arch/aarch64/thread.rs | `default` | 35 | 8 | yes |
| arch/aarch64/thread.rs | `zeroed` | 42 | 74 | yes |
| arch/aarch64/thread.rs | `new` | 115 | 22 | yes |
| arch/aarch64/thread.rs | `tp` | 136 | 7 | yes |
| arch/aarch64/thread.rs | `set_tp` | 142 | 7 | yes |
| arch/aarch64/thread.rs | `mark_fpu_clean` | 148 | 7 | yes |
| arch/aarch64/thread.rs | `get_fs` | 154 | 13 | yes |
| arch/aarch64/thread.rs | `fpu_save` | 166 | 40 | yes |
| arch/aarch64/thread.rs | `fpu_restore` | 205 | 38 | yes |
| arch/aarch64/thread.rs | `mark_vstate_clean` | 242 | 5 | yes |
| arch/aarch64/thread.rs | `default` | 246 | 34 | yes |
| arch/aarch64/trap.rs | `esr_ec` | 73 | 7 | yes |
| arch/aarch64/trap.rs | `esr_iss` | 79 | 46 | yes |
| arch/aarch64/trap.rs | `dfsc_class` | 124 | 20 | yes |
| arch/aarch64/trap.rs | `current_pt_regs` | 143 | 7 | yes |
| arch/aarch64/trap.rs | `current_task_pt_regs` | 149 | 16 | yes |
| arch/aarch64/trap.rs | `pt_regs_rip_for_cpu` | 164 | 228 | yes |
| arch/aarch64/trap.rs | `oops_show_regs` | 391 | 40 | yes |
| arch/aarch64/trap.rs | `handle_syscall` | 430 | 37 | yes |
| arch/aarch64/trap.rs | `handle_external_interrupt` | 466 | 39 | yes |
| arch/aarch64/trap.rs | `handle_timer_interrupt` | 504 | 64 | yes |
| arch/aarch64/trap.rs | `handle_illegal_instruction` | 567 | 24 | yes |
| arch/aarch64/trap.rs | `sig_has_handler` | 590 | 12 | yes |
| arch/aarch64/trap.rs | `sig_blocked_current` | 601 | 7 | yes |
| arch/aarch64/trap.rs | `sigsegv_has_handler` | 607 | 5 | yes |
| arch/aarch64/trap.rs | `sigsegv_blocked_current` | 611 | 5 | yes |
| arch/aarch64/trap.rs | `sigbus_has_handler` | 615 | 5 | yes |
| arch/aarch64/trap.rs | `sigill_has_handler` | 619 | 11 | yes |
| arch/aarch64/trap.rs | `fault_sigsegv_route` | 629 | 63 | yes |
| arch/aarch64/trap.rs | `handle_page_fault` | 691 | 158 | yes |
| arch/aarch64/trap.rs | `init` | 848 | 3 | yes |
| arch/aarch64/trap.rs | `aarch64_vector_table` | 850 | 15 | yes |
| arch/aarch64/trap.rs | `init_syscall` | 864 | 9 | yes |
| arch/aarch64/trap.rs | `enable_timer_interrupt` | 872 | 23 | yes |
| arch/aarch64/trap.rs | `disable_timer_interrupt` | 894 | 14 | yes |
| arch/aarch64/trap.rs | `enable_external_interrupt` | 907 | 14 | yes |
| arch/aarch64/uaccess.rs | `access_ok` | 43 | 22 | yes |
| arch/aarch64/uaccess.rs | `__aa64_copy_to_user` | 64 | 2 | yes |
| arch/aarch64/uaccess.rs | `__aa64_copy_from_user` | 65 | 2 | yes |
| arch/aarch64/uaccess.rs | `__aa64_clear_user` | 66 | 2 | yes |
| arch/aarch64/uaccess.rs | `__aa64_futex_cmpxchg_u32` | 67 | 11 | yes |
| arch/aarch64/uaccess.rs | `futex_cmpxchg_u32_user` | 77 | 31 | yes |
| arch/aarch64/uaccess.rs | `copy_to_user` | 107 | 22 | yes |
| arch/aarch64/uaccess.rs | `copy_from_user` | 128 | 30 | yes |
| arch/aarch64/uaccess.rs | `clear_user` | 157 | 17 | yes |
| arch/aarch64/uaccess.rs | `put_user` | 173 | 16 | yes |
| arch/aarch64/uaccess.rs | `get_user` | 188 | 26 | yes |
| arch/aarch64/uaccess.rs | `strncpy_from_user` | 213 | 46 | yes |
| arch/aarch64/uaccess.rs | `strnlen_user` | 258 | 39 | yes |
| arch/aarch64/uaccess.rs | `selftest` | 296 | 4 | yes |
| arch/aarch64/uaccess.rs | `emit` | 299 | 109 | yes |
