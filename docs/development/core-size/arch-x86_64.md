# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch/x86_64 — hot 8486 lines (stay) / cold 8186 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/x86_64/boot.rs | `rust_main` | 21 | 61 |  |
| arch/x86_64/boot.rs | `get_boot_magic` | 81 | 8 |  |
| arch/x86_64/boot.rs | `get_dtb_pointer` | 88 | 9 | yes |
| arch/x86_64/boot.rs | `early_boot_init` | 96 | 128 |  |
| arch/x86_64/boot.rs | `boot_memory_regions` | 223 | 11 |  |
| arch/x86_64/boot.rs | `boot_cmdline` | 233 | 23 |  |
| arch/x86_64/context.rs | `__switch_to` | 46 | 26 | yes |
| arch/x86_64/context.rs | `switch_mm` | 71 | 335 | yes |
| arch/x86_64/context.rs | `set_prev_task` | 405 | 34 | yes |
| arch/x86_64/context.rs | `context_switch` | 438 | 8 | yes |
| arch/x86_64/context.rs | `context_switch_body` | 445 | 229 | yes |
| arch/x86_64/context.rs | `cpu_of` | 673 | 16 | yes |
| arch/x86_64/context.rs | `putfrag_no_lock` | 688 | 10 | yes |
| arch/x86_64/context.rs | `puthex_no_lock` | 697 | 13 | yes |
| arch/x86_64/context.rs | `puthex32_no_lock` | 709 | 91 | yes |
| arch/x86_64/cpu.rs | `inb` | 22 | 11 | yes |
| arch/x86_64/cpu.rs | `outb` | 32 | 9 | yes |
| arch/x86_64/cpu.rs | `inw` | 40 | 11 | yes |
| arch/x86_64/cpu.rs | `outw` | 50 | 9 | yes |
| arch/x86_64/cpu.rs | `inl` | 58 | 11 | yes |
| arch/x86_64/cpu.rs | `outl` | 68 | 9 | yes |
| arch/x86_64/cpu.rs | `io_wait` | 76 | 12 | yes |
| arch/x86_64/cpu.rs | `read_cr2` | 87 | 10 | yes |
| arch/x86_64/cpu.rs | `read_cr0` | 96 | 15 | yes |
| arch/x86_64/cpu.rs | `write_cr0` | 110 | 13 | yes |
| arch/x86_64/cpu.rs | `ts_set` | 122 | 8 | yes |
| arch/x86_64/cpu.rs | `clts` | 129 | 9 | yes |
| arch/x86_64/cpu.rs | `stts` | 137 | 19 | yes |
| arch/x86_64/cpu.rs | `enable_write_protect` | 155 | 9 | yes |
| arch/x86_64/cpu.rs | `read_cr3` | 163 | 13 | yes |
| arch/x86_64/cpu.rs | `write_cr3` | 175 | 10 | yes |
| arch/x86_64/cpu.rs | `rdmsr` | 184 | 14 | yes |
| arch/x86_64/cpu.rs | `wrmsr` | 197 | 17 | yes |
| arch/x86_64/cpu.rs | `invlpg` | 213 | 7 | yes |
| arch/x86_64/cpu.rs | `read_cr4` | 219 | 14 | yes |
| arch/x86_64/cpu.rs | `write_cr4` | 232 | 19 | yes |
| arch/x86_64/cpu.rs | `invpcid` | 250 | 21 | yes |
| arch/x86_64/cpu.rs | `get_core_id` | 270 | 7 | yes |
| arch/x86_64/cpu.rs | `get_thread_id_of` | 276 | 10 | yes |
| arch/x86_64/cpu.rs | `get_thread_id` | 285 | 7 | yes |
| arch/x86_64/cpu.rs | `set_thread_id` | 291 | 9 | yes |
| arch/x86_64/cpu.rs | `get_counter_freq` | 299 | 7 | yes |
| arch/x86_64/cpu.rs | `read_counter` | 305 | 26 | yes |
| arch/x86_64/cpu.rs | `smap_enabled` | 330 | 17 | yes |
| arch/x86_64/cpu.rs | `fsgsbase_enabled` | 346 | 8 | yes |
| arch/x86_64/cpu.rs | `enable_fsgsbase` | 353 | 37 | yes |
| arch/x86_64/cpu.rs | `enable_smep` | 389 | 24 | yes |
| arch/x86_64/cpu.rs | `wrfsbase` | 412 | 12 | yes |
| arch/x86_64/cpu.rs | `wrgsbase` | 423 | 23 | yes |
| arch/x86_64/cpu.rs | `smap_report` | 445 | 32 | yes |
| arch/x86_64/cpu.rs | `cpuid` | 476 | 27 | yes |
| arch/x86_64/cpu.rs | `enable_irq` | 502 | 12 | yes |
| arch/x86_64/cpu.rs | `disable_irq` | 513 | 12 | yes |
| arch/x86_64/cpu.rs | `wfi` | 524 | 20 | yes |
| arch/x86_64/cpu.rs | `read_time` | 543 | 7 | yes |
| arch/x86_64/cpu.rs | `isb` | 549 | 18 | yes |
| arch/x86_64/cpu.rs | `dsb` | 566 | 8 | yes |
| arch/x86_64/cpu.rs | `dmb` | 573 | 8 | yes |
| arch/x86_64/cpu.rs | `get_interrupts_state` | 580 | 18 | yes |
| arch/x86_64/cpu.rs | `save_and_disable_irq` | 597 | 12 | yes |
| arch/x86_64/cpu.rs | `restore_irq` | 608 | 6 | yes |
| arch/x86_64/ipi.rs | `vector` | 53 | 18 |  |
| arch/x86_64/ipi.rs | `send_ipi_type` | 70 | 25 |  |
| arch/x86_64/ipi.rs | `send_reschedule_ipi` | 94 | 12 |  |
| arch/x86_64/ipi.rs | `flush_tlb_others` | 105 | 20 |  |
| arch/x86_64/ipi.rs | `flush_tlb_others_mm` | 124 | 19 |  |
| arch/x86_64/ipi.rs | `started_cpu_mask` | 142 | 32 |  |
| arch/x86_64/ipi.rs | `tlb_epoch_snapshot` | 173 | 13 |  |
| arch/x86_64/ipi.rs | `tlb_remote_targets` | 185 | 8 |  |
| arch/x86_64/ipi.rs | `tlb_epochs_advanced` | 192 | 24 |  |
| arch/x86_64/ipi.rs | `handle_ipi_vector` | 215 | 105 |  |
| arch/x86_64/ipi.rs | `snapshot_self` | 319 | 75 |  |
| arch/x86_64/ipi.rs | `remote_bt_self` | 393 | 55 |  |
| arch/x86_64/ipi.rs | `remote_backtrace` | 447 | 104 |  |
| arch/x86_64/ipi.rs | `new` | 550 | 21 | yes |
| arch/x86_64/ipi.rs | `csd_init` | 570 | 15 |  |
| arch/x86_64/ipi.rs | `smp_call_function` | 584 | 52 |  |
| arch/x86_64/ipi.rs | `csd_fire` | 635 | 43 |  |
| arch/x86_64/ipi.rs | `smp_call_function_timeout` | 677 | 46 |  |
| arch/x86_64/ipi.rs | `csd_flush_queue` | 722 | 53 |  |
| arch/x86_64/ipi.rs | `init` | 774 | 4 | yes |
| arch/x86_64/mm/asid.rs | `pcid_enabled` | 46 | 28 | yes |
| arch/x86_64/mm/asid.rs | `enable_pcid` | 73 | 56 |  |
| arch/x86_64/mm/asid.rs | `alloc_asid` | 128 | 39 |  |
| arch/x86_64/mm/asid.rs | `free_asid` | 166 | 15 |  |
| arch/x86_64/mm/asid.rs | `ipi_flush_all` | 180 | 9 |  |
| arch/x86_64/mm/asid.rs | `flush_all_cpus_local` | 188 | 12 |  |
| arch/x86_64/mm/asid.rs | `asid_usage_count` | 199 | 12 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_all` | 210 | 16 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_asid` | 225 | 14 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_page` | 238 | 8 | yes |
| arch/x86_64/mm/asid.rs | `flush_tlb_page_asid` | 245 | 11 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_range` | 255 | 10 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_kernel` | 264 | 20 |  |
| arch/x86_64/mm/asid.rs | `build_satp` | 283 | 8 | yes |
| arch/x86_64/mm/asid.rs | `satp_to_asid` | 290 | 10 |  |
| arch/x86_64/mm/asid.rs | `satp_to_ppn` | 299 | 19 | yes |
| arch/x86_64/mm/asid.rs | `switch_root` | 317 | 28 | yes |
| arch/x86_64/mm/asid.rs | `read_satp` | 344 | 7 |  |
| arch/x86_64/mm/asid.rs | `write_satp` | 350 | 8 |  |
| arch/x86_64/mm/asid.rs | `new` | 357 | 6 | yes |
| arch/x86_64/mm/asid.rs | `print_asid_status` | 362 | 8 |  |
| arch/x86_64/mm/exception.rs | `fixup_exception` | 36 | 18 | yes |
| arch/x86_64/mm/exception.rs | `exception_table_empty` | 53 | 6 | yes |
| arch/x86_64/mm/exception.rs | `exception_table_count` | 58 | 15 | yes |
| arch/x86_64/mm/exception.rs | `send_signal` | 72 | 28 | yes |
| arch/x86_64/mm/exception.rs | `in_interrupt` | 99 | 8 | yes |
| arch/x86_64/mm/exception.rs | `bad_area` | 106 | 22 | yes |
| arch/x86_64/mm/exception.rs | `no_context` | 127 | 41 | yes |
| arch/x86_64/mm/exception.rs | `do_page_fault` | 167 | 261 | yes |
| arch/x86_64/mm/exception.rs | `phex` | 427 | 8 | yes |
| arch/x86_64/mm/exception.rs | `ptag` | 434 | 262 | yes |
| arch/x86_64/mm/fixmap.rs | `fix_to_virt` | 33 | 6 |  |
| arch/x86_64/mm/fixmap.rs | `virt_to_fix` | 38 | 9 |  |
| arch/x86_64/mm/fixmap.rs | `is_fixmap_addr` | 46 | 10 |  |
| arch/x86_64/mm/fixmap.rs | `set_fixmap` | 55 | 7 |  |
| arch/x86_64/mm/fixmap.rs | `clear_fixmap` | 61 | 46 |  |
| arch/x86_64/mm/fixmap.rs | `init_uart_fixmap` | 106 | 5 |  |
| arch/x86_64/mm/fixmap.rs | `uart_virt_addr` | 110 | 5 |  |
| arch/x86_64/mm/fixmap.rs | `is_uart_fixmap_initialized` | 114 | 7 |  |
| arch/x86_64/mm/fixmap.rs | `copy_fixmap_to_user` | 120 | 3 |  |
| arch/x86_64/mm/fixmap.rs | `print_fixmap_status` | 122 | 2 |  |
| arch/x86_64/mm/memory_layout.rs | `device_alias` | 125 | 16 |  |
| arch/x86_64/mm/memory_layout.rs | `device_alias_fixed` | 140 | 44 |  |
| arch/x86_64/mm/memory_layout.rs | `is_linear_mapping` | 183 | 157 |  |
| arch/x86_64/mm/memory_layout.rs | `new` | 339 | 4 | yes |
| arch/x86_64/mm/memory_layout.rs | `bits` | 342 | 4 | yes |
| arch/x86_64/mm/memory_layout.rs | `is_aligned` | 345 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `floor` | 348 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `ceil` | 351 | 8 |  |
| arch/x86_64/mm/memory_layout.rs | `page_offset` | 358 | 5 |  |
| arch/x86_64/mm/memory_layout.rs | `vpn` | 362 | 5 |  |
| arch/x86_64/mm/memory_layout.rs | `pgd_index` | 366 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `pud_index` | 369 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `pmd_index` | 372 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `pte_index` | 375 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `as_u64` | 378 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `as_usize` | 381 | 11 | yes |
| arch/x86_64/mm/memory_layout.rs | `new` | 391 | 4 | yes |
| arch/x86_64/mm/memory_layout.rs | `bits` | 394 | 4 | yes |
| arch/x86_64/mm/memory_layout.rs | `is_aligned` | 397 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `floor` | 400 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `ceil` | 403 | 8 |  |
| arch/x86_64/mm/memory_layout.rs | `ppn` | 410 | 7 |  |
| arch/x86_64/mm/memory_layout.rs | `phys_to_virt` | 416 | 29 |  |
| arch/x86_64/mm/memory_layout.rs | `virt_to_phys` | 444 | 13 |  |
| arch/x86_64/mm/memory_layout.rs | `mmio_alias` | 456 | 4 |  |
| arch/x86_64/mm/mm_ops.rs | `enable` | 53 | 20 |  |
| arch/x86_64/mm/mm_ops.rs | `disable` | 72 | 9 |  |
| arch/x86_64/mm/mm_ops.rs | `flush_tlb` | 80 | 6 |  |
| arch/x86_64/mm/mm_ops.rs | `flush_tlb_addr_page` | 85 | 9 |  |
| arch/x86_64/mm/mm_ops.rs | `map_single_page` | 93 | 48 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_vma` | 140 | 22 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_pages` | 161 | 337 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_clear_leaf` | 497 | 9 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_pages_slow` | 505 | 92 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_one_user_leaf_locked` | 596 | 63 |  |
| arch/x86_64/mm/mm_ops.rs | `clear_pte` | 658 | 62 |  |
| arch/x86_64/mm/mm_ops.rs | `perm_to_flags` | 719 | 41 |  |
| arch/x86_64/mm/mm_ops.rs | `walk` | 759 | 92 |  |
| arch/x86_64/mm/mm_ops.rs | `alloc_user_phys_page` | 850 | 11 |  |
| arch/x86_64/mm/mm_ops.rs | `create_user_address_space` | 860 | 65 |  |
| arch/x86_64/mm/mm_ops.rs | `copy_kernel_mappings` | 924 | 38 |  |
| arch/x86_64/mm/mm_ops.rs | `map_user_page` | 961 | 28 |  |
| arch/x86_64/mm/mm_ops.rs | `pte_lock_for` | 988 | 29 |  |
| arch/x86_64/mm/mm_ops.rs | `pte_flag_bits` | 1016 | 27 |  |
| arch/x86_64/mm/mm_ops.rs | `leaf_pte_replace` | 1042 | 64 |  |
| arch/x86_64/mm/mm_ops.rs | `leaf_pte_install` | 1105 | 50 |  |
| arch/x86_64/mm/mm_ops.rs | `map_user_region` | 1154 | 41 |  |
| arch/x86_64/mm/mm_ops.rs | `alloc_and_map_user_memory` | 1194 | 102 |  |
| arch/x86_64/mm/mm_ops.rs | `alloc_and_map_to_kernel_table` | 1295 | 66 |  |
| arch/x86_64/mm/mm_ops.rs | `alloc_and_map_to_user_table` | 1360 | 111 |  |
| arch/x86_64/mm/mm_ops.rs | `release_cache_mapping_pin` | 1470 | 26 |  |
| arch/x86_64/mm/mm_ops.rs | `copy_page_table_cow` | 1495 | 404 |  |
| arch/x86_64/mm/mm_ops.rs | `handle_cow_fault` | 1898 | 20 | yes |
| arch/x86_64/mm/mm_ops.rs | `drop` | 1917 | 195 |  |
| arch/x86_64/mm/mm_ops.rs | `is_cow_page` | 2111 | 50 |  |
| arch/x86_64/mm/mm_ops.rs | `check_pte_permissions` | 2160 | 93 |  |
| arch/x86_64/mm/mm_ops.rs | `mprotect_pte_range` | 2252 | 114 |  |
| arch/x86_64/mm/mmu_init.rs | `kernel_image_phys_end` | 48 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `raw_image_phys_end` | 53 | 14 |  |
| arch/x86_64/mm/mmu_init.rs | `root_static_phys` | 66 | 11 |  |
| arch/x86_64/mm/mmu_init.rs | `root_static_ppn` | 76 | 13 |  |
| arch/x86_64/mm/mmu_init.rs | `root_page_table_ppn` | 88 | 48 | yes |
| arch/x86_64/mm/mmu_init.rs | `from_u8` | 135 | 15 |  |
| arch/x86_64/mm/mmu_init.rs | `get_alloc_stage` | 149 | 7 |  |
| arch/x86_64/mm/mmu_init.rs | `alloc_stage_is_late` | 155 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_ops_set_fixmap` | 160 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_ops_set_late` | 165 | 12 |  |
| arch/x86_64/mm/mmu_init.rs | `early_tables_used` | 176 | 11 |  |
| arch/x86_64/mm/mmu_init.rs | `zero_owned_pages` | 186 | 7 |  |
| arch/x86_64/mm/mmu_init.rs | `alloc_page_table` | 192 | 169 |  |
| arch/x86_64/mm/mmu_init.rs | `free_page_table_checked` | 360 | 7 |  |
| arch/x86_64/mm/mmu_init.rs | `free_page_table_checked_rooted` | 366 | 62 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp_kernel_tree_boot` | 427 | 39 |  |
| arch/x86_64/mm/mmu_init.rs | `free_page_table` | 465 | 92 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_free_emptied_table` | 556 | 47 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_pending_drain` | 602 | 20 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_pending_count` | 621 | 42 |  |
| arch/x86_64/mm/mmu_init.rs | `new` | 662 | 11 | yes |
| arch/x86_64/mm/mmu_init.rs | `bit_index` | 672 | 11 |  |
| arch/x86_64/mm/mmu_init.rs | `live_bit` | 682 | 11 |  |
| arch/x86_64/mm/mmu_init.rs | `set_live_bit` | 692 | 17 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp_boot` | 708 | 17 |  |
| arch/x86_64/mm/mmu_init.rs | `take_returns` | 724 | 13 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp` | 736 | 41 |  |
| arch/x86_64/mm/mmu_init.rs | `take` | 776 | 104 |  |
| arch/x86_64/mm/mmu_init.rs | `new` | 879 | 26 | yes |
| arch/x86_64/mm/mmu_init.rs | `new` | 904 | 12 | yes |
| arch/x86_64/mm/mmu_init.rs | `register_fork_root` | 915 | 8 |  |
| arch/x86_64/mm/mmu_init.rs | `task_pid_alive` | 922 | 27 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp_root` | 948 | 7 |  |
| arch/x86_64/mm/mmu_init.rs | `is_boot` | 954 | 8 |  |
| arch/x86_64/mm/mmu_init.rs | `peek` | 961 | 7 |  |
| arch/x86_64/mm/mmu_init.rs | `freed_by_peek` | 967 | 9 |  |
| arch/x86_64/mm/mmu_init.rs | `is_live_table` | 975 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `poison_table` | 980 | 12 |  |
| arch/x86_64/mm/mmu_init.rs | `is_poisoned` | 991 | 13 |  |
| arch/x86_64/mm/mmu_init.rs | `live_count` | 1003 | 15 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_free_ring_prev_free_pid` | 1017 | 14 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_free_ring_prev_free_root` | 1030 | 18 |  |
| arch/x86_64/mm/mmu_init.rs | `new` | 1047 | 25 | yes |
| arch/x86_64/mm/mmu_init.rs | `new` | 1071 | 13 | yes |
| arch/x86_64/mm/mmu_init.rs | `pte_install_log` | 1083 | 27 |  |
| arch/x86_64/mm/mmu_init.rs | `free_user_page_tables` | 1109 | 560 |  |
| arch/x86_64/mm/mmu_init.rs | `valid_table_link_ppn` | 1668 | 24 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_alloc_fail` | 1691 | 25 |  |
| arch/x86_64/mm/mmu_init.rs | `map_page_noflush` | 1715 | 296 |  |
| arch/x86_64/mm/mmu_init.rs | `map_page` | 2010 | 24 |  |
| arch/x86_64/mm/mmu_init.rs | `map_pmd_huge_page` | 2033 | 51 |  |
| arch/x86_64/mm/mmu_init.rs | `map_pud_huge_page` | 2083 | 38 |  |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_page_noflush` | 2120 | 12 |  |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_page` | 2131 | 10 |  |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_region` | 2140 | 25 |  |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_region_huge` | 2164 | 36 |  |
| arch/x86_64/mm/mmu_init.rs | `init` | 2199 | 112 |  |
| arch/x86_64/mm/mmu_init.rs | `drop_bootstrap_identity` | 2310 | 15 |  |
| arch/x86_64/mm/mmu_init.rs | `setup_linear_mapping` | 2324 | 98 |  |
| arch/x86_64/mm/mmu_init.rs | `ioremap_device_window` | 2421 | 27 |  |
| arch/x86_64/mm/mmu_init.rs | `kernel_device_window_pte` | 2447 | 68 |  |
| arch/x86_64/mm/mmu_init.rs | `new` | 2514 | 25 | yes |
| arch/x86_64/mm/mmu_init.rs | `mark_user_mms_exist` | 2538 | 9 |  |
| arch/x86_64/mm/mmu_init.rs | `device_alias_dyn` | 2546 | 28 |  |
| arch/x86_64/mm/mmu_init.rs | `ioremap_bar_kernel` | 2573 | 32 |  |
| arch/x86_64/mm/mmu_init.rs | `setup_device_mappings` | 2604 | 61 |  |
| arch/x86_64/mm/mmu_init.rs | `enable` | 2664 | 9 |  |
| arch/x86_64/mm/mmu_init.rs | `map_identity` | 2672 | 11 |  |
| arch/x86_64/mm/mmu_init.rs | `get_kernel_page_table_ppn` | 2682 | 30 |  |
| arch/x86_64/mm/mmu_init.rs | `forensics_scan_trees` | 2711 | 149 |  |
| arch/x86_64/mm/mod.rs | `register_in_tree_mm_policy` | 47 | 6 |  |
| arch/x86_64/mm/page_fault.rs | `fill_file_page` | 56 | 41 |  |
| arch/x86_64/mm/page_fault.rs | `filemap_pin_cache_page` | 96 | 60 |  |
| arch/x86_64/mm/page_fault.rs | `filemap_bookkeep_mapped_cache_page` | 155 | 12 |  |
| arch/x86_64/mm/page_fault.rs | `release_fill_page` | 166 | 32 |  |
| arch/x86_64/mm/page_fault.rs | `get_user_phys` | 197 | 13 |  |
| arch/x86_64/mm/page_fault.rs | `write_pte_raw` | 209 | 68 |  |
| arch/x86_64/mm/page_fault.rs | `l1_table_for` | 276 | 24 |  |
| arch/x86_64/mm/page_fault.rs | `read_pte_raw` | 299 | 53 |  |
| arch/x86_64/mm/page_fault.rs | `_root_table_ref` | 351 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `new` | 63 | 5 | yes |
| arch/x86_64/mm/pagetable.rs | `from_bits` | 67 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `bits` | 71 | 5 | yes |
| arch/x86_64/mm/pagetable.rs | `is_valid` | 75 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `is_readable` | 79 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `is_writable` | 83 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `is_executable` | 87 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `is_user` | 91 | 6 |  |
| arch/x86_64/mm/pagetable.rs | `is_leaf` | 96 | 6 |  |
| arch/x86_64/mm/pagetable.rs | `is_table` | 101 | 6 |  |
| arch/x86_64/mm/pagetable.rs | `ppn` | 106 | 6 |  |
| arch/x86_64/mm/pagetable.rs | `phys_addr` | 111 | 15 |  |
| arch/x86_64/mm/pagetable.rs | `new_table` | 125 | 6 |  |
| arch/x86_64/mm/pagetable.rs | `new_page_kernel` | 130 | 6 |  |
| arch/x86_64/mm/pagetable.rs | `new_page_user` | 135 | 6 |  |
| arch/x86_64/mm/pagetable.rs | `new_page_ro` | 140 | 15 |  |
| arch/x86_64/mm/pagetable.rs | `new` | 154 | 5 | yes |
| arch/x86_64/mm/pagetable.rs | `get` | 158 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `set` | 162 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `zero` | 166 | 14 |  |
| arch/x86_64/mm/pagetable.rs | `new` | 179 | 5 | yes |
| arch/x86_64/mm/pagetable.rs | `bits` | 183 | 5 | yes |
| arch/x86_64/mm/pagetable.rs | `ppn` | 187 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `asid` | 191 | 7 |  |
| arch/x86_64/mm/pagetable.rs | `get_satp` | 197 | 4 | yes |
| arch/x86_64/mm/unsafe_edges.rs | `get_page_table_virt` | 45 | 37 |  |
| arch/x86_64/mm/unsafe_edges.rs | `from_phys` | 81 | 25 |  |
| arch/x86_64/mm/unsafe_edges.rs | `from_phys_live` | 105 | 6 |  |
| arch/x86_64/mm/unsafe_edges.rs | `as_phys_base` | 110 | 8 |  |
| arch/x86_64/mm/unsafe_edges.rs | `get_raw` | 117 | 12 |  |
| arch/x86_64/mm/unsafe_edges.rs | `get` | 128 | 8 |  |
| arch/x86_64/mm/unsafe_edges.rs | `set` | 135 | 15 |  |
| arch/x86_64/mm/unsafe_edges.rs | `set_raw` | 149 | 6 |  |
| arch/x86_64/mm/unsafe_edges.rs | `all_zero` | 154 | 12 |  |
| arch/x86_64/mm/unsafe_edges.rs | `zero_entries` | 165 | 13 |  |
| arch/x86_64/mm/unsafe_edges.rs | `static_root` | 177 | 17 |  |
| arch/x86_64/mm/unsafe_edges.rs | `copy_page` | 193 | 9 |  |
| arch/x86_64/mod.rs | `arch_init` | 23 | 5 |  |
| arch/x86_64/mod.rs | `init` | 27 | 26 | yes |
| arch/x86_64/mod.rs | `enable_interrupts` | 52 | 15 |  |
| arch/x86_64/mod.rs | `cpu_id` | 66 | 4 | yes |
| arch/x86_64/mpk.rs | `mpk_probe_write` | 57 | 7 |  |
| arch/x86_64/mpk.rs | `rdpkru` | 63 | 20 |  |
| arch/x86_64/mpk.rs | `wrpkru` | 82 | 24 |  |
| arch/x86_64/mpk.rs | `enable_pke` | 105 | 23 |  |
| arch/x86_64/mpk.rs | `probe` | 127 | 178 |  |
| arch/x86_64/mpk.rs | `closed_pkru` | 304 | 8 |  |
| arch/x86_64/mpk.rs | `guard_arm` | 311 | 20 |  |
| arch/x86_64/mpk.rs | `guard_on` | 330 | 13 |  |
| arch/x86_64/mpk.rs | `cpu_online_arm` | 342 | 26 | yes |
| arch/x86_64/mpk.rs | `zone_open` | 367 | 11 |  |
| arch/x86_64/mpk.rs | `zone_close` | 377 | 16 |  |
| arch/x86_64/mpk.rs | `switch_rearm` | 392 | 10 |  |
| arch/x86_64/mpk.rs | `tag_kernel_pages` | 401 | 55 |  |
| arch/x86_64/mpk.rs | `untag_kernel_pages` | 455 | 54 |  |
| arch/x86_64/mpk.rs | `mpk_guard_escape` | 508 | 6 |  |
| arch/x86_64/mpk.rs | `probe_guard_escape` | 513 | 12 |  |
| arch/x86_64/process.rs | `start_thread` | 21 | 32 |  |
| arch/x86_64/process.rs | `flush_thread` | 52 | 59 |  |
| arch/x86_64/process.rs | `sys_arch_prctl` | 110 | 75 |  |
| arch/x86_64/process.rs | `sys_ioperm` | 184 | 16 |  |
| arch/x86_64/process.rs | `sys_iopl` | 199 | 11 |  |
| arch/x86_64/pt_regs.rs | `new` | 83 | 12 | yes |
| arch/x86_64/pt_regs.rs | `user_mode` | 94 | 7 | yes |
| arch/x86_64/pt_regs.rs | `kernel_mode` | 100 | 7 | yes |
| arch/x86_64/pt_regs.rs | `mark_user_frame` | 106 | 8 |  |
| arch/x86_64/pt_regs.rs | `mark_kernel_frame` | 113 | 17 |  |
| arch/x86_64/pt_regs.rs | `syscall_nr` | 129 | 7 |  |
| arch/x86_64/pt_regs.rs | `syscall_args` | 135 | 7 |  |
| arch/x86_64/pt_regs.rs | `set_return_value` | 141 | 7 |  |
| arch/x86_64/pt_regs.rs | `set_return_error` | 147 | 7 |  |
| arch/x86_64/pt_regs.rs | `syscall_rollback` | 153 | 7 |  |
| arch/x86_64/pt_regs.rs | `instruction_pointer` | 159 | 7 |  |
| arch/x86_64/pt_regs.rs | `set_instruction_pointer` | 165 | 7 |  |
| arch/x86_64/pt_regs.rs | `user_stack_pointer` | 171 | 7 |  |
| arch/x86_64/pt_regs.rs | `set_user_stack_pointer` | 177 | 7 |  |
| arch/x86_64/pt_regs.rs | `frame_pointer` | 183 | 7 |  |
| arch/x86_64/pt_regs.rs | `irqs_disabled` | 189 | 41 |  |
| arch/x86_64/pt_regs.rs | `from_cause` | 229 | 36 | yes |
| arch/x86_64/pt_regs.rs | `is_interrupt` | 264 | 6 |  |
| arch/x86_64/pt_regs.rs | `is_exception` | 269 | 7 |  |
| arch/x86_64/pt_regs.rs | `is_page_fault` | 275 | 10 |  |
| arch/x86_64/pt_regs.rs | `in_interrupt` | 284 | 7 |  |
| arch/x86_64/pt_regs.rs | `in_task` | 290 | 39 |  |
| arch/x86_64/smp.rs | `new` | 97 | 17 | yes |
| arch/x86_64/smp.rs | `arm_magic` | 113 | 32 | yes |
| arch/x86_64/smp.rs | `load_per_cpu_base` | 144 | 18 | yes |
| arch/x86_64/smp.rs | `signal_boot_complete` | 161 | 5 | yes |
| arch/x86_64/smp.rs | `is_boot_complete` | 165 | 16 | yes |
| arch/x86_64/smp.rs | `init` | 180 | 10 | yes |
| arch/x86_64/smp.rs | `init_per_cpu_intr_stacks` | 189 | 4 | yes |
| arch/x86_64/smp.rs | `num_started_cpus` | 192 | 5 | yes |
| arch/x86_64/smp.rs | `cpu_started` | 196 | 6 | yes |
| arch/x86_64/smp.rs | `is_boot_hart` | 201 | 7 | yes |
| arch/x86_64/smp.rs | `gs_cpu_id` | 207 | 18 | yes |
| arch/x86_64/smp.rs | `cpu_id` | 224 | 7 | yes |
| arch/x86_64/smp.rs | `current_task_ptr` | 230 | 8 | yes |
| arch/x86_64/smp.rs | `current_task_ptr_of` | 237 | 9 | yes |
| arch/x86_64/smp.rs | `set_current_task_ptr` | 245 | 62 | yes |
| arch/x86_64/smp.rs | `ap_boot_stacks_range` | 306 | 7 | yes |
| arch/x86_64/smp.rs | `tramp_write` | 312 | 15 | yes |
| arch/x86_64/smp.rs | `tramp_write_u64` | 326 | 5 | yes |
| arch/x86_64/smp.rs | `tramp_write_u32` | 330 | 5 | yes |
| arch/x86_64/smp.rs | `tramp_write_u16` | 334 | 6 | yes |
| arch/x86_64/smp.rs | `setup_trampoline` | 339 | 68 | yes |
| arch/x86_64/smp.rs | `tramp_va` | 406 | 15 | yes |
| arch/x86_64/smp.rs | `start_secondaries` | 420 | 81 | yes |
| arch/x86_64/smp.rs | `smp_selftest` | 500 | 7 | yes |
| arch/x86_64/smp.rs | `bump` | 506 | 37 | yes |
| arch/x86_64/smp.rs | `wait_started` | 542 | 101 | yes |
| arch/x86_64/thread.rs | `zeroed` | 42 | 17 | yes |
| arch/x86_64/thread.rs | `initial` | 58 | 35 | yes |
| arch/x86_64/thread.rs | `new` | 92 | 29 | yes |
| arch/x86_64/thread.rs | `fpu_save_for_switch` | 120 | 62 | yes |
| arch/x86_64/thread.rs | `restore_fpu` | 181 | 75 | yes |
| arch/x86_64/thread.rs | `mark_fpu_clean` | 255 | 7 | yes |
| arch/x86_64/thread.rs | `tp` | 261 | 6 | yes |
| arch/x86_64/thread.rs | `set_tp` | 266 | 7 | yes |
| arch/x86_64/thread.rs | `default` | 272 | 20 | yes |
| arch/x86_64/thread.rs | `fpu_init` | 291 | 178 | yes |
| arch/x86_64/thread.rs | `xstate_sigframe_size` | 468 | 18 | yes |
| arch/x86_64/thread.rs | `avx_enabled` | 485 | 5 | yes |
| arch/x86_64/thread.rs | `xsave_ready` | 489 | 5 | yes |
| arch/x86_64/thread.rs | `xsaveopt_ready` | 493 | 4 | yes |
| arch/x86_64/trap.rs | `x86_load_gdt` | 185 | 2 | yes |
| arch/x86_64/trap.rs | `x86_load_idt` | 186 | 18 | yes |
| arch/x86_64/trap.rs | `empty` | 203 | 13 | yes |
| arch/x86_64/trap.rs | `set_handler` | 215 | 20 | yes |
| arch/x86_64/trap.rs | `zeroed` | 234 | 6 | yes |
| arch/x86_64/trap.rs | `write_u64` | 239 | 9 | yes |
| arch/x86_64/trap.rs | `read_u64` | 247 | 27 | yes |
| arch/x86_64/trap.rs | `seg_desc` | 273 | 25 | yes |
| arch/x86_64/trap.rs | `set_tss_rsp0` | 297 | 24 | yes |
| arch/x86_64/trap.rs | `init_cpu_state` | 320 | 40 | yes |
| arch/x86_64/trap.rs | `init` | 359 | 125 | yes |
| arch/x86_64/trap.rs | `init_secondary` | 483 | 24 | yes |
| arch/x86_64/trap.rs | `init_syscall` | 506 | 37 | yes |
| arch/x86_64/trap.rs | `init_8259` | 542 | 29 | yes |
| arch/x86_64/trap.rs | `pic_send_eoi` | 570 | 14 | yes |
| arch/x86_64/trap.rs | `pic_claim` | 583 | 21 | yes |
| arch/x86_64/trap.rs | `unmask_irq_line` | 603 | 16 | yes |
| arch/x86_64/trap.rs | `mask_irq_line` | 618 | 32 | yes |
| arch/x86_64/trap.rs | `request_irq_line` | 649 | 8 | yes |
| arch/x86_64/trap.rs | `request_irq_line_named` | 656 | 26 | yes |
| arch/x86_64/trap.rs | `irq_line_name` | 681 | 15 | yes |
| arch/x86_64/trap.rs | `free_irq_line` | 695 | 28 | yes |
| arch/x86_64/trap.rs | `run_line_action` | 722 | 57 | yes |
| arch/x86_64/trap.rs | `spawn_intx_irq_thread` | 778 | 37 | yes |
| arch/x86_64/trap.rs | `spawn_blk_msix_irq_thread` | 814 | 17 | yes |
| arch/x86_64/trap.rs | `irq_threads_boot_init` | 830 | 35 | yes |
| arch/x86_64/trap.rs | `enable_timer_interrupt` | 864 | 25 | yes |
| arch/x86_64/trap.rs | `disable_timer_interrupt` | 888 | 13 | yes |
| arch/x86_64/trap.rs | `enable_external_interrupt` | 900 | 11 | yes |
| arch/x86_64/trap.rs | `current_pt_regs` | 910 | 10 | yes |
| arch/x86_64/trap.rs | `pt_regs_rip_for_cpu` | 919 | 22 | yes |
| arch/x86_64/trap.rs | `current_task_pt_regs` | 940 | 47 | yes |
| arch/x86_64/trap.rs | `frame_in_task_stack` | 986 | 15 | yes |
| arch/x86_64/trap.rs | `scribble_put_hex` | 1000 | 9 | yes |
| arch/x86_64/trap.rs | `scribble_put_str` | 1008 | 10 | yes |
| arch/x86_64/trap.rs | `scribble_foreign_entry` | 1017 | 525 | yes |
| arch/x86_64/trap.rs | `ww_irqwin_budget_ok` | 1541 | 58 | yes |
| arch/x86_64/trap.rs | `irq_exit_maybe_preempt_kernel` | 1598 | 142 | yes |
| arch/x86_64/trap.rs | `handle_timer_tick` | 1739 | 16 | yes |
| arch/x86_64/trap.rs | `ph` | 1754 | 284 | yes |
| arch/x86_64/trap.rs | `handle_timer_interrupt` | 2037 | 31 | yes |
| arch/x86_64/trap.rs | `handle_external_interrupt` | 2067 | 63 | yes |
| arch/x86_64/trap.rs | `handle_syscall` | 2129 | 36 | yes |
| arch/x86_64/trap.rs | `handle_illegal_instruction` | 2164 | 102 | yes |
| arch/x86_64/trap.rs | `sigill_has_handler` | 2265 | 9 | yes |
| arch/x86_64/trap.rs | `handle_general_protection` | 2273 | 65 | yes |
| arch/x86_64/trap.rs | `handle_arith_exception` | 2337 | 18 | yes |
| arch/x86_64/trap.rs | `handle_alignment_check` | 2354 | 23 | yes |
| arch/x86_64/trap.rs | `handle_device_not_available` | 2376 | 22 | yes |
| arch/x86_64/trap.rs | `handle_debug` | 2397 | 81 | yes |
| arch/x86_64/trap.rs | `sigtrap_has_handler` | 2477 | 11 | yes |
| arch/x86_64/trap.rs | `sigtrap_blocked_current` | 2487 | 12 | yes |
| arch/x86_64/trap.rs | `handle_breakpoint` | 2498 | 72 | yes |
| arch/x86_64/trap.rs | `sigsegv_has_handler` | 2569 | 17 | yes |
| arch/x86_64/trap.rs | `sigsegv_blocked_current` | 2585 | 7 | yes |
| arch/x86_64/trap.rs | `sigbus_has_handler` | 2591 | 9 | yes |
| arch/x86_64/trap.rs | `sigfpe_has_handler` | 2599 | 15 | yes |
| arch/x86_64/trap.rs | `gp_regmem_dump` | 2613 | 109 | yes |
| arch/x86_64/trap.rs | `gp_mmhistory_dump` | 2721 | 98 | yes |
| arch/x86_64/trap.rs | `segv_hist_note` | 2818 | 18 | yes |
| arch/x86_64/trap.rs | `segv_hist_age` | 2835 | 34 | yes |
| arch/x86_64/trap.rs | `user_segv_forensics` | 2868 | 192 | yes |
| arch/x86_64/trap.rs | `dump_mm_ops` | 3059 | 299 | yes |
| arch/x86_64/trap.rs | `fore_read_u64` | 3357 | 134 | yes |
| arch/x86_64/trap.rs | `handle_page_fault` | 3490 | 474 | yes |
| arch/x86_64/trap.rs | `kp_put_hex` | 3963 | 132 | yes |
| arch/x86_64/trap.rs | `kp_read64` | 4094 | 9 | yes |
| arch/x86_64/trap.rs | `kp_read32` | 4102 | 77 | yes |
| arch/x86_64/trap.rs | `oops_show_regs` | 4178 | 45 | yes |
| arch/x86_64/trap.rs | `handle_unknown_exception` | 4222 | 141 | yes |
| arch/x86_64/trap.rs | `comm_has_launcher` | 4362 | 22 | yes |
| arch/x86_64/trap.rs | `utime_sample_rip` | 4383 | 57 | yes |
| arch/x86_64/trap.rs | `utime_ripsample_report` | 4439 | 40 | yes |
| arch/x86_64/trap.rs | `stime_sample_rip` | 4478 | 99 | yes |
| arch/x86_64/trap.rs | `stime_profile_top` | 4576 | 23 | yes |
| arch/x86_64/uaccess.rs | `__x64_copy_to_user` | 60 | 2 | yes |
| arch/x86_64/uaccess.rs | `__x64_copy_from_user` | 61 | 2 | yes |
| arch/x86_64/uaccess.rs | `__x64_clear_user` | 62 | 2 | yes |
| arch/x86_64/uaccess.rs | `__x64_futex_cmpxchg_u32` | 63 | 11 | yes |
| arch/x86_64/uaccess.rs | `futex_cmpxchg_u32_user` | 73 | 16 | yes |
| arch/x86_64/uaccess.rs | `access_ok` | 88 | 19 | yes |
| arch/x86_64/uaccess.rs | `copy_to_user` | 106 | 30 | yes |
| arch/x86_64/uaccess.rs | `copy_from_user` | 135 | 26 | yes |
| arch/x86_64/uaccess.rs | `clear_user` | 160 | 26 | yes |
| arch/x86_64/uaccess.rs | `fault_in_write` | 185 | 98 | yes |
| arch/x86_64/uaccess.rs | `get_user` | 282 | 33 | yes |
| arch/x86_64/uaccess.rs | `put_user` | 314 | 29 | yes |
| arch/x86_64/uaccess.rs | `strncpy_from_user` | 342 | 80 | yes |
| arch/x86_64/uaccess.rs | `strnlen_user` | 421 | 21 | yes |
| arch/x86_64/unsafe_edges.rs | `from_trap` | 53 | 11 | yes |
| arch/x86_64/unsafe_edges.rs | `regs` | 63 | 9 |  |
| arch/x86_64/unsafe_edges.rs | `deref` | 71 | 8 |  |
| arch/x86_64/unsafe_edges.rs | `deref_mut` | 78 | 5 |  |
