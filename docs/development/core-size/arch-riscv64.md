# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch/riscv64 — hot 7055 lines (stay) / cold 4722 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/riscv64/boot.rs | `get_core_id` | 17 | 11 | yes |
| arch/riscv64/boot.rs | `get_dtb_pointer` | 28 | 5 | yes |
| arch/riscv64/context.rs | `current_task` | 53 | 77 | yes |
| arch/riscv64/context.rs | `set_prev_task` | 130 | 30 | yes |
| arch/riscv64/context.rs | `__switch_mm_linear` | 160 | 18 | yes |
| arch/riscv64/context.rs | `switch_mm` | 178 | 27 | yes |
| arch/riscv64/context.rs | `get_current_satp` | 205 | 17 | yes |
| arch/riscv64/context.rs | `__switch_to` | 222 | 22 | yes |
| arch/riscv64/context.rs | `context_switch` | 244 | 7 | yes |
| arch/riscv64/context.rs | `context_switch_body` | 251 | 151 | yes |
| arch/riscv64/cpu.rs | `get_core_id` | 11 | 11 | yes |
| arch/riscv64/cpu.rs | `get_thread_id_of` | 22 | 10 | yes |
| arch/riscv64/cpu.rs | `get_thread_id` | 32 | 12 | yes |
| arch/riscv64/cpu.rs | `set_thread_id` | 44 | 9 | yes |
| arch/riscv64/cpu.rs | `get_counter_freq` | 53 | 7 | yes |
| arch/riscv64/cpu.rs | `read_counter` | 60 | 22 | yes |
| arch/riscv64/cpu.rs | `enable_irq` | 82 | 12 | yes |
| arch/riscv64/cpu.rs | `disable_irq` | 94 | 12 | yes |
| arch/riscv64/cpu.rs | `wfi` | 106 | 11 | yes |
| arch/riscv64/cpu.rs | `read_time` | 117 | 12 | yes |
| arch/riscv64/cpu.rs | `isb` | 129 | 9 | yes |
| arch/riscv64/cpu.rs | `dsb` | 138 | 9 | yes |
| arch/riscv64/cpu.rs | `dmb` | 147 | 9 | yes |
| arch/riscv64/cpu.rs | `get_interrupts_state` | 156 | 12 | yes |
| arch/riscv64/cpu.rs | `save_and_disable_irq` | 168 | 12 | yes |
| arch/riscv64/cpu.rs | `restore_irq` | 180 | 5 | yes |
| arch/riscv64/ipi.rs | `bit` | 48 | 20 |  |
| arch/riscv64/ipi.rs | `request_ipi` | 68 | 20 |  |
| arch/riscv64/ipi.rs | `send_ipi_type` | 88 | 19 | yes |
| arch/riscv64/ipi.rs | `send_reschedule_ipi` | 107 | 15 | yes |
| arch/riscv64/ipi.rs | `flush_tlb_others` | 122 | 18 | yes |
| arch/riscv64/ipi.rs | `handle_software_ipi` | 140 | 53 | yes |
| arch/riscv64/ipi.rs | `new` | 193 | 20 | yes |
| arch/riscv64/ipi.rs | `csd_init` | 213 | 14 |  |
| arch/riscv64/ipi.rs | `smp_call_function` | 227 | 72 | yes |
| arch/riscv64/ipi.rs | `csd_flush_queue` | 299 | 47 |  |
| arch/riscv64/ipi.rs | `ipi_reschedule_handler` | 346 | 12 |  |
| arch/riscv64/ipi.rs | `ipi_call_function_handler` | 358 | 4 |  |
| arch/riscv64/ipi.rs | `ipi_stop_handler` | 362 | 10 |  |
| arch/riscv64/ipi.rs | `ipi_irq_work_handler` | 372 | 7 |  |
| arch/riscv64/ipi.rs | `ipi_tlb_flush_handler` | 379 | 17 |  |
| arch/riscv64/ipi.rs | `ipi_irq_handler` | 396 | 23 |  |
| arch/riscv64/ipi.rs | `register_irq_handlers` | 419 | 13 |  |
| arch/riscv64/ipi.rs | `handle_ipi` | 432 | 27 |  |
| arch/riscv64/ipi.rs | `init` | 459 | 44 | yes |
| arch/riscv64/ipi.rs | `per_hart_enable_ssoftware` | 503 | 10 | yes |
| arch/riscv64/mm/asid.rs | `alloc_asid` | 45 | 36 | yes |
| arch/riscv64/mm/asid.rs | `free_asid` | 81 | 11 |  |
| arch/riscv64/mm/asid.rs | `asid_usage_count` | 92 | 15 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_all` | 107 | 16 |  |
| arch/riscv64/mm/asid.rs | `sfence_vma_addr` | 123 | 19 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_asid` | 142 | 16 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_page` | 158 | 16 | yes |
| arch/riscv64/mm/asid.rs | `flush_tlb_range` | 174 | 11 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_kernel` | 185 | 15 |  |
| arch/riscv64/mm/asid.rs | `build_satp` | 200 | 11 | yes |
| arch/riscv64/mm/asid.rs | `satp_to_asid` | 211 | 6 |  |
| arch/riscv64/mm/asid.rs | `satp_to_ppn` | 217 | 6 | yes |
| arch/riscv64/mm/asid.rs | `read_satp` | 223 | 18 | yes |
| arch/riscv64/mm/asid.rs | `write_satp` | 241 | 29 | yes |
| arch/riscv64/mm/asid.rs | `new` | 270 | 8 | yes |
| arch/riscv64/mm/asid.rs | `asid` | 278 | 5 | yes |
| arch/riscv64/mm/asid.rs | `alloc` | 283 | 11 | yes |
| arch/riscv64/mm/asid.rs | `free` | 294 | 9 | yes |
| arch/riscv64/mm/asid.rs | `generation` | 303 | 8 |  |
| arch/riscv64/mm/asid.rs | `print_asid_status` | 311 | 13 |  |
| arch/riscv64/mm/exception.rs | `fixup_exception` | 79 | 22 | yes |
| arch/riscv64/mm/exception.rs | `exception_table_empty` | 101 | 10 | yes |
| arch/riscv64/mm/exception.rs | `exception_table_count` | 111 | 17 | yes |
| arch/riscv64/mm/exception.rs | `send_signal` | 128 | 20 | yes |
| arch/riscv64/mm/exception.rs | `in_interrupt` | 148 | 7 | yes |
| arch/riscv64/mm/exception.rs | `bad_area` | 155 | 21 | yes |
| arch/riscv64/mm/exception.rs | `no_context` | 176 | 19 | yes |
| arch/riscv64/mm/exception.rs | `do_page_fault` | 195 | 535 | yes |
| arch/riscv64/mm/fixmap.rs | `fix_to_virt` | 74 | 8 |  |
| arch/riscv64/mm/fixmap.rs | `virt_to_fix` | 82 | 10 |  |
| arch/riscv64/mm/fixmap.rs | `is_fixmap_addr` | 92 | 18 |  |
| arch/riscv64/mm/fixmap.rs | `set_fixmap` | 110 | 28 |  |
| arch/riscv64/mm/fixmap.rs | `clear_fixmap` | 138 | 54 |  |
| arch/riscv64/mm/fixmap.rs | `init_uart_fixmap` | 192 | 25 |  |
| arch/riscv64/mm/fixmap.rs | `uart_virt_addr` | 217 | 13 |  |
| arch/riscv64/mm/fixmap.rs | `is_uart_fixmap_initialized` | 230 | 16 |  |
| arch/riscv64/mm/fixmap.rs | `copy_fixmap_to_user` | 246 | 23 |  |
| arch/riscv64/mm/fixmap.rs | `print_fixmap_status` | 269 | 5 |  |
| arch/riscv64/mm/memory_layout.rs | `is_linear_mapping` | 106 | 130 |  |
| arch/riscv64/mm/memory_layout.rs | `mmio_alias` | 236 | 193 |  |
| arch/riscv64/mm/memory_layout.rs | `new` | 429 | 13 | yes |
| arch/riscv64/mm/memory_layout.rs | `bits` | 442 | 6 | yes |
| arch/riscv64/mm/memory_layout.rs | `is_aligned` | 448 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `floor` | 454 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `ceil` | 460 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `page_offset` | 466 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `vpn` | 472 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `as_u64` | 478 | 6 | yes |
| arch/riscv64/mm/memory_layout.rs | `as_usize` | 484 | 13 | yes |
| arch/riscv64/mm/memory_layout.rs | `new` | 497 | 6 | yes |
| arch/riscv64/mm/memory_layout.rs | `bits` | 503 | 6 | yes |
| arch/riscv64/mm/memory_layout.rs | `is_aligned` | 509 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `floor` | 515 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `ceil` | 521 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `ppn` | 527 | 9 | yes |
| arch/riscv64/mm/memory_layout.rs | `phys_to_virt` | 536 | 11 | yes |
| arch/riscv64/mm/memory_layout.rs | `virt_to_phys` | 547 | 31 | yes |
| arch/riscv64/mm/mm_ops.rs | `enable` | 54 | 11 | yes |
| arch/riscv64/mm/mm_ops.rs | `disable` | 65 | 10 | yes |
| arch/riscv64/mm/mm_ops.rs | `flush_tlb` | 75 | 6 |  |
| arch/riscv64/mm/mm_ops.rs | `flush_tlb_addr_page` | 81 | 9 |  |
| arch/riscv64/mm/mm_ops.rs | `map_single_page` | 90 | 49 |  |
| arch/riscv64/mm/mm_ops.rs | `unmap_vma` | 139 | 10 |  |
| arch/riscv64/mm/mm_ops.rs | `unmap_pages` | 149 | 128 |  |
| arch/riscv64/mm/mm_ops.rs | `unmap_one_user_leaf_locked` | 277 | 43 |  |
| arch/riscv64/mm/mm_ops.rs | `clear_pte` | 320 | 46 |  |
| arch/riscv64/mm/mm_ops.rs | `perm_to_flags` | 366 | 36 |  |
| arch/riscv64/mm/mm_ops.rs | `walk` | 402 | 113 | yes |
| arch/riscv64/mm/mm_ops.rs | `alloc_user_phys_page` | 515 | 10 |  |
| arch/riscv64/mm/mm_ops.rs | `create_user_address_space` | 525 | 56 |  |
| arch/riscv64/mm/mm_ops.rs | `copy_kernel_mappings` | 581 | 95 |  |
| arch/riscv64/mm/mm_ops.rs | `map_user_page` | 676 | 32 |  |
| arch/riscv64/mm/mm_ops.rs | `pte_lock_for` | 708 | 27 |  |
| arch/riscv64/mm/mm_ops.rs | `pte_flag_bits` | 735 | 24 |  |
| arch/riscv64/mm/mm_ops.rs | `leaf_pte_replace` | 759 | 47 |  |
| arch/riscv64/mm/mm_ops.rs | `leaf_pte_install` | 806 | 33 |  |
| arch/riscv64/mm/mm_ops.rs | `map_user_region` | 839 | 44 | yes |
| arch/riscv64/mm/mm_ops.rs | `alloc_and_map_user_memory` | 883 | 112 |  |
| arch/riscv64/mm/mm_ops.rs | `alloc_and_map_to_kernel_table` | 995 | 56 |  |
| arch/riscv64/mm/mm_ops.rs | `alloc_and_map_to_user_table` | 1051 | 134 |  |
| arch/riscv64/mm/mm_ops.rs | `pte_is_shared_cache` | 1185 | 13 |  |
| arch/riscv64/mm/mm_ops.rs | `release_cache_mapping_pin` | 1198 | 20 |  |
| arch/riscv64/mm/mm_ops.rs | `copy_page_table_cow` | 1218 | 292 |  |
| arch/riscv64/mm/mm_ops.rs | `handle_cow_fault` | 1510 | 24 | yes |
| arch/riscv64/mm/mm_ops.rs | `drop` | 1534 | 225 | yes |
| arch/riscv64/mm/mm_ops.rs | `is_cow_page` | 1759 | 48 |  |
| arch/riscv64/mm/mm_ops.rs | `check_pte_permissions` | 1807 | 58 |  |
| arch/riscv64/mm/mmu_init.rs | `root_page_table_ppn` | 48 | 49 | yes |
| arch/riscv64/mm/mmu_init.rs | `from_u8` | 97 | 14 |  |
| arch/riscv64/mm/mmu_init.rs | `get_alloc_stage` | 111 | 6 |  |
| arch/riscv64/mm/mmu_init.rs | `alloc_stage_is_late` | 117 | 5 | yes |
| arch/riscv64/mm/mmu_init.rs | `pt_ops_set_fixmap` | 122 | 5 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_ops_set_late` | 127 | 6 |  |
| arch/riscv64/mm/mmu_init.rs | `is_frame_allocator_ready` | 133 | 10 |  |
| arch/riscv64/mm/mmu_init.rs | `alloc_page_table` | 143 | 148 |  |
| arch/riscv64/mm/mmu_init.rs | `free_page_table_checked` | 291 | 24 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp_kernel_tree_boot` | 315 | 52 |  |
| arch/riscv64/mm/mmu_init.rs | `free_page_table` | 367 | 71 |  |
| arch/riscv64/mm/mmu_init.rs | `new` | 438 | 10 | yes |
| arch/riscv64/mm/mmu_init.rs | `bit_index` | 448 | 10 |  |
| arch/riscv64/mm/mmu_init.rs | `live_bit` | 458 | 10 |  |
| arch/riscv64/mm/mmu_init.rs | `set_live_bit` | 468 | 16 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp_boot` | 484 | 16 |  |
| arch/riscv64/mm/mmu_init.rs | `take_returns` | 500 | 12 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp` | 512 | 34 |  |
| arch/riscv64/mm/mmu_init.rs | `take` | 546 | 111 | yes |
| arch/riscv64/mm/mmu_init.rs | `new` | 657 | 27 | yes |
| arch/riscv64/mm/mmu_init.rs | `new` | 684 | 12 | yes |
| arch/riscv64/mm/mmu_init.rs | `register_fork_root` | 696 | 7 |  |
| arch/riscv64/mm/mmu_init.rs | `task_pid_alive` | 703 | 26 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp_root` | 729 | 6 |  |
| arch/riscv64/mm/mmu_init.rs | `is_boot` | 735 | 7 |  |
| arch/riscv64/mm/mmu_init.rs | `peek` | 742 | 6 |  |
| arch/riscv64/mm/mmu_init.rs | `freed_by_peek` | 748 | 9 |  |
| arch/riscv64/mm/mmu_init.rs | `poison_table` | 757 | 12 | yes |
| arch/riscv64/mm/mmu_init.rs | `is_poisoned` | 769 | 12 | yes |
| arch/riscv64/mm/mmu_init.rs | `is_live_table` | 781 | 7 | yes |
| arch/riscv64/mm/mmu_init.rs | `live_count` | 788 | 14 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_free_ring_prev_free_pid` | 802 | 17 |  |
| arch/riscv64/mm/mmu_init.rs | `new` | 819 | 29 | yes |
| arch/riscv64/mm/mmu_init.rs | `is_framebuffer_frame` | 848 | 30 |  |
| arch/riscv64/mm/mmu_init.rs | `new` | 878 | 12 | yes |
| arch/riscv64/mm/mmu_init.rs | `pte_install_log` | 890 | 21 |  |
| arch/riscv64/mm/mmu_init.rs | `owner_maps_pfn` | 911 | 50 |  |
| arch/riscv64/mm/mmu_init.rs | `textfree_owner_maps` | 961 | 29 |  |
| arch/riscv64/mm/mmu_init.rs | `release_user_leaf` | 990 | 174 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_pending_drain` | 1164 | 2 |  |
| arch/riscv64/mm/mmu_init.rs | `free_user_page_tables` | 1166 | 344 | yes |
| arch/riscv64/mm/mmu_init.rs | `pt_alloc_fail` | 1510 | 17 |  |
| arch/riscv64/mm/mmu_init.rs | `map_page_noflush` | 1527 | 163 |  |
| arch/riscv64/mm/mmu_init.rs | `map_page` | 1690 | 11 |  |
| arch/riscv64/mm/mmu_init.rs | `map_region` | 1701 | 20 |  |
| arch/riscv64/mm/mmu_init.rs | `map_pmd_huge_page` | 1721 | 55 |  |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_page_noflush` | 1776 | 75 |  |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_page` | 1851 | 22 | yes |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_region` | 1873 | 35 |  |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_region_huge` | 1908 | 24 |  |
| arch/riscv64/mm/mmu_init.rs | `map_device_page` | 1932 | 16 |  |
| arch/riscv64/mm/mmu_init.rs | `best_map_size` | 1948 | 105 |  |
| arch/riscv64/mm/mmu_init.rs | `init` | 2053 | 87 | yes |
| arch/riscv64/mm/mmu_init.rs | `kernel_device_window_pte` | 2140 | 25 |  |
| arch/riscv64/mm/mmu_init.rs | `setup_device_mappings` | 2165 | 67 |  |
| arch/riscv64/mm/mmu_init.rs | `setup_linear_mapping` | 2232 | 44 |  |
| arch/riscv64/mm/mmu_init.rs | `enable` | 2276 | 11 | yes |
| arch/riscv64/mm/mmu_init.rs | `map_identity` | 2287 | 10 |  |
| arch/riscv64/mm/mmu_init.rs | `get_kernel_page_table_ppn` | 2297 | 14 |  |
| arch/riscv64/mm/mmu_init.rs | `forensics_scan_trees` | 2311 | 133 |  |
| arch/riscv64/mm/mod.rs | `register_in_tree_mm_policy` | 63 | 5 |  |
| arch/riscv64/mm/page_fault.rs | `filemap_pin_cache_page` | 51 | 60 |  |
| arch/riscv64/mm/page_fault.rs | `get_user_phys` | 111 | 52 |  |
| arch/riscv64/mm/page_fault.rs | `write_pte_raw` | 163 | 19 |  |
| arch/riscv64/mm/page_fault.rs | `read_pte_raw` | 182 | 33 |  |
| arch/riscv64/mm/pagetable.rs | `new` | 68 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `from_bits` | 74 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `bits` | 80 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `is_valid` | 86 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `is_readable` | 92 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `is_writable` | 98 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `is_executable` | 104 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `is_user` | 110 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `is_leaf` | 117 | 7 | yes |
| arch/riscv64/mm/pagetable.rs | `ppn` | 124 | 9 | yes |
| arch/riscv64/mm/pagetable.rs | `ppn_for_2mb_page` | 133 | 12 |  |
| arch/riscv64/mm/pagetable.rs | `new_table` | 145 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `new_page_kernel` | 151 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `new_page_user` | 157 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `new_page_ro` | 163 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `default` | 169 | 16 | yes |
| arch/riscv64/mm/pagetable.rs | `new` | 185 | 8 | yes |
| arch/riscv64/mm/pagetable.rs | `get` | 193 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `set` | 199 | 5 | yes |
| arch/riscv64/mm/pagetable.rs | `zero` | 204 | 8 |  |
| arch/riscv64/mm/pagetable.rs | `default` | 212 | 26 | yes |
| arch/riscv64/mm/pagetable.rs | `new` | 238 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `sv39` | 244 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `bits` | 250 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `mode` | 256 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `asid` | 262 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `ppn` | 268 | 6 | yes |
| arch/riscv64/mm/pagetable.rs | `is_bare` | 274 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `is_sv39` | 280 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `get_satp` | 286 | 9 | yes |
| arch/riscv64/mm/unsafe_edges.rs | `get_page_table_virt` | 45 | 40 | yes |
| arch/riscv64/mm/unsafe_edges.rs | `from_phys` | 85 | 22 | yes |
| arch/riscv64/mm/unsafe_edges.rs | `from_phys_live` | 107 | 5 |  |
| arch/riscv64/mm/unsafe_edges.rs | `as_phys_base` | 112 | 6 |  |
| arch/riscv64/mm/unsafe_edges.rs | `get_raw` | 118 | 8 |  |
| arch/riscv64/mm/unsafe_edges.rs | `get` | 126 | 6 | yes |
| arch/riscv64/mm/unsafe_edges.rs | `set` | 132 | 11 | yes |
| arch/riscv64/mm/unsafe_edges.rs | `all_zero` | 143 | 11 |  |
| arch/riscv64/mm/unsafe_edges.rs | `zero_entries` | 154 | 12 |  |
| arch/riscv64/mm/unsafe_edges.rs | `static_root` | 166 | 19 |  |
| arch/riscv64/mm/unsafe_edges.rs | `copy_page` | 185 | 8 |  |
| arch/riscv64/mod.rs | `arch_init` | 30 | 4 |  |
| arch/riscv64/mod.rs | `init` | 34 | 40 | yes |
| arch/riscv64/mod.rs | `print_cpu_info` | 74 | 7 |  |
| arch/riscv64/mod.rs | `enable_interrupts` | 81 | 27 |  |
| arch/riscv64/mod.rs | `cpu_id` | 108 | 28 | yes |
| arch/riscv64/process.rs | `start_thread` | 40 | 48 |  |
| arch/riscv64/process.rs | `flush_thread` | 88 | 42 |  |
| arch/riscv64/process.rs | `current_pt_regs` | 130 | 16 | yes |
| arch/riscv64/process.rs | `task_pt_regs` | 146 | 13 |  |
| arch/riscv64/process.rs | `user_stack_pointer` | 159 | 8 |  |
| arch/riscv64/process.rs | `set_user_stack_pointer` | 167 | 8 |  |
| arch/riscv64/process.rs | `instruction_pointer` | 175 | 8 | yes |
| arch/riscv64/process.rs | `set_instruction_pointer` | 183 | 14 |  |
| arch/riscv64/process.rs | `is_user_address` | 197 | 9 |  |
| arch/riscv64/pt_regs.rs | `new` | 116 | 18 | yes |
| arch/riscv64/pt_regs.rs | `user_mode` | 134 | 6 | yes |
| arch/riscv64/pt_regs.rs | `kernel_mode` | 140 | 6 | yes |
| arch/riscv64/pt_regs.rs | `syscall_nr` | 146 | 8 | yes |
| arch/riscv64/pt_regs.rs | `syscall_args` | 154 | 13 |  |
| arch/riscv64/pt_regs.rs | `set_return_value` | 167 | 8 |  |
| arch/riscv64/pt_regs.rs | `set_return_error` | 175 | 8 |  |
| arch/riscv64/pt_regs.rs | `syscall_rollback` | 183 | 6 |  |
| arch/riscv64/pt_regs.rs | `instruction_pointer` | 189 | 6 | yes |
| arch/riscv64/pt_regs.rs | `set_instruction_pointer` | 195 | 6 |  |
| arch/riscv64/pt_regs.rs | `user_stack_pointer` | 201 | 6 |  |
| arch/riscv64/pt_regs.rs | `set_user_stack_pointer` | 207 | 6 |  |
| arch/riscv64/pt_regs.rs | `frame_pointer` | 213 | 6 |  |
| arch/riscv64/pt_regs.rs | `irqs_disabled` | 219 | 7 |  |
| arch/riscv64/pt_regs.rs | `mark_user_frame` | 226 | 7 |  |
| arch/riscv64/pt_regs.rs | `mark_kernel_frame` | 233 | 89 |  |
| arch/riscv64/pt_regs.rs | `from_cause` | 322 | 34 | yes |
| arch/riscv64/pt_regs.rs | `is_interrupt` | 356 | 8 |  |
| arch/riscv64/pt_regs.rs | `is_exception` | 364 | 5 |  |
| arch/riscv64/pt_regs.rs | `is_page_fault` | 369 | 12 |  |
| arch/riscv64/pt_regs.rs | `in_interrupt` | 381 | 6 | yes |
| arch/riscv64/pt_regs.rs | `in_task` | 387 | 53 | yes |
| arch/riscv64/smp.rs | `boot_hart_id` | 31 | 4 | yes |
| arch/riscv64/smp.rs | `mark_cpu_started` | 35 | 12 | yes |
| arch/riscv64/smp.rs | `cpu_started` | 47 | 18 | yes |
| arch/riscv64/smp.rs | `cpu_id` | 65 | 36 | yes |
| arch/riscv64/smp.rs | `is_boot_hart` | 101 | 13 | yes |
| arch/riscv64/smp.rs | `init` | 114 | 9 | yes |
| arch/riscv64/smp.rs | `secondary_start` | 123 | 23 | yes |
| arch/riscv64/smp.rs | `signal_boot_complete` | 146 | 5 | yes |
| arch/riscv64/smp.rs | `is_boot_complete` | 151 | 6 | yes |
| arch/riscv64/smp.rs | `start_secondaries` | 157 | 169 | yes |
| arch/riscv64/smp.rs | `num_started_cpus` | 326 | 21 | yes |
| arch/riscv64/smp.rs | `as_ptr` | 347 | 14 | yes |
| arch/riscv64/smp.rs | `init_per_cpu_intr_stacks` | 361 | 34 | yes |
| arch/riscv64/thread.rs | `new` | 100 | 34 | yes |
| arch/riscv64/thread.rs | `save_fpu` | 134 | 27 | yes |
| arch/riscv64/thread.rs | `restore_fpu` | 161 | 24 | yes |
| arch/riscv64/thread.rs | `fpu_save_for_switch` | 185 | 28 | yes |
| arch/riscv64/thread.rs | `get_fs` | 213 | 20 | yes |
| arch/riscv64/thread.rs | `vstate_save_for_switch` | 233 | 27 | yes |
| arch/riscv64/thread.rs | `restore_vstate` | 260 | 19 | yes |
| arch/riscv64/thread.rs | `save_vstate` | 279 | 21 | yes |
| arch/riscv64/thread.rs | `vstate_first_use` | 300 | 12 | yes |
| arch/riscv64/thread.rs | `mark_vstate_clean` | 312 | 14 | yes |
| arch/riscv64/thread.rs | `__fstate_save` | 326 | 62 | yes |
| arch/riscv64/thread.rs | `__fstate_restore` | 388 | 55 | yes |
| arch/riscv64/thread.rs | `tp` | 443 | 6 | yes |
| arch/riscv64/thread.rs | `set_tp` | 449 | 7 | yes |
| arch/riscv64/thread.rs | `mark_fpu_clean` | 456 | 6 | yes |
| arch/riscv64/thread.rs | `default` | 462 | 13 | yes |
| arch/riscv64/thread.rs | `fpu_init` | 475 | 75 | yes |
| arch/riscv64/trap.rs | `current_task_pt_regs` | 45 | 31 | yes |
| arch/riscv64/trap.rs | `current_pt_regs` | 76 | 9 | yes |
| arch/riscv64/trap.rs | `pt_regs_rip_for_cpu` | 85 | 24 | yes |
| arch/riscv64/trap.rs | `init` | 109 | 6 | yes |
| arch/riscv64/trap.rs | `trap_entry` | 115 | 22 | yes |
| arch/riscv64/trap.rs | `init_syscall` | 137 | 4 | yes |
| arch/riscv64/trap.rs | `enable_timer_interrupt` | 141 | 25 | yes |
| arch/riscv64/trap.rs | `disable_timer_interrupt` | 166 | 7 | yes |
| arch/riscv64/trap.rs | `enable_external_interrupt` | 173 | 30 | yes |
| arch/riscv64/trap.rs | `putfrag` | 203 | 8 | yes |
| arch/riscv64/trap.rs | `puthex` | 211 | 9 | yes |
| arch/riscv64/trap.rs | `putb` | 220 | 381 | yes |
| arch/riscv64/trap.rs | `irq_exit_maybe_preempt_kernel` | 601 | 70 | yes |
| arch/riscv64/trap.rs | `handle_timer_interrupt` | 671 | 68 | yes |
| arch/riscv64/trap.rs | `handle_software_interrupt` | 739 | 20 | yes |
| arch/riscv64/trap.rs | `handle_external_interrupt` | 759 | 17 | yes |
| arch/riscv64/trap.rs | `handle_syscall` | 776 | 89 | yes |
| arch/riscv64/trap.rs | `handle_illegal_instruction` | 865 | 207 | yes |
| arch/riscv64/trap.rs | `sigill_mapping_forensics` | 1072 | 181 | yes |
| arch/riscv64/trap.rs | `handle_breakpoint` | 1253 | 105 | yes |
| arch/riscv64/trap.rs | `sigsegv_has_handler` | 1358 | 14 | yes |
| arch/riscv64/trap.rs | `sigsegv_blocked_current` | 1372 | 6 | yes |
| arch/riscv64/trap.rs | `sigbus_has_handler` | 1378 | 53 | yes |
| arch/riscv64/trap.rs | `handle_page_fault` | 1431 | 538 | yes |
| arch/riscv64/trap.rs | `oops_show_regs` | 1969 | 60 | yes |
| arch/riscv64/trap.rs | `handle_unknown_exception` | 2029 | 37 | yes |
| arch/riscv64/trap.rs | `current_trap_frame` | 2066 | 65 | yes |
| arch/riscv64/trap.rs | `phex` | 2131 | 8 | yes |
| arch/riscv64/trap.rs | `pdec` | 2139 | 76 | yes |
| arch/riscv64/uaccess.rs | `access_ok` | 59 | 39 | yes |
| arch/riscv64/uaccess.rs | `__copy_to_user` | 98 | 1 | yes |
| arch/riscv64/uaccess.rs | `__copy_from_user` | 99 | 1 | yes |
| arch/riscv64/uaccess.rs | `__futex_cmpxchg_u32` | 100 | 1 | yes |
| arch/riscv64/uaccess.rs | `__clear_user` | 101 | 17 | yes |
| arch/riscv64/uaccess.rs | `copy_to_user` | 118 | 35 | yes |
| arch/riscv64/uaccess.rs | `fault_in_write` | 153 | 104 | yes |
| arch/riscv64/uaccess.rs | `copy_from_user` | 257 | 41 | yes |
| arch/riscv64/uaccess.rs | `clear_user` | 298 | 38 | yes |
| arch/riscv64/uaccess.rs | `get_user` | 336 | 31 | yes |
| arch/riscv64/uaccess.rs | `put_user` | 367 | 29 | yes |
| arch/riscv64/uaccess.rs | `strncpy_from_user` | 396 | 80 | yes |
| arch/riscv64/uaccess.rs | `strnlen_user` | 476 | 28 | yes |
| arch/riscv64/uaccess.rs | `futex_cmpxchg_u32_user` | 504 | 13 | yes |
| arch/riscv64/unsafe_edges.rs | `from_trap` | 53 | 10 | yes |
| arch/riscv64/unsafe_edges.rs | `regs` | 63 | 8 |  |
| arch/riscv64/unsafe_edges.rs | `deref` | 71 | 7 | yes |
| arch/riscv64/unsafe_edges.rs | `deref_mut` | 78 | 4 | yes |
| arch/riscv64/vext.rs | `vmv_v_i0` | 61 | 4 |  |
| arch/riscv64/vext.rs | `vs1r_v` | 65 | 4 |  |
| arch/riscv64/vext.rs | `vl1r_v` | 69 | 32 |  |
| arch/riscv64/vext.rs | `hart_has_v` | 101 | 8 | yes |
| arch/riscv64/vext.rs | `prime_hart_has_v` | 109 | 12 |  |
| arch/riscv64/vext.rs | `vlen_fits` | 121 | 12 |  |
| arch/riscv64/vext.rs | `vs_disable` | 133 | 25 |  |
| arch/riscv64/vext.rs | `vstate_first_use_init` | 158 | 71 | yes |
| arch/riscv64/vext.rs | `vstate_save` | 229 | 93 | yes |
| arch/riscv64/vext.rs | `vstate_restore` | 322 | 85 | yes |
| arch/riscv64/vext.rs | `is_vector_instr32` | 407 | 7 | yes |
