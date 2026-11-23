# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch/riscv64 — hot 5665 lines (stay) / cold 6475 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/riscv64/boot.rs | `get_core_id` | 17 | 12 | yes |
| arch/riscv64/boot.rs | `get_dtb_pointer` | 28 | 6 | yes |
| arch/riscv64/context.rs | `current_task` | 53 | 78 | yes |
| arch/riscv64/context.rs | `set_prev_task` | 130 | 31 | yes |
| arch/riscv64/context.rs | `__switch_mm_linear` | 160 | 19 | yes |
| arch/riscv64/context.rs | `switch_mm` | 178 | 28 | yes |
| arch/riscv64/context.rs | `get_current_satp` | 205 | 18 | yes |
| arch/riscv64/context.rs | `__switch_to` | 222 | 23 | yes |
| arch/riscv64/context.rs | `context_switch` | 244 | 8 | yes |
| arch/riscv64/context.rs | `context_switch_body` | 251 | 152 | yes |
| arch/riscv64/cpu.rs | `get_core_id` | 11 | 12 | yes |
| arch/riscv64/cpu.rs | `get_thread_id_of` | 22 | 11 | yes |
| arch/riscv64/cpu.rs | `get_thread_id` | 32 | 13 | yes |
| arch/riscv64/cpu.rs | `set_thread_id` | 44 | 10 | yes |
| arch/riscv64/cpu.rs | `get_counter_freq` | 53 | 8 | yes |
| arch/riscv64/cpu.rs | `read_counter` | 60 | 23 | yes |
| arch/riscv64/cpu.rs | `enable_irq` | 82 | 13 | yes |
| arch/riscv64/cpu.rs | `disable_irq` | 94 | 13 | yes |
| arch/riscv64/cpu.rs | `wfi` | 106 | 12 | yes |
| arch/riscv64/cpu.rs | `read_time` | 117 | 13 | yes |
| arch/riscv64/cpu.rs | `isb` | 129 | 10 | yes |
| arch/riscv64/cpu.rs | `dsb` | 138 | 10 | yes |
| arch/riscv64/cpu.rs | `dmb` | 147 | 10 | yes |
| arch/riscv64/cpu.rs | `get_interrupts_state` | 156 | 13 | yes |
| arch/riscv64/cpu.rs | `save_and_disable_irq` | 168 | 13 | yes |
| arch/riscv64/cpu.rs | `restore_irq` | 180 | 6 | yes |
| arch/riscv64/ipi.rs | `bit` | 48 | 21 |  |
| arch/riscv64/ipi.rs | `request_ipi` | 68 | 21 |  |
| arch/riscv64/ipi.rs | `send_ipi_type` | 88 | 20 |  |
| arch/riscv64/ipi.rs | `send_reschedule_ipi` | 107 | 16 |  |
| arch/riscv64/ipi.rs | `flush_tlb_others` | 122 | 19 |  |
| arch/riscv64/ipi.rs | `handle_software_ipi` | 140 | 54 |  |
| arch/riscv64/ipi.rs | `new` | 193 | 21 | yes |
| arch/riscv64/ipi.rs | `csd_init` | 213 | 15 |  |
| arch/riscv64/ipi.rs | `smp_call_function` | 227 | 73 |  |
| arch/riscv64/ipi.rs | `csd_flush_queue` | 299 | 48 |  |
| arch/riscv64/ipi.rs | `ipi_reschedule_handler` | 346 | 13 |  |
| arch/riscv64/ipi.rs | `ipi_call_function_handler` | 358 | 5 |  |
| arch/riscv64/ipi.rs | `ipi_stop_handler` | 362 | 11 |  |
| arch/riscv64/ipi.rs | `ipi_irq_work_handler` | 372 | 8 |  |
| arch/riscv64/ipi.rs | `ipi_tlb_flush_handler` | 379 | 18 |  |
| arch/riscv64/ipi.rs | `ipi_irq_handler` | 396 | 24 |  |
| arch/riscv64/ipi.rs | `register_irq_handlers` | 419 | 14 |  |
| arch/riscv64/ipi.rs | `handle_ipi` | 432 | 28 |  |
| arch/riscv64/ipi.rs | `init` | 459 | 45 | yes |
| arch/riscv64/ipi.rs | `per_hart_enable_ssoftware` | 503 | 11 |  |
| arch/riscv64/mm/asid.rs | `alloc_asid` | 45 | 37 |  |
| arch/riscv64/mm/asid.rs | `free_asid` | 81 | 12 |  |
| arch/riscv64/mm/asid.rs | `asid_usage_count` | 92 | 16 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_all` | 107 | 17 |  |
| arch/riscv64/mm/asid.rs | `sfence_vma_addr` | 123 | 20 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_asid` | 142 | 17 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_page` | 158 | 17 | yes |
| arch/riscv64/mm/asid.rs | `flush_tlb_range` | 174 | 12 |  |
| arch/riscv64/mm/asid.rs | `flush_tlb_kernel` | 185 | 16 |  |
| arch/riscv64/mm/asid.rs | `build_satp` | 200 | 12 | yes |
| arch/riscv64/mm/asid.rs | `satp_to_asid` | 211 | 7 |  |
| arch/riscv64/mm/asid.rs | `satp_to_ppn` | 217 | 7 | yes |
| arch/riscv64/mm/asid.rs | `read_satp` | 223 | 19 |  |
| arch/riscv64/mm/asid.rs | `write_satp` | 241 | 30 |  |
| arch/riscv64/mm/asid.rs | `new` | 270 | 9 | yes |
| arch/riscv64/mm/asid.rs | `asid` | 278 | 6 |  |
| arch/riscv64/mm/asid.rs | `alloc` | 283 | 12 |  |
| arch/riscv64/mm/asid.rs | `free` | 294 | 10 |  |
| arch/riscv64/mm/asid.rs | `generation` | 303 | 9 |  |
| arch/riscv64/mm/asid.rs | `print_asid_status` | 311 | 14 |  |
| arch/riscv64/mm/exception.rs | `fixup_exception` | 79 | 23 | yes |
| arch/riscv64/mm/exception.rs | `exception_table_empty` | 101 | 11 | yes |
| arch/riscv64/mm/exception.rs | `exception_table_count` | 111 | 18 | yes |
| arch/riscv64/mm/exception.rs | `send_signal` | 128 | 21 | yes |
| arch/riscv64/mm/exception.rs | `in_interrupt` | 148 | 8 | yes |
| arch/riscv64/mm/exception.rs | `bad_area` | 155 | 22 | yes |
| arch/riscv64/mm/exception.rs | `no_context` | 176 | 20 | yes |
| arch/riscv64/mm/exception.rs | `do_page_fault` | 195 | 536 | yes |
| arch/riscv64/mm/fixmap.rs | `fix_to_virt` | 74 | 9 |  |
| arch/riscv64/mm/fixmap.rs | `virt_to_fix` | 82 | 11 |  |
| arch/riscv64/mm/fixmap.rs | `is_fixmap_addr` | 92 | 19 |  |
| arch/riscv64/mm/fixmap.rs | `set_fixmap` | 110 | 29 |  |
| arch/riscv64/mm/fixmap.rs | `clear_fixmap` | 138 | 55 |  |
| arch/riscv64/mm/fixmap.rs | `init_uart_fixmap` | 192 | 26 |  |
| arch/riscv64/mm/fixmap.rs | `uart_virt_addr` | 217 | 14 |  |
| arch/riscv64/mm/fixmap.rs | `is_uart_fixmap_initialized` | 230 | 17 |  |
| arch/riscv64/mm/fixmap.rs | `copy_fixmap_to_user` | 246 | 24 |  |
| arch/riscv64/mm/fixmap.rs | `print_fixmap_status` | 269 | 6 |  |
| arch/riscv64/mm/memory_layout.rs | `is_linear_mapping` | 106 | 131 |  |
| arch/riscv64/mm/memory_layout.rs | `mmio_alias` | 236 | 194 |  |
| arch/riscv64/mm/memory_layout.rs | `new` | 429 | 14 | yes |
| arch/riscv64/mm/memory_layout.rs | `bits` | 442 | 7 | yes |
| arch/riscv64/mm/memory_layout.rs | `is_aligned` | 448 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `floor` | 454 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `ceil` | 460 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `page_offset` | 466 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `vpn` | 472 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `as_u64` | 478 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `as_usize` | 484 | 14 | yes |
| arch/riscv64/mm/memory_layout.rs | `new` | 497 | 7 | yes |
| arch/riscv64/mm/memory_layout.rs | `bits` | 503 | 7 | yes |
| arch/riscv64/mm/memory_layout.rs | `is_aligned` | 509 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `floor` | 515 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `ceil` | 521 | 7 |  |
| arch/riscv64/mm/memory_layout.rs | `ppn` | 527 | 10 |  |
| arch/riscv64/mm/memory_layout.rs | `phys_to_virt` | 536 | 12 |  |
| arch/riscv64/mm/memory_layout.rs | `virt_to_phys` | 547 | 32 |  |
| arch/riscv64/mm/mm_ops.rs | `enable` | 54 | 12 |  |
| arch/riscv64/mm/mm_ops.rs | `disable` | 65 | 11 |  |
| arch/riscv64/mm/mm_ops.rs | `flush_tlb` | 75 | 7 |  |
| arch/riscv64/mm/mm_ops.rs | `flush_tlb_addr_page` | 81 | 10 |  |
| arch/riscv64/mm/mm_ops.rs | `map_single_page` | 90 | 50 |  |
| arch/riscv64/mm/mm_ops.rs | `unmap_vma` | 139 | 11 |  |
| arch/riscv64/mm/mm_ops.rs | `unmap_pages` | 149 | 129 |  |
| arch/riscv64/mm/mm_ops.rs | `unmap_one_user_leaf_locked` | 277 | 44 |  |
| arch/riscv64/mm/mm_ops.rs | `clear_pte` | 320 | 47 |  |
| arch/riscv64/mm/mm_ops.rs | `perm_to_flags` | 366 | 37 |  |
| arch/riscv64/mm/mm_ops.rs | `walk` | 402 | 114 | yes |
| arch/riscv64/mm/mm_ops.rs | `alloc_user_phys_page` | 515 | 11 |  |
| arch/riscv64/mm/mm_ops.rs | `create_user_address_space` | 525 | 57 |  |
| arch/riscv64/mm/mm_ops.rs | `copy_kernel_mappings` | 581 | 96 |  |
| arch/riscv64/mm/mm_ops.rs | `map_user_page` | 676 | 33 |  |
| arch/riscv64/mm/mm_ops.rs | `pte_lock_for` | 708 | 28 |  |
| arch/riscv64/mm/mm_ops.rs | `pte_flag_bits` | 735 | 25 |  |
| arch/riscv64/mm/mm_ops.rs | `leaf_pte_replace` | 759 | 48 |  |
| arch/riscv64/mm/mm_ops.rs | `leaf_pte_install` | 806 | 34 |  |
| arch/riscv64/mm/mm_ops.rs | `map_user_region` | 839 | 45 |  |
| arch/riscv64/mm/mm_ops.rs | `alloc_and_map_user_memory` | 883 | 113 |  |
| arch/riscv64/mm/mm_ops.rs | `alloc_and_map_to_kernel_table` | 995 | 57 |  |
| arch/riscv64/mm/mm_ops.rs | `alloc_and_map_to_user_table` | 1051 | 135 |  |
| arch/riscv64/mm/mm_ops.rs | `pte_is_shared_cache` | 1185 | 14 |  |
| arch/riscv64/mm/mm_ops.rs | `release_cache_mapping_pin` | 1198 | 21 |  |
| arch/riscv64/mm/mm_ops.rs | `copy_page_table_cow` | 1218 | 293 |  |
| arch/riscv64/mm/mm_ops.rs | `handle_cow_fault` | 1510 | 25 | yes |
| arch/riscv64/mm/mm_ops.rs | `drop` | 1534 | 226 |  |
| arch/riscv64/mm/mm_ops.rs | `is_cow_page` | 1759 | 49 |  |
| arch/riscv64/mm/mm_ops.rs | `check_pte_permissions` | 1807 | 59 |  |
| arch/riscv64/mm/mmu_init.rs | `root_page_table_ppn` | 48 | 50 | yes |
| arch/riscv64/mm/mmu_init.rs | `from_u8` | 97 | 15 |  |
| arch/riscv64/mm/mmu_init.rs | `get_alloc_stage` | 111 | 7 |  |
| arch/riscv64/mm/mmu_init.rs | `alloc_stage_is_late` | 117 | 6 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_ops_set_fixmap` | 122 | 6 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_ops_set_late` | 127 | 7 |  |
| arch/riscv64/mm/mmu_init.rs | `is_frame_allocator_ready` | 133 | 11 |  |
| arch/riscv64/mm/mmu_init.rs | `alloc_page_table` | 143 | 149 |  |
| arch/riscv64/mm/mmu_init.rs | `free_page_table_checked` | 291 | 25 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp_kernel_tree_boot` | 315 | 53 |  |
| arch/riscv64/mm/mmu_init.rs | `free_page_table` | 367 | 72 |  |
| arch/riscv64/mm/mmu_init.rs | `new` | 438 | 11 | yes |
| arch/riscv64/mm/mmu_init.rs | `bit_index` | 448 | 11 |  |
| arch/riscv64/mm/mmu_init.rs | `live_bit` | 458 | 11 |  |
| arch/riscv64/mm/mmu_init.rs | `set_live_bit` | 468 | 17 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp_boot` | 484 | 17 |  |
| arch/riscv64/mm/mmu_init.rs | `take_returns` | 500 | 13 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp` | 512 | 35 |  |
| arch/riscv64/mm/mmu_init.rs | `take` | 546 | 112 |  |
| arch/riscv64/mm/mmu_init.rs | `new` | 657 | 28 | yes |
| arch/riscv64/mm/mmu_init.rs | `new` | 684 | 13 | yes |
| arch/riscv64/mm/mmu_init.rs | `register_fork_root` | 696 | 8 |  |
| arch/riscv64/mm/mmu_init.rs | `task_pid_alive` | 703 | 27 |  |
| arch/riscv64/mm/mmu_init.rs | `stamp_root` | 729 | 7 |  |
| arch/riscv64/mm/mmu_init.rs | `is_boot` | 735 | 8 |  |
| arch/riscv64/mm/mmu_init.rs | `peek` | 742 | 7 |  |
| arch/riscv64/mm/mmu_init.rs | `freed_by_peek` | 748 | 10 |  |
| arch/riscv64/mm/mmu_init.rs | `poison_table` | 757 | 13 |  |
| arch/riscv64/mm/mmu_init.rs | `is_poisoned` | 769 | 13 |  |
| arch/riscv64/mm/mmu_init.rs | `is_live_table` | 781 | 8 |  |
| arch/riscv64/mm/mmu_init.rs | `live_count` | 788 | 15 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_free_ring_prev_free_pid` | 802 | 18 |  |
| arch/riscv64/mm/mmu_init.rs | `new` | 819 | 30 | yes |
| arch/riscv64/mm/mmu_init.rs | `is_framebuffer_frame` | 848 | 31 |  |
| arch/riscv64/mm/mmu_init.rs | `new` | 878 | 13 | yes |
| arch/riscv64/mm/mmu_init.rs | `pte_install_log` | 890 | 22 |  |
| arch/riscv64/mm/mmu_init.rs | `owner_maps_pfn` | 911 | 51 |  |
| arch/riscv64/mm/mmu_init.rs | `textfree_owner_maps` | 961 | 30 |  |
| arch/riscv64/mm/mmu_init.rs | `release_user_leaf` | 990 | 175 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_pending_drain` | 1164 | 3 |  |
| arch/riscv64/mm/mmu_init.rs | `free_user_page_tables` | 1166 | 345 |  |
| arch/riscv64/mm/mmu_init.rs | `pt_alloc_fail` | 1510 | 18 |  |
| arch/riscv64/mm/mmu_init.rs | `map_page_noflush` | 1527 | 164 |  |
| arch/riscv64/mm/mmu_init.rs | `map_page` | 1690 | 12 |  |
| arch/riscv64/mm/mmu_init.rs | `map_region` | 1701 | 21 |  |
| arch/riscv64/mm/mmu_init.rs | `map_pmd_huge_page` | 1721 | 56 |  |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_page_noflush` | 1776 | 76 |  |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_page` | 1851 | 23 |  |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_region` | 1873 | 36 |  |
| arch/riscv64/mm/mmu_init.rs | `map_kernel_region_huge` | 1908 | 25 |  |
| arch/riscv64/mm/mmu_init.rs | `map_device_page` | 1932 | 17 |  |
| arch/riscv64/mm/mmu_init.rs | `best_map_size` | 1948 | 106 |  |
| arch/riscv64/mm/mmu_init.rs | `init` | 2053 | 88 |  |
| arch/riscv64/mm/mmu_init.rs | `kernel_device_window_pte` | 2140 | 26 |  |
| arch/riscv64/mm/mmu_init.rs | `setup_device_mappings` | 2165 | 68 |  |
| arch/riscv64/mm/mmu_init.rs | `setup_linear_mapping` | 2232 | 45 |  |
| arch/riscv64/mm/mmu_init.rs | `enable` | 2276 | 12 |  |
| arch/riscv64/mm/mmu_init.rs | `map_identity` | 2287 | 11 |  |
| arch/riscv64/mm/mmu_init.rs | `get_kernel_page_table_ppn` | 2297 | 15 |  |
| arch/riscv64/mm/mmu_init.rs | `forensics_scan_trees` | 2311 | 134 |  |
| arch/riscv64/mm/mod.rs | `register_in_tree_mm_policy` | 63 | 6 |  |
| arch/riscv64/mm/page_fault.rs | `filemap_pin_cache_page` | 51 | 61 |  |
| arch/riscv64/mm/page_fault.rs | `get_user_phys` | 111 | 53 |  |
| arch/riscv64/mm/page_fault.rs | `write_pte_raw` | 163 | 20 |  |
| arch/riscv64/mm/page_fault.rs | `read_pte_raw` | 182 | 34 |  |
| arch/riscv64/mm/pagetable.rs | `new` | 68 | 7 | yes |
| arch/riscv64/mm/pagetable.rs | `from_bits` | 74 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `bits` | 80 | 7 | yes |
| arch/riscv64/mm/pagetable.rs | `is_valid` | 86 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `is_readable` | 92 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `is_writable` | 98 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `is_executable` | 104 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `is_user` | 110 | 8 |  |
| arch/riscv64/mm/pagetable.rs | `is_leaf` | 117 | 8 |  |
| arch/riscv64/mm/pagetable.rs | `ppn` | 124 | 10 |  |
| arch/riscv64/mm/pagetable.rs | `ppn_for_2mb_page` | 133 | 13 |  |
| arch/riscv64/mm/pagetable.rs | `new_table` | 145 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `new_page_kernel` | 151 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `new_page_user` | 157 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `new_page_ro` | 163 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `default` | 169 | 17 |  |
| arch/riscv64/mm/pagetable.rs | `new` | 185 | 9 | yes |
| arch/riscv64/mm/pagetable.rs | `get` | 193 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `set` | 199 | 6 |  |
| arch/riscv64/mm/pagetable.rs | `zero` | 204 | 9 |  |
| arch/riscv64/mm/pagetable.rs | `default` | 212 | 27 |  |
| arch/riscv64/mm/pagetable.rs | `new` | 238 | 7 | yes |
| arch/riscv64/mm/pagetable.rs | `sv39` | 244 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `bits` | 250 | 7 | yes |
| arch/riscv64/mm/pagetable.rs | `mode` | 256 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `asid` | 262 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `ppn` | 268 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `is_bare` | 274 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `is_sv39` | 280 | 7 |  |
| arch/riscv64/mm/pagetable.rs | `get_satp` | 286 | 10 | yes |
| arch/riscv64/mm/unsafe_edges.rs | `get_page_table_virt` | 45 | 41 |  |
| arch/riscv64/mm/unsafe_edges.rs | `from_phys` | 85 | 23 |  |
| arch/riscv64/mm/unsafe_edges.rs | `from_phys_live` | 107 | 6 |  |
| arch/riscv64/mm/unsafe_edges.rs | `as_phys_base` | 112 | 7 |  |
| arch/riscv64/mm/unsafe_edges.rs | `get_raw` | 118 | 9 |  |
| arch/riscv64/mm/unsafe_edges.rs | `get` | 126 | 7 |  |
| arch/riscv64/mm/unsafe_edges.rs | `set` | 132 | 12 |  |
| arch/riscv64/mm/unsafe_edges.rs | `all_zero` | 143 | 12 |  |
| arch/riscv64/mm/unsafe_edges.rs | `zero_entries` | 154 | 13 |  |
| arch/riscv64/mm/unsafe_edges.rs | `static_root` | 166 | 20 |  |
| arch/riscv64/mm/unsafe_edges.rs | `copy_page` | 185 | 9 |  |
| arch/riscv64/mod.rs | `arch_init` | 30 | 5 |  |
| arch/riscv64/mod.rs | `init` | 34 | 41 | yes |
| arch/riscv64/mod.rs | `print_cpu_info` | 74 | 8 |  |
| arch/riscv64/mod.rs | `enable_interrupts` | 81 | 28 |  |
| arch/riscv64/mod.rs | `cpu_id` | 108 | 29 | yes |
| arch/riscv64/process.rs | `start_thread` | 40 | 49 |  |
| arch/riscv64/process.rs | `flush_thread` | 88 | 43 |  |
| arch/riscv64/process.rs | `current_pt_regs` | 130 | 17 | yes |
| arch/riscv64/process.rs | `task_pt_regs` | 146 | 14 |  |
| arch/riscv64/process.rs | `user_stack_pointer` | 159 | 9 |  |
| arch/riscv64/process.rs | `set_user_stack_pointer` | 167 | 9 |  |
| arch/riscv64/process.rs | `instruction_pointer` | 175 | 9 |  |
| arch/riscv64/process.rs | `set_instruction_pointer` | 183 | 15 |  |
| arch/riscv64/process.rs | `is_user_address` | 197 | 10 |  |
| arch/riscv64/pt_regs.rs | `new` | 116 | 19 | yes |
| arch/riscv64/pt_regs.rs | `user_mode` | 134 | 7 | yes |
| arch/riscv64/pt_regs.rs | `kernel_mode` | 140 | 7 | yes |
| arch/riscv64/pt_regs.rs | `syscall_nr` | 146 | 9 |  |
| arch/riscv64/pt_regs.rs | `syscall_args` | 154 | 14 |  |
| arch/riscv64/pt_regs.rs | `set_return_value` | 167 | 9 |  |
| arch/riscv64/pt_regs.rs | `set_return_error` | 175 | 9 |  |
| arch/riscv64/pt_regs.rs | `syscall_rollback` | 183 | 7 |  |
| arch/riscv64/pt_regs.rs | `instruction_pointer` | 189 | 7 |  |
| arch/riscv64/pt_regs.rs | `set_instruction_pointer` | 195 | 7 |  |
| arch/riscv64/pt_regs.rs | `user_stack_pointer` | 201 | 7 |  |
| arch/riscv64/pt_regs.rs | `set_user_stack_pointer` | 207 | 7 |  |
| arch/riscv64/pt_regs.rs | `frame_pointer` | 213 | 7 |  |
| arch/riscv64/pt_regs.rs | `irqs_disabled` | 219 | 8 |  |
| arch/riscv64/pt_regs.rs | `mark_user_frame` | 226 | 8 |  |
| arch/riscv64/pt_regs.rs | `mark_kernel_frame` | 233 | 90 |  |
| arch/riscv64/pt_regs.rs | `from_cause` | 322 | 35 | yes |
| arch/riscv64/pt_regs.rs | `is_interrupt` | 356 | 9 |  |
| arch/riscv64/pt_regs.rs | `is_exception` | 364 | 6 |  |
| arch/riscv64/pt_regs.rs | `is_page_fault` | 369 | 13 |  |
| arch/riscv64/pt_regs.rs | `in_interrupt` | 381 | 7 |  |
| arch/riscv64/pt_regs.rs | `in_task` | 387 | 54 |  |
| arch/riscv64/smp.rs | `boot_hart_id` | 31 | 5 | yes |
| arch/riscv64/smp.rs | `mark_cpu_started` | 35 | 13 | yes |
| arch/riscv64/smp.rs | `cpu_started` | 47 | 19 | yes |
| arch/riscv64/smp.rs | `cpu_id` | 65 | 37 | yes |
| arch/riscv64/smp.rs | `is_boot_hart` | 101 | 14 | yes |
| arch/riscv64/smp.rs | `init` | 114 | 10 | yes |
| arch/riscv64/smp.rs | `secondary_start` | 123 | 24 | yes |
| arch/riscv64/smp.rs | `signal_boot_complete` | 146 | 6 | yes |
| arch/riscv64/smp.rs | `is_boot_complete` | 151 | 7 | yes |
| arch/riscv64/smp.rs | `start_secondaries` | 157 | 170 | yes |
| arch/riscv64/smp.rs | `num_started_cpus` | 326 | 22 | yes |
| arch/riscv64/smp.rs | `as_ptr` | 347 | 15 | yes |
| arch/riscv64/smp.rs | `init_per_cpu_intr_stacks` | 361 | 35 | yes |
| arch/riscv64/thread.rs | `new` | 100 | 35 | yes |
| arch/riscv64/thread.rs | `save_fpu` | 134 | 28 | yes |
| arch/riscv64/thread.rs | `restore_fpu` | 161 | 25 | yes |
| arch/riscv64/thread.rs | `fpu_save_for_switch` | 185 | 29 | yes |
| arch/riscv64/thread.rs | `get_fs` | 213 | 21 | yes |
| arch/riscv64/thread.rs | `vstate_save_for_switch` | 233 | 28 | yes |
| arch/riscv64/thread.rs | `restore_vstate` | 260 | 20 | yes |
| arch/riscv64/thread.rs | `save_vstate` | 279 | 22 | yes |
| arch/riscv64/thread.rs | `vstate_first_use` | 300 | 13 | yes |
| arch/riscv64/thread.rs | `mark_vstate_clean` | 312 | 15 | yes |
| arch/riscv64/thread.rs | `__fstate_save` | 326 | 63 | yes |
| arch/riscv64/thread.rs | `__fstate_restore` | 388 | 56 | yes |
| arch/riscv64/thread.rs | `tp` | 443 | 7 | yes |
| arch/riscv64/thread.rs | `set_tp` | 449 | 8 | yes |
| arch/riscv64/thread.rs | `mark_fpu_clean` | 456 | 7 | yes |
| arch/riscv64/thread.rs | `default` | 462 | 14 | yes |
| arch/riscv64/thread.rs | `fpu_init` | 475 | 76 | yes |
| arch/riscv64/trap.rs | `current_task_pt_regs` | 45 | 32 | yes |
| arch/riscv64/trap.rs | `current_pt_regs` | 76 | 10 | yes |
| arch/riscv64/trap.rs | `pt_regs_rip_for_cpu` | 85 | 25 | yes |
| arch/riscv64/trap.rs | `init` | 109 | 7 | yes |
| arch/riscv64/trap.rs | `trap_entry` | 115 | 23 | yes |
| arch/riscv64/trap.rs | `init_syscall` | 137 | 5 | yes |
| arch/riscv64/trap.rs | `enable_timer_interrupt` | 141 | 26 | yes |
| arch/riscv64/trap.rs | `disable_timer_interrupt` | 166 | 8 | yes |
| arch/riscv64/trap.rs | `enable_external_interrupt` | 173 | 31 | yes |
| arch/riscv64/trap.rs | `putfrag` | 203 | 9 | yes |
| arch/riscv64/trap.rs | `puthex` | 211 | 10 | yes |
| arch/riscv64/trap.rs | `putb` | 220 | 382 | yes |
| arch/riscv64/trap.rs | `irq_exit_maybe_preempt_kernel` | 601 | 71 | yes |
| arch/riscv64/trap.rs | `handle_timer_interrupt` | 671 | 69 | yes |
| arch/riscv64/trap.rs | `handle_software_interrupt` | 739 | 21 | yes |
| arch/riscv64/trap.rs | `handle_external_interrupt` | 759 | 18 | yes |
| arch/riscv64/trap.rs | `handle_syscall` | 776 | 90 | yes |
| arch/riscv64/trap.rs | `handle_illegal_instruction` | 865 | 208 | yes |
| arch/riscv64/trap.rs | `sigill_mapping_forensics` | 1072 | 182 | yes |
| arch/riscv64/trap.rs | `handle_breakpoint` | 1253 | 106 | yes |
| arch/riscv64/trap.rs | `sigsegv_has_handler` | 1358 | 15 | yes |
| arch/riscv64/trap.rs | `sigsegv_blocked_current` | 1372 | 7 | yes |
| arch/riscv64/trap.rs | `sigbus_has_handler` | 1378 | 54 | yes |
| arch/riscv64/trap.rs | `handle_page_fault` | 1431 | 539 | yes |
| arch/riscv64/trap.rs | `oops_show_regs` | 1969 | 61 | yes |
| arch/riscv64/trap.rs | `handle_unknown_exception` | 2029 | 38 | yes |
| arch/riscv64/trap.rs | `current_trap_frame` | 2066 | 66 | yes |
| arch/riscv64/trap.rs | `phex` | 2131 | 9 | yes |
| arch/riscv64/trap.rs | `pdec` | 2139 | 77 | yes |
| arch/riscv64/uaccess.rs | `access_ok` | 59 | 40 | yes |
| arch/riscv64/uaccess.rs | `__copy_to_user` | 98 | 2 | yes |
| arch/riscv64/uaccess.rs | `__copy_from_user` | 99 | 2 | yes |
| arch/riscv64/uaccess.rs | `__futex_cmpxchg_u32` | 100 | 2 | yes |
| arch/riscv64/uaccess.rs | `__clear_user` | 101 | 18 | yes |
| arch/riscv64/uaccess.rs | `copy_to_user` | 118 | 36 | yes |
| arch/riscv64/uaccess.rs | `fault_in_write` | 153 | 105 | yes |
| arch/riscv64/uaccess.rs | `copy_from_user` | 257 | 42 | yes |
| arch/riscv64/uaccess.rs | `clear_user` | 298 | 39 | yes |
| arch/riscv64/uaccess.rs | `get_user` | 336 | 32 | yes |
| arch/riscv64/uaccess.rs | `put_user` | 367 | 30 | yes |
| arch/riscv64/uaccess.rs | `strncpy_from_user` | 396 | 81 | yes |
| arch/riscv64/uaccess.rs | `strnlen_user` | 476 | 29 | yes |
| arch/riscv64/uaccess.rs | `futex_cmpxchg_u32_user` | 504 | 14 | yes |
| arch/riscv64/unsafe_edges.rs | `from_trap` | 53 | 11 | yes |
| arch/riscv64/unsafe_edges.rs | `regs` | 63 | 9 |  |
| arch/riscv64/unsafe_edges.rs | `deref` | 71 | 8 |  |
| arch/riscv64/unsafe_edges.rs | `deref_mut` | 78 | 5 |  |
| arch/riscv64/vext.rs | `vmv_v_i0` | 61 | 5 |  |
| arch/riscv64/vext.rs | `vs1r_v` | 65 | 5 |  |
| arch/riscv64/vext.rs | `vl1r_v` | 69 | 33 |  |
| arch/riscv64/vext.rs | `hart_has_v` | 101 | 9 | yes |
| arch/riscv64/vext.rs | `prime_hart_has_v` | 109 | 13 |  |
| arch/riscv64/vext.rs | `vlen_fits` | 121 | 13 |  |
| arch/riscv64/vext.rs | `vs_disable` | 133 | 26 |  |
| arch/riscv64/vext.rs | `vstate_first_use_init` | 158 | 72 | yes |
| arch/riscv64/vext.rs | `vstate_save` | 229 | 94 | yes |
| arch/riscv64/vext.rs | `vstate_restore` | 322 | 86 | yes |
| arch/riscv64/vext.rs | `is_vector_instr32` | 407 | 8 | yes |
