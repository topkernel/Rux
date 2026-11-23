# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch/x86_64 — hot 10370 lines (stay) / cold 5826 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/x86_64/boot.rs | `rust_main` | 21 | 60 |  |
| arch/x86_64/boot.rs | `get_boot_magic` | 81 | 7 |  |
| arch/x86_64/boot.rs | `get_dtb_pointer` | 88 | 8 | yes |
| arch/x86_64/boot.rs | `early_boot_init` | 96 | 127 |  |
| arch/x86_64/boot.rs | `boot_memory_regions` | 223 | 10 |  |
| arch/x86_64/boot.rs | `boot_cmdline` | 233 | 22 |  |
| arch/x86_64/context.rs | `__switch_to` | 46 | 25 | yes |
| arch/x86_64/context.rs | `switch_mm` | 71 | 334 | yes |
| arch/x86_64/context.rs | `set_prev_task` | 405 | 33 | yes |
| arch/x86_64/context.rs | `context_switch` | 438 | 7 | yes |
| arch/x86_64/context.rs | `context_switch_body` | 445 | 228 | yes |
| arch/x86_64/context.rs | `cpu_of` | 673 | 15 | yes |
| arch/x86_64/context.rs | `putfrag_no_lock` | 688 | 9 | yes |
| arch/x86_64/context.rs | `puthex_no_lock` | 697 | 12 | yes |
| arch/x86_64/context.rs | `puthex32_no_lock` | 709 | 90 | yes |
| arch/x86_64/cpu.rs | `inb` | 22 | 10 | yes |
| arch/x86_64/cpu.rs | `outb` | 32 | 8 | yes |
| arch/x86_64/cpu.rs | `inw` | 40 | 10 | yes |
| arch/x86_64/cpu.rs | `outw` | 50 | 8 | yes |
| arch/x86_64/cpu.rs | `inl` | 58 | 10 | yes |
| arch/x86_64/cpu.rs | `outl` | 68 | 8 | yes |
| arch/x86_64/cpu.rs | `io_wait` | 76 | 11 | yes |
| arch/x86_64/cpu.rs | `read_cr2` | 87 | 9 | yes |
| arch/x86_64/cpu.rs | `read_cr0` | 96 | 14 | yes |
| arch/x86_64/cpu.rs | `write_cr0` | 110 | 12 | yes |
| arch/x86_64/cpu.rs | `ts_set` | 122 | 7 | yes |
| arch/x86_64/cpu.rs | `clts` | 129 | 8 | yes |
| arch/x86_64/cpu.rs | `stts` | 137 | 18 | yes |
| arch/x86_64/cpu.rs | `enable_write_protect` | 155 | 8 | yes |
| arch/x86_64/cpu.rs | `read_cr3` | 163 | 12 | yes |
| arch/x86_64/cpu.rs | `write_cr3` | 175 | 9 | yes |
| arch/x86_64/cpu.rs | `rdmsr` | 184 | 13 | yes |
| arch/x86_64/cpu.rs | `wrmsr` | 197 | 16 | yes |
| arch/x86_64/cpu.rs | `invlpg` | 213 | 6 | yes |
| arch/x86_64/cpu.rs | `read_cr4` | 219 | 13 | yes |
| arch/x86_64/cpu.rs | `write_cr4` | 232 | 18 | yes |
| arch/x86_64/cpu.rs | `invpcid` | 250 | 20 | yes |
| arch/x86_64/cpu.rs | `get_core_id` | 270 | 6 | yes |
| arch/x86_64/cpu.rs | `get_thread_id_of` | 276 | 9 | yes |
| arch/x86_64/cpu.rs | `get_thread_id` | 285 | 6 | yes |
| arch/x86_64/cpu.rs | `set_thread_id` | 291 | 8 | yes |
| arch/x86_64/cpu.rs | `get_counter_freq` | 299 | 6 | yes |
| arch/x86_64/cpu.rs | `read_counter` | 305 | 25 | yes |
| arch/x86_64/cpu.rs | `smap_enabled` | 330 | 16 | yes |
| arch/x86_64/cpu.rs | `fsgsbase_enabled` | 346 | 7 | yes |
| arch/x86_64/cpu.rs | `enable_fsgsbase` | 353 | 36 | yes |
| arch/x86_64/cpu.rs | `enable_smep` | 389 | 23 | yes |
| arch/x86_64/cpu.rs | `wrfsbase` | 412 | 11 | yes |
| arch/x86_64/cpu.rs | `wrgsbase` | 423 | 22 | yes |
| arch/x86_64/cpu.rs | `smap_report` | 445 | 31 | yes |
| arch/x86_64/cpu.rs | `cpuid` | 476 | 26 | yes |
| arch/x86_64/cpu.rs | `enable_irq` | 502 | 11 | yes |
| arch/x86_64/cpu.rs | `disable_irq` | 513 | 11 | yes |
| arch/x86_64/cpu.rs | `wfi` | 524 | 19 | yes |
| arch/x86_64/cpu.rs | `read_time` | 543 | 6 | yes |
| arch/x86_64/cpu.rs | `isb` | 549 | 17 | yes |
| arch/x86_64/cpu.rs | `dsb` | 566 | 7 | yes |
| arch/x86_64/cpu.rs | `dmb` | 573 | 7 | yes |
| arch/x86_64/cpu.rs | `get_interrupts_state` | 580 | 17 | yes |
| arch/x86_64/cpu.rs | `save_and_disable_irq` | 597 | 11 | yes |
| arch/x86_64/cpu.rs | `restore_irq` | 608 | 5 | yes |
| arch/x86_64/ipi.rs | `vector` | 53 | 17 |  |
| arch/x86_64/ipi.rs | `send_ipi_type` | 70 | 24 | yes |
| arch/x86_64/ipi.rs | `send_reschedule_ipi` | 94 | 11 | yes |
| arch/x86_64/ipi.rs | `flush_tlb_others` | 105 | 19 | yes |
| arch/x86_64/ipi.rs | `flush_tlb_others_mm` | 124 | 18 |  |
| arch/x86_64/ipi.rs | `started_cpu_mask` | 142 | 31 |  |
| arch/x86_64/ipi.rs | `tlb_epoch_snapshot` | 173 | 12 |  |
| arch/x86_64/ipi.rs | `tlb_remote_targets` | 185 | 7 |  |
| arch/x86_64/ipi.rs | `tlb_epochs_advanced` | 192 | 23 |  |
| arch/x86_64/ipi.rs | `handle_ipi_vector` | 215 | 104 | yes |
| arch/x86_64/ipi.rs | `snapshot_self` | 319 | 74 |  |
| arch/x86_64/ipi.rs | `remote_bt_self` | 393 | 54 |  |
| arch/x86_64/ipi.rs | `remote_backtrace` | 447 | 103 | yes |
| arch/x86_64/ipi.rs | `new` | 550 | 20 | yes |
| arch/x86_64/ipi.rs | `csd_init` | 570 | 14 |  |
| arch/x86_64/ipi.rs | `smp_call_function` | 584 | 51 | yes |
| arch/x86_64/ipi.rs | `csd_fire` | 635 | 42 |  |
| arch/x86_64/ipi.rs | `smp_call_function_timeout` | 677 | 45 |  |
| arch/x86_64/ipi.rs | `csd_flush_queue` | 722 | 52 |  |
| arch/x86_64/ipi.rs | `init` | 774 | 3 | yes |
| arch/x86_64/mm/asid.rs | `pcid_enabled` | 46 | 27 | yes |
| arch/x86_64/mm/asid.rs | `enable_pcid` | 73 | 55 | yes |
| arch/x86_64/mm/asid.rs | `alloc_asid` | 128 | 38 | yes |
| arch/x86_64/mm/asid.rs | `free_asid` | 166 | 14 |  |
| arch/x86_64/mm/asid.rs | `ipi_flush_all` | 180 | 8 |  |
| arch/x86_64/mm/asid.rs | `flush_all_cpus_local` | 188 | 11 |  |
| arch/x86_64/mm/asid.rs | `asid_usage_count` | 199 | 11 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_all` | 210 | 15 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_asid` | 225 | 13 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_page` | 238 | 7 | yes |
| arch/x86_64/mm/asid.rs | `flush_tlb_page_asid` | 245 | 10 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_range` | 255 | 9 |  |
| arch/x86_64/mm/asid.rs | `flush_tlb_kernel` | 264 | 19 |  |
| arch/x86_64/mm/asid.rs | `build_satp` | 283 | 7 | yes |
| arch/x86_64/mm/asid.rs | `satp_to_asid` | 290 | 9 |  |
| arch/x86_64/mm/asid.rs | `satp_to_ppn` | 299 | 18 | yes |
| arch/x86_64/mm/asid.rs | `switch_root` | 317 | 27 | yes |
| arch/x86_64/mm/asid.rs | `read_satp` | 344 | 6 | yes |
| arch/x86_64/mm/asid.rs | `write_satp` | 350 | 7 | yes |
| arch/x86_64/mm/asid.rs | `new` | 357 | 5 | yes |
| arch/x86_64/mm/asid.rs | `print_asid_status` | 362 | 7 |  |
| arch/x86_64/mm/exception.rs | `fixup_exception` | 36 | 17 | yes |
| arch/x86_64/mm/exception.rs | `exception_table_empty` | 53 | 5 | yes |
| arch/x86_64/mm/exception.rs | `exception_table_count` | 58 | 14 | yes |
| arch/x86_64/mm/exception.rs | `send_signal` | 72 | 27 | yes |
| arch/x86_64/mm/exception.rs | `in_interrupt` | 99 | 7 | yes |
| arch/x86_64/mm/exception.rs | `bad_area` | 106 | 21 | yes |
| arch/x86_64/mm/exception.rs | `no_context` | 127 | 40 | yes |
| arch/x86_64/mm/exception.rs | `do_page_fault` | 167 | 260 | yes |
| arch/x86_64/mm/exception.rs | `phex` | 427 | 7 | yes |
| arch/x86_64/mm/exception.rs | `ptag` | 434 | 261 | yes |
| arch/x86_64/mm/fixmap.rs | `fix_to_virt` | 33 | 5 |  |
| arch/x86_64/mm/fixmap.rs | `virt_to_fix` | 38 | 8 |  |
| arch/x86_64/mm/fixmap.rs | `is_fixmap_addr` | 46 | 9 |  |
| arch/x86_64/mm/fixmap.rs | `set_fixmap` | 55 | 6 |  |
| arch/x86_64/mm/fixmap.rs | `clear_fixmap` | 61 | 45 |  |
| arch/x86_64/mm/fixmap.rs | `init_uart_fixmap` | 106 | 4 |  |
| arch/x86_64/mm/fixmap.rs | `uart_virt_addr` | 110 | 4 |  |
| arch/x86_64/mm/fixmap.rs | `is_uart_fixmap_initialized` | 114 | 6 |  |
| arch/x86_64/mm/fixmap.rs | `copy_fixmap_to_user` | 120 | 2 |  |
| arch/x86_64/mm/fixmap.rs | `print_fixmap_status` | 122 | 1 |  |
| arch/x86_64/mm/memory_layout.rs | `device_alias` | 125 | 15 |  |
| arch/x86_64/mm/memory_layout.rs | `device_alias_fixed` | 140 | 43 |  |
| arch/x86_64/mm/memory_layout.rs | `is_linear_mapping` | 183 | 156 |  |
| arch/x86_64/mm/memory_layout.rs | `new` | 339 | 3 | yes |
| arch/x86_64/mm/memory_layout.rs | `bits` | 342 | 3 | yes |
| arch/x86_64/mm/memory_layout.rs | `is_aligned` | 345 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `floor` | 348 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `ceil` | 351 | 7 |  |
| arch/x86_64/mm/memory_layout.rs | `page_offset` | 358 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `vpn` | 362 | 4 |  |
| arch/x86_64/mm/memory_layout.rs | `pgd_index` | 366 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `pud_index` | 369 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `pmd_index` | 372 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `pte_index` | 375 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `as_u64` | 378 | 3 | yes |
| arch/x86_64/mm/memory_layout.rs | `as_usize` | 381 | 10 | yes |
| arch/x86_64/mm/memory_layout.rs | `new` | 391 | 3 | yes |
| arch/x86_64/mm/memory_layout.rs | `bits` | 394 | 3 | yes |
| arch/x86_64/mm/memory_layout.rs | `is_aligned` | 397 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `floor` | 400 | 3 |  |
| arch/x86_64/mm/memory_layout.rs | `ceil` | 403 | 7 |  |
| arch/x86_64/mm/memory_layout.rs | `ppn` | 410 | 6 | yes |
| arch/x86_64/mm/memory_layout.rs | `phys_to_virt` | 416 | 28 | yes |
| arch/x86_64/mm/memory_layout.rs | `virt_to_phys` | 444 | 12 | yes |
| arch/x86_64/mm/memory_layout.rs | `mmio_alias` | 456 | 3 |  |
| arch/x86_64/mm/mm_ops.rs | `enable` | 53 | 19 | yes |
| arch/x86_64/mm/mm_ops.rs | `disable` | 72 | 8 | yes |
| arch/x86_64/mm/mm_ops.rs | `flush_tlb` | 80 | 5 |  |
| arch/x86_64/mm/mm_ops.rs | `flush_tlb_addr_page` | 85 | 8 |  |
| arch/x86_64/mm/mm_ops.rs | `map_single_page` | 93 | 47 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_vma` | 140 | 21 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_pages` | 161 | 336 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_clear_leaf` | 497 | 8 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_pages_slow` | 505 | 91 |  |
| arch/x86_64/mm/mm_ops.rs | `unmap_one_user_leaf_locked` | 596 | 62 |  |
| arch/x86_64/mm/mm_ops.rs | `clear_pte` | 658 | 61 |  |
| arch/x86_64/mm/mm_ops.rs | `perm_to_flags` | 719 | 40 |  |
| arch/x86_64/mm/mm_ops.rs | `walk` | 759 | 91 | yes |
| arch/x86_64/mm/mm_ops.rs | `alloc_user_phys_page` | 850 | 10 |  |
| arch/x86_64/mm/mm_ops.rs | `create_user_address_space` | 860 | 64 |  |
| arch/x86_64/mm/mm_ops.rs | `copy_kernel_mappings` | 924 | 37 |  |
| arch/x86_64/mm/mm_ops.rs | `map_user_page` | 961 | 27 |  |
| arch/x86_64/mm/mm_ops.rs | `pte_lock_for` | 988 | 28 |  |
| arch/x86_64/mm/mm_ops.rs | `pte_flag_bits` | 1016 | 26 |  |
| arch/x86_64/mm/mm_ops.rs | `leaf_pte_replace` | 1042 | 63 |  |
| arch/x86_64/mm/mm_ops.rs | `leaf_pte_install` | 1105 | 49 |  |
| arch/x86_64/mm/mm_ops.rs | `map_user_region` | 1154 | 40 | yes |
| arch/x86_64/mm/mm_ops.rs | `alloc_and_map_user_memory` | 1194 | 101 |  |
| arch/x86_64/mm/mm_ops.rs | `alloc_and_map_to_kernel_table` | 1295 | 65 |  |
| arch/x86_64/mm/mm_ops.rs | `alloc_and_map_to_user_table` | 1360 | 110 |  |
| arch/x86_64/mm/mm_ops.rs | `release_cache_mapping_pin` | 1470 | 25 |  |
| arch/x86_64/mm/mm_ops.rs | `copy_page_table_cow` | 1495 | 403 |  |
| arch/x86_64/mm/mm_ops.rs | `handle_cow_fault` | 1898 | 19 | yes |
| arch/x86_64/mm/mm_ops.rs | `drop` | 1917 | 194 | yes |
| arch/x86_64/mm/mm_ops.rs | `is_cow_page` | 2111 | 49 |  |
| arch/x86_64/mm/mm_ops.rs | `check_pte_permissions` | 2160 | 92 |  |
| arch/x86_64/mm/mm_ops.rs | `mprotect_pte_range` | 2252 | 113 | yes |
| arch/x86_64/mm/mmu_init.rs | `kernel_image_phys_end` | 48 | 5 |  |
| arch/x86_64/mm/mmu_init.rs | `raw_image_phys_end` | 53 | 13 |  |
| arch/x86_64/mm/mmu_init.rs | `root_static_phys` | 66 | 10 | yes |
| arch/x86_64/mm/mmu_init.rs | `root_static_ppn` | 76 | 12 |  |
| arch/x86_64/mm/mmu_init.rs | `root_page_table_ppn` | 88 | 47 | yes |
| arch/x86_64/mm/mmu_init.rs | `from_u8` | 135 | 14 |  |
| arch/x86_64/mm/mmu_init.rs | `get_alloc_stage` | 149 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `alloc_stage_is_late` | 155 | 5 | yes |
| arch/x86_64/mm/mmu_init.rs | `pt_ops_set_fixmap` | 160 | 5 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_ops_set_late` | 165 | 11 |  |
| arch/x86_64/mm/mmu_init.rs | `early_tables_used` | 176 | 10 |  |
| arch/x86_64/mm/mmu_init.rs | `zero_owned_pages` | 186 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `alloc_page_table` | 192 | 168 |  |
| arch/x86_64/mm/mmu_init.rs | `free_page_table_checked` | 360 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `free_page_table_checked_rooted` | 366 | 61 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp_kernel_tree_boot` | 427 | 38 |  |
| arch/x86_64/mm/mmu_init.rs | `free_page_table` | 465 | 91 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_free_emptied_table` | 556 | 46 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_pending_drain` | 602 | 19 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_pending_count` | 621 | 41 |  |
| arch/x86_64/mm/mmu_init.rs | `new` | 662 | 10 | yes |
| arch/x86_64/mm/mmu_init.rs | `bit_index` | 672 | 10 |  |
| arch/x86_64/mm/mmu_init.rs | `live_bit` | 682 | 10 |  |
| arch/x86_64/mm/mmu_init.rs | `set_live_bit` | 692 | 16 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp_boot` | 708 | 16 |  |
| arch/x86_64/mm/mmu_init.rs | `take_returns` | 724 | 12 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp` | 736 | 40 |  |
| arch/x86_64/mm/mmu_init.rs | `take` | 776 | 103 | yes |
| arch/x86_64/mm/mmu_init.rs | `new` | 879 | 25 | yes |
| arch/x86_64/mm/mmu_init.rs | `new` | 904 | 11 | yes |
| arch/x86_64/mm/mmu_init.rs | `register_fork_root` | 915 | 7 |  |
| arch/x86_64/mm/mmu_init.rs | `task_pid_alive` | 922 | 26 |  |
| arch/x86_64/mm/mmu_init.rs | `stamp_root` | 948 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `is_boot` | 954 | 7 |  |
| arch/x86_64/mm/mmu_init.rs | `peek` | 961 | 6 |  |
| arch/x86_64/mm/mmu_init.rs | `freed_by_peek` | 967 | 8 |  |
| arch/x86_64/mm/mmu_init.rs | `is_live_table` | 975 | 5 | yes |
| arch/x86_64/mm/mmu_init.rs | `poison_table` | 980 | 11 | yes |
| arch/x86_64/mm/mmu_init.rs | `is_poisoned` | 991 | 12 | yes |
| arch/x86_64/mm/mmu_init.rs | `live_count` | 1003 | 14 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_free_ring_prev_free_pid` | 1017 | 13 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_free_ring_prev_free_root` | 1030 | 17 |  |
| arch/x86_64/mm/mmu_init.rs | `new` | 1047 | 24 | yes |
| arch/x86_64/mm/mmu_init.rs | `new` | 1071 | 12 | yes |
| arch/x86_64/mm/mmu_init.rs | `pte_install_log` | 1083 | 26 |  |
| arch/x86_64/mm/mmu_init.rs | `free_user_page_tables` | 1109 | 559 | yes |
| arch/x86_64/mm/mmu_init.rs | `valid_table_link_ppn` | 1668 | 23 |  |
| arch/x86_64/mm/mmu_init.rs | `pt_alloc_fail` | 1691 | 24 |  |
| arch/x86_64/mm/mmu_init.rs | `map_page_noflush` | 1715 | 295 |  |
| arch/x86_64/mm/mmu_init.rs | `map_page` | 2010 | 23 |  |
| arch/x86_64/mm/mmu_init.rs | `map_pmd_huge_page` | 2033 | 50 |  |
| arch/x86_64/mm/mmu_init.rs | `map_pud_huge_page` | 2083 | 37 |  |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_page_noflush` | 2120 | 11 |  |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_page` | 2131 | 9 | yes |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_region` | 2140 | 24 |  |
| arch/x86_64/mm/mmu_init.rs | `map_kernel_region_huge` | 2164 | 35 |  |
| arch/x86_64/mm/mmu_init.rs | `init` | 2199 | 111 | yes |
| arch/x86_64/mm/mmu_init.rs | `drop_bootstrap_identity` | 2310 | 14 |  |
| arch/x86_64/mm/mmu_init.rs | `setup_linear_mapping` | 2324 | 97 |  |
| arch/x86_64/mm/mmu_init.rs | `ioremap_device_window` | 2421 | 26 |  |
| arch/x86_64/mm/mmu_init.rs | `kernel_device_window_pte` | 2447 | 67 |  |
| arch/x86_64/mm/mmu_init.rs | `new` | 2514 | 24 | yes |
| arch/x86_64/mm/mmu_init.rs | `mark_user_mms_exist` | 2538 | 8 |  |
| arch/x86_64/mm/mmu_init.rs | `device_alias_dyn` | 2546 | 27 |  |
| arch/x86_64/mm/mmu_init.rs | `ioremap_bar_kernel` | 2573 | 31 |  |
| arch/x86_64/mm/mmu_init.rs | `setup_device_mappings` | 2604 | 60 |  |
| arch/x86_64/mm/mmu_init.rs | `enable` | 2664 | 8 | yes |
| arch/x86_64/mm/mmu_init.rs | `map_identity` | 2672 | 10 |  |
| arch/x86_64/mm/mmu_init.rs | `get_kernel_page_table_ppn` | 2682 | 29 |  |
| arch/x86_64/mm/mmu_init.rs | `forensics_scan_trees` | 2711 | 148 |  |
| arch/x86_64/mm/mod.rs | `register_in_tree_mm_policy` | 47 | 5 |  |
| arch/x86_64/mm/page_fault.rs | `fill_file_page` | 56 | 40 |  |
| arch/x86_64/mm/page_fault.rs | `filemap_pin_cache_page` | 96 | 59 |  |
| arch/x86_64/mm/page_fault.rs | `filemap_bookkeep_mapped_cache_page` | 155 | 11 |  |
| arch/x86_64/mm/page_fault.rs | `release_fill_page` | 166 | 31 |  |
| arch/x86_64/mm/page_fault.rs | `get_user_phys` | 197 | 12 |  |
| arch/x86_64/mm/page_fault.rs | `write_pte_raw` | 209 | 67 |  |
| arch/x86_64/mm/page_fault.rs | `l1_table_for` | 276 | 23 |  |
| arch/x86_64/mm/page_fault.rs | `read_pte_raw` | 299 | 52 |  |
| arch/x86_64/mm/page_fault.rs | `_root_table_ref` | 351 | 4 |  |
| arch/x86_64/mm/pagetable.rs | `new` | 63 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `from_bits` | 67 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `bits` | 71 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `is_valid` | 75 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `is_readable` | 79 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `is_writable` | 83 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `is_executable` | 87 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `is_user` | 91 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `is_leaf` | 96 | 5 | yes |
| arch/x86_64/mm/pagetable.rs | `is_table` | 101 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `ppn` | 106 | 5 | yes |
| arch/x86_64/mm/pagetable.rs | `phys_addr` | 111 | 14 |  |
| arch/x86_64/mm/pagetable.rs | `new_table` | 125 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `new_page_kernel` | 130 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `new_page_user` | 135 | 5 |  |
| arch/x86_64/mm/pagetable.rs | `new_page_ro` | 140 | 14 |  |
| arch/x86_64/mm/pagetable.rs | `new` | 154 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `get` | 158 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `set` | 162 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `zero` | 166 | 13 |  |
| arch/x86_64/mm/pagetable.rs | `new` | 179 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `bits` | 183 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `ppn` | 187 | 4 | yes |
| arch/x86_64/mm/pagetable.rs | `asid` | 191 | 6 | yes |
| arch/x86_64/mm/pagetable.rs | `get_satp` | 197 | 3 | yes |
| arch/x86_64/mm/unsafe_edges.rs | `get_page_table_virt` | 45 | 36 | yes |
| arch/x86_64/mm/unsafe_edges.rs | `from_phys` | 81 | 24 | yes |
| arch/x86_64/mm/unsafe_edges.rs | `from_phys_live` | 105 | 5 |  |
| arch/x86_64/mm/unsafe_edges.rs | `as_phys_base` | 110 | 7 |  |
| arch/x86_64/mm/unsafe_edges.rs | `get_raw` | 117 | 11 |  |
| arch/x86_64/mm/unsafe_edges.rs | `get` | 128 | 7 | yes |
| arch/x86_64/mm/unsafe_edges.rs | `set` | 135 | 14 | yes |
| arch/x86_64/mm/unsafe_edges.rs | `set_raw` | 149 | 5 |  |
| arch/x86_64/mm/unsafe_edges.rs | `all_zero` | 154 | 11 |  |
| arch/x86_64/mm/unsafe_edges.rs | `zero_entries` | 165 | 12 |  |
| arch/x86_64/mm/unsafe_edges.rs | `static_root` | 177 | 16 |  |
| arch/x86_64/mm/unsafe_edges.rs | `copy_page` | 193 | 8 |  |
| arch/x86_64/mod.rs | `arch_init` | 23 | 4 |  |
| arch/x86_64/mod.rs | `init` | 27 | 25 | yes |
| arch/x86_64/mod.rs | `enable_interrupts` | 52 | 14 |  |
| arch/x86_64/mod.rs | `cpu_id` | 66 | 3 | yes |
| arch/x86_64/mpk.rs | `mpk_probe_write` | 57 | 6 |  |
| arch/x86_64/mpk.rs | `rdpkru` | 63 | 19 |  |
| arch/x86_64/mpk.rs | `wrpkru` | 82 | 23 |  |
| arch/x86_64/mpk.rs | `enable_pke` | 105 | 22 |  |
| arch/x86_64/mpk.rs | `probe` | 127 | 177 |  |
| arch/x86_64/mpk.rs | `closed_pkru` | 304 | 7 |  |
| arch/x86_64/mpk.rs | `guard_arm` | 311 | 19 |  |
| arch/x86_64/mpk.rs | `guard_on` | 330 | 12 | yes |
| arch/x86_64/mpk.rs | `cpu_online_arm` | 342 | 25 | yes |
| arch/x86_64/mpk.rs | `zone_open` | 367 | 10 | yes |
| arch/x86_64/mpk.rs | `zone_close` | 377 | 15 | yes |
| arch/x86_64/mpk.rs | `switch_rearm` | 392 | 9 | yes |
| arch/x86_64/mpk.rs | `tag_kernel_pages` | 401 | 54 | yes |
| arch/x86_64/mpk.rs | `untag_kernel_pages` | 455 | 53 | yes |
| arch/x86_64/mpk.rs | `mpk_guard_escape` | 508 | 5 |  |
| arch/x86_64/mpk.rs | `probe_guard_escape` | 513 | 11 |  |
| arch/x86_64/process.rs | `start_thread` | 21 | 31 |  |
| arch/x86_64/process.rs | `flush_thread` | 52 | 58 |  |
| arch/x86_64/process.rs | `sys_arch_prctl` | 110 | 74 |  |
| arch/x86_64/process.rs | `sys_ioperm` | 184 | 15 |  |
| arch/x86_64/process.rs | `sys_iopl` | 199 | 10 |  |
| arch/x86_64/pt_regs.rs | `new` | 83 | 11 | yes |
| arch/x86_64/pt_regs.rs | `user_mode` | 94 | 6 | yes |
| arch/x86_64/pt_regs.rs | `kernel_mode` | 100 | 6 | yes |
| arch/x86_64/pt_regs.rs | `mark_user_frame` | 106 | 7 |  |
| arch/x86_64/pt_regs.rs | `mark_kernel_frame` | 113 | 16 |  |
| arch/x86_64/pt_regs.rs | `syscall_nr` | 129 | 6 | yes |
| arch/x86_64/pt_regs.rs | `syscall_args` | 135 | 6 |  |
| arch/x86_64/pt_regs.rs | `set_return_value` | 141 | 6 |  |
| arch/x86_64/pt_regs.rs | `set_return_error` | 147 | 6 |  |
| arch/x86_64/pt_regs.rs | `syscall_rollback` | 153 | 6 |  |
| arch/x86_64/pt_regs.rs | `instruction_pointer` | 159 | 6 | yes |
| arch/x86_64/pt_regs.rs | `set_instruction_pointer` | 165 | 6 |  |
| arch/x86_64/pt_regs.rs | `user_stack_pointer` | 171 | 6 |  |
| arch/x86_64/pt_regs.rs | `set_user_stack_pointer` | 177 | 6 |  |
| arch/x86_64/pt_regs.rs | `frame_pointer` | 183 | 6 |  |
| arch/x86_64/pt_regs.rs | `irqs_disabled` | 189 | 40 |  |
| arch/x86_64/pt_regs.rs | `from_cause` | 229 | 35 | yes |
| arch/x86_64/pt_regs.rs | `is_interrupt` | 264 | 5 |  |
| arch/x86_64/pt_regs.rs | `is_exception` | 269 | 6 |  |
| arch/x86_64/pt_regs.rs | `is_page_fault` | 275 | 9 |  |
| arch/x86_64/pt_regs.rs | `in_interrupt` | 284 | 6 | yes |
| arch/x86_64/pt_regs.rs | `in_task` | 290 | 38 | yes |
| arch/x86_64/smp.rs | `new` | 97 | 16 | yes |
| arch/x86_64/smp.rs | `arm_magic` | 113 | 31 | yes |
| arch/x86_64/smp.rs | `load_per_cpu_base` | 144 | 17 | yes |
| arch/x86_64/smp.rs | `signal_boot_complete` | 161 | 4 | yes |
| arch/x86_64/smp.rs | `is_boot_complete` | 165 | 15 | yes |
| arch/x86_64/smp.rs | `init` | 180 | 9 | yes |
| arch/x86_64/smp.rs | `init_per_cpu_intr_stacks` | 189 | 3 | yes |
| arch/x86_64/smp.rs | `num_started_cpus` | 192 | 4 | yes |
| arch/x86_64/smp.rs | `cpu_started` | 196 | 5 | yes |
| arch/x86_64/smp.rs | `is_boot_hart` | 201 | 6 | yes |
| arch/x86_64/smp.rs | `gs_cpu_id` | 207 | 17 | yes |
| arch/x86_64/smp.rs | `cpu_id` | 224 | 6 | yes |
| arch/x86_64/smp.rs | `current_task_ptr` | 230 | 7 | yes |
| arch/x86_64/smp.rs | `current_task_ptr_of` | 237 | 8 | yes |
| arch/x86_64/smp.rs | `set_current_task_ptr` | 245 | 61 | yes |
| arch/x86_64/smp.rs | `ap_boot_stacks_range` | 306 | 6 | yes |
| arch/x86_64/smp.rs | `tramp_write` | 312 | 14 | yes |
| arch/x86_64/smp.rs | `tramp_write_u64` | 326 | 4 | yes |
| arch/x86_64/smp.rs | `tramp_write_u32` | 330 | 4 | yes |
| arch/x86_64/smp.rs | `tramp_write_u16` | 334 | 5 | yes |
| arch/x86_64/smp.rs | `setup_trampoline` | 339 | 67 | yes |
| arch/x86_64/smp.rs | `tramp_va` | 406 | 14 | yes |
| arch/x86_64/smp.rs | `start_secondaries` | 420 | 80 | yes |
| arch/x86_64/smp.rs | `smp_selftest` | 500 | 6 | yes |
| arch/x86_64/smp.rs | `bump` | 506 | 36 | yes |
| arch/x86_64/smp.rs | `wait_started` | 542 | 100 | yes |
| arch/x86_64/thread.rs | `zeroed` | 42 | 16 | yes |
| arch/x86_64/thread.rs | `initial` | 58 | 34 | yes |
| arch/x86_64/thread.rs | `new` | 92 | 28 | yes |
| arch/x86_64/thread.rs | `fpu_save_for_switch` | 120 | 61 | yes |
| arch/x86_64/thread.rs | `restore_fpu` | 181 | 74 | yes |
| arch/x86_64/thread.rs | `mark_fpu_clean` | 255 | 6 | yes |
| arch/x86_64/thread.rs | `tp` | 261 | 5 | yes |
| arch/x86_64/thread.rs | `set_tp` | 266 | 6 | yes |
| arch/x86_64/thread.rs | `default` | 272 | 19 | yes |
| arch/x86_64/thread.rs | `fpu_init` | 291 | 177 | yes |
| arch/x86_64/thread.rs | `xstate_sigframe_size` | 468 | 17 | yes |
| arch/x86_64/thread.rs | `avx_enabled` | 485 | 4 | yes |
| arch/x86_64/thread.rs | `xsave_ready` | 489 | 4 | yes |
| arch/x86_64/thread.rs | `xsaveopt_ready` | 493 | 3 | yes |
| arch/x86_64/trap.rs | `x86_load_gdt` | 185 | 1 | yes |
| arch/x86_64/trap.rs | `x86_load_idt` | 186 | 17 | yes |
| arch/x86_64/trap.rs | `empty` | 203 | 12 | yes |
| arch/x86_64/trap.rs | `set_handler` | 215 | 19 | yes |
| arch/x86_64/trap.rs | `zeroed` | 234 | 5 | yes |
| arch/x86_64/trap.rs | `write_u64` | 239 | 8 | yes |
| arch/x86_64/trap.rs | `read_u64` | 247 | 26 | yes |
| arch/x86_64/trap.rs | `seg_desc` | 273 | 24 | yes |
| arch/x86_64/trap.rs | `set_tss_rsp0` | 297 | 23 | yes |
| arch/x86_64/trap.rs | `init_cpu_state` | 320 | 39 | yes |
| arch/x86_64/trap.rs | `init` | 359 | 124 | yes |
| arch/x86_64/trap.rs | `init_secondary` | 483 | 23 | yes |
| arch/x86_64/trap.rs | `init_syscall` | 506 | 36 | yes |
| arch/x86_64/trap.rs | `init_8259` | 542 | 28 | yes |
| arch/x86_64/trap.rs | `pic_send_eoi` | 570 | 13 | yes |
| arch/x86_64/trap.rs | `pic_claim` | 583 | 20 | yes |
| arch/x86_64/trap.rs | `unmask_irq_line` | 603 | 15 | yes |
| arch/x86_64/trap.rs | `mask_irq_line` | 618 | 31 | yes |
| arch/x86_64/trap.rs | `request_irq_line` | 649 | 7 | yes |
| arch/x86_64/trap.rs | `request_irq_line_named` | 656 | 25 | yes |
| arch/x86_64/trap.rs | `irq_line_name` | 681 | 14 | yes |
| arch/x86_64/trap.rs | `free_irq_line` | 695 | 27 | yes |
| arch/x86_64/trap.rs | `run_line_action` | 722 | 56 | yes |
| arch/x86_64/trap.rs | `spawn_intx_irq_thread` | 778 | 36 | yes |
| arch/x86_64/trap.rs | `spawn_blk_msix_irq_thread` | 814 | 16 | yes |
| arch/x86_64/trap.rs | `irq_threads_boot_init` | 830 | 34 | yes |
| arch/x86_64/trap.rs | `enable_timer_interrupt` | 864 | 24 | yes |
| arch/x86_64/trap.rs | `disable_timer_interrupt` | 888 | 12 | yes |
| arch/x86_64/trap.rs | `enable_external_interrupt` | 900 | 10 | yes |
| arch/x86_64/trap.rs | `current_pt_regs` | 910 | 9 | yes |
| arch/x86_64/trap.rs | `pt_regs_rip_for_cpu` | 919 | 21 | yes |
| arch/x86_64/trap.rs | `current_task_pt_regs` | 940 | 46 | yes |
| arch/x86_64/trap.rs | `frame_in_task_stack` | 986 | 14 | yes |
| arch/x86_64/trap.rs | `scribble_put_hex` | 1000 | 8 | yes |
| arch/x86_64/trap.rs | `scribble_put_str` | 1008 | 9 | yes |
| arch/x86_64/trap.rs | `scribble_foreign_entry` | 1017 | 524 | yes |
| arch/x86_64/trap.rs | `ww_irqwin_budget_ok` | 1541 | 57 | yes |
| arch/x86_64/trap.rs | `irq_exit_maybe_preempt_kernel` | 1598 | 141 | yes |
| arch/x86_64/trap.rs | `handle_timer_tick` | 1739 | 15 | yes |
| arch/x86_64/trap.rs | `ph` | 1754 | 283 | yes |
| arch/x86_64/trap.rs | `handle_timer_interrupt` | 2037 | 30 | yes |
| arch/x86_64/trap.rs | `handle_external_interrupt` | 2067 | 62 | yes |
| arch/x86_64/trap.rs | `handle_syscall` | 2129 | 35 | yes |
| arch/x86_64/trap.rs | `handle_illegal_instruction` | 2164 | 101 | yes |
| arch/x86_64/trap.rs | `sigill_has_handler` | 2265 | 8 | yes |
| arch/x86_64/trap.rs | `handle_general_protection` | 2273 | 64 | yes |
| arch/x86_64/trap.rs | `handle_arith_exception` | 2337 | 17 | yes |
| arch/x86_64/trap.rs | `handle_alignment_check` | 2354 | 22 | yes |
| arch/x86_64/trap.rs | `handle_device_not_available` | 2376 | 21 | yes |
| arch/x86_64/trap.rs | `handle_debug` | 2397 | 80 | yes |
| arch/x86_64/trap.rs | `sigtrap_has_handler` | 2477 | 10 | yes |
| arch/x86_64/trap.rs | `sigtrap_blocked_current` | 2487 | 11 | yes |
| arch/x86_64/trap.rs | `handle_breakpoint` | 2498 | 71 | yes |
| arch/x86_64/trap.rs | `sigsegv_has_handler` | 2569 | 16 | yes |
| arch/x86_64/trap.rs | `sigsegv_blocked_current` | 2585 | 6 | yes |
| arch/x86_64/trap.rs | `sigbus_has_handler` | 2591 | 8 | yes |
| arch/x86_64/trap.rs | `sigfpe_has_handler` | 2599 | 14 | yes |
| arch/x86_64/trap.rs | `gp_regmem_dump` | 2613 | 108 | yes |
| arch/x86_64/trap.rs | `gp_mmhistory_dump` | 2721 | 97 | yes |
| arch/x86_64/trap.rs | `segv_hist_note` | 2818 | 17 | yes |
| arch/x86_64/trap.rs | `segv_hist_age` | 2835 | 33 | yes |
| arch/x86_64/trap.rs | `user_segv_forensics` | 2868 | 191 | yes |
| arch/x86_64/trap.rs | `dump_mm_ops` | 3059 | 298 | yes |
| arch/x86_64/trap.rs | `fore_read_u64` | 3357 | 133 | yes |
| arch/x86_64/trap.rs | `handle_page_fault` | 3490 | 473 | yes |
| arch/x86_64/trap.rs | `kp_put_hex` | 3963 | 131 | yes |
| arch/x86_64/trap.rs | `kp_read64` | 4094 | 8 | yes |
| arch/x86_64/trap.rs | `kp_read32` | 4102 | 76 | yes |
| arch/x86_64/trap.rs | `oops_show_regs` | 4178 | 44 | yes |
| arch/x86_64/trap.rs | `handle_unknown_exception` | 4222 | 140 | yes |
| arch/x86_64/trap.rs | `comm_has_launcher` | 4362 | 21 | yes |
| arch/x86_64/trap.rs | `utime_sample_rip` | 4383 | 56 | yes |
| arch/x86_64/trap.rs | `utime_ripsample_report` | 4439 | 39 | yes |
| arch/x86_64/trap.rs | `stime_sample_rip` | 4478 | 98 | yes |
| arch/x86_64/trap.rs | `stime_profile_top` | 4576 | 22 | yes |
| arch/x86_64/uaccess.rs | `__x64_copy_to_user` | 60 | 1 | yes |
| arch/x86_64/uaccess.rs | `__x64_copy_from_user` | 61 | 1 | yes |
| arch/x86_64/uaccess.rs | `__x64_clear_user` | 62 | 1 | yes |
| arch/x86_64/uaccess.rs | `__x64_futex_cmpxchg_u32` | 63 | 10 | yes |
| arch/x86_64/uaccess.rs | `futex_cmpxchg_u32_user` | 73 | 15 | yes |
| arch/x86_64/uaccess.rs | `access_ok` | 88 | 18 | yes |
| arch/x86_64/uaccess.rs | `copy_to_user` | 106 | 29 | yes |
| arch/x86_64/uaccess.rs | `copy_from_user` | 135 | 25 | yes |
| arch/x86_64/uaccess.rs | `clear_user` | 160 | 25 | yes |
| arch/x86_64/uaccess.rs | `fault_in_write` | 185 | 97 | yes |
| arch/x86_64/uaccess.rs | `get_user` | 282 | 32 | yes |
| arch/x86_64/uaccess.rs | `put_user` | 314 | 28 | yes |
| arch/x86_64/uaccess.rs | `strncpy_from_user` | 342 | 79 | yes |
| arch/x86_64/uaccess.rs | `strnlen_user` | 421 | 20 | yes |
| arch/x86_64/unsafe_edges.rs | `from_trap` | 53 | 10 | yes |
| arch/x86_64/unsafe_edges.rs | `regs` | 63 | 8 |  |
| arch/x86_64/unsafe_edges.rs | `deref` | 71 | 7 | yes |
| arch/x86_64/unsafe_edges.rs | `deref_mut` | 78 | 4 | yes |
