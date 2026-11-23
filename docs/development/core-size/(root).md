# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# (root) — hot 4333 lines (stay) / cold 10323 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| boot_edges.rs | `image_end_va` | 21 | 12 |  |
| boot_edges.rs | `set_va_pa_offset` | 33 | 12 |  |
| boot_edges.rs | `zeropage_initrd_hint` | 45 | 27 |  |
| boot_edges.rs | `regions_from_arch_boot` | 72 | 28 |  |
| boot_edges.rs | `panic_hex` | 100 | 21 |  |
| boot_edges.rs | `rawframes_line` | 121 | 47 |  |
| boot_edges.rs | `x86_panic_reset` | 168 | 31 |  |
| boot_edges.rs | `x86_reboot_pulse` | 199 | 13 |  |
| boot_edges.rs | `sbi_shutdown_ecall` | 212 | 13 |  |
| cmdline.rs | `fdt_u8` | 85 | 8 | yes |
| cmdline.rs | `fdt_add` | 93 | 8 | yes |
| cmdline.rs | `fdt_u32` | 101 | 6 | yes |
| cmdline.rs | `parse_bootargs` | 107 | 143 |  |
| cmdline.rs | `parse_memory_regions` | 250 | 159 |  |
| cmdline.rs | `parse_cpu_isa_has_v` | 409 | 90 |  |
| cmdline.rs | `parse_initrd_region` | 499 | 10 |  |
| cmdline.rs | `parse_chosen_u64_pair` | 509 | 11 |  |
| cmdline.rs | `parse_chosen_prop` | 520 | 100 |  |
| cmdline.rs | `init` | 620 | 36 | yes |
| cmdline.rs | `init_from` | 656 | 14 |  |
| cmdline.rs | `install` | 670 | 42 |  |
| cmdline.rs | `get_cmdline` | 712 | 31 | yes |
| cmdline.rs | `get_param` | 743 | 20 | yes |
| cmdline.rs | `parse_ipv4_addr` | 763 | 31 |  |
| cmdline.rs | `has_param` | 794 | 19 |  |
| cmdline.rs | `get_all_params` | 813 | 23 |  |
| cmdline.rs | `get_root_device` | 836 | 13 |  |
| cmdline.rs | `get_init_program` | 849 | 9 |  |
| cmdline.rs | `absolutize_init_path` | 858 | 12 |  |
| cmdline.rs | `get_rdinit_program` | 870 | 8 |  |
| cmdline.rs | `get_root_fs_type` | 878 | 8 |  |
| cmdline.rs | `get_root_flags` | 886 | 12 |  |
| cmdline.rs | `is_root_readonly` | 898 | 6 |  |
| cmdline.rs | `is_debug_mode` | 904 | 10 |  |
| cmdline.rs | `get_console_device` | 914 | 13 |  |
| cmdline.rs | `get_console_devices` | 927 | 23 |  |
| console.rs | `report_tx_stall_recovery` | 111 | 86 |  |
| console.rs | `get_uart_base` | 197 | 11 |  |
| console.rs | `new` | 208 | 6 | yes |
| console.rs | `putc` | 214 | 128 |  |
| console.rs | `puts_batch` | 342 | 130 |  |
| console.rs | `early_init` | 472 | 34 |  |
| console.rs | `early_init` | 506 | 17 |  |
| console.rs | `early_init` | 523 | 3 |  |
| console.rs | `init` | 526 | 8 | yes |
| console.rs | `init_irq` | 534 | 49 |  |
| console.rs | `init_irq` | 583 | 1 |  |
| console.rs | `uart4_irq_line` | 584 | 28 |  |
| console.rs | `init_irq` | 612 | 7 |  |
| console.rs | `write_reg` | 619 | 6 |  |
| console.rs | `read_reg` | 625 | 5 |  |
| console.rs | `write_reg` | 630 | 7 |  |
| console.rs | `read_reg` | 637 | 20 |  |
| console.rs | `uart_rx_debug_magic` | 657 | 59 |  |
| console.rs | `uart_irq_handler` | 716 | 59 |  |
| console.rs | `tty_isig_record` | 775 | 24 |  |
| console.rs | `tty_isig_deliver_pending` | 799 | 12 |  |
| console.rs | `tty_isig_deliver` | 811 | 27 |  |
| console.rs | `putchar` | 838 | 9 | yes |
| console.rs | `puts` | 847 | 13 |  |
| console.rs | `puts_bytes` | 860 | 6 |  |
| console.rs | `lock` | 866 | 12 | yes |
| console.rs | `putchar_no_lock` | 878 | 6 | yes |
| console.rs | `putchar_no_lock` | 884 | 7 | yes |
| console.rs | `uart_putc_raw` | 891 | 7 | yes |
| console.rs | `puts_no_lock` | 898 | 6 |  |
| console.rs | `puts_no_lock` | 904 | 5 |  |
| console.rs | `puts_bytes_no_lock` | 909 | 6 |  |
| console.rs | `puts_bytes_no_lock` | 915 | 9 |  |
| console.rs | `uart_data_ready` | 924 | 6 |  |
| console.rs | `uart_data_ready` | 930 | 24 |  |
| console.rs | `uart_rx_bridge_to_tty` | 954 | 13 |  |
| console.rs | `getchar` | 967 | 83 |  |
| console.rs | `read_waitq` | 1050 | 7 | yes |
| console.rs | `uart_has_data` | 1057 | 21 | yes |
| console.rs | `uart_has_data` | 1078 | 13 | yes |
| console.rs | `uart_has_data` | 1091 | 6 | yes |
| console.rs | `pl011_rx_pending` | 1097 | 12 |  |
| console.rs | `init_irq` | 1109 | 1 |  |
| console.rs | `pl011_irq_handler` | 1110 | 73 |  |
| console.rs | `uart_has_data` | 1183 | 11 | yes |
| console.rs | `process_input` | 1194 | 65 |  |
| console.rs | `write_str` | 1259 | 61 | yes |
| console.rs | `set_framebuffer_info` | 1320 | 5 |  |
| console.rs | `get_framebuffer_info` | 1325 | 3 |  |
| cov.rs | `cov_line` | 36 | 14 |  |
| cov.rs | `trigger_write` | 50 | 8 |  |
| cov.rs | `trigger_generate` | 58 | 12 |  |
| cov.rs | `capture_profile` | 70 | 15 |  |
| cov.rs | `dump_to_console` | 85 | 20 |  |
| cov.rs | `console_dump_active` | 105 | 4 |  |
| cov.rs | `dump_inner` | 109 | 21 |  |
| errno.rs | `as_i32` | 151 | 6 |  |
| errno.rs | `as_neg_i32` | 157 | 13 | yes |
| errno.rs | `from_neg_i32` | 170 | 48 |  |
| errno.rs | `as_neg_u64` | 218 | 154 |  |
| errno.rs | `from` | 372 | 4 | yes |
| init.rs | `init_task_storage_addr` | 33 | 28 | yes |
| init.rs | `init` | 61 | 40 | yes |
| init.rs | `init_candidates` | 101 | 45 |  |
| init.rs | `load_init_program` | 146 | 37 |  |
| init.rs | `create_and_start_init_process` | 183 | 105 |  |
| init.rs | `load_and_setup_elf` | 288 | 149 |  |
| init.rs | `ret_from_exception` | 437 | 2 |  |
| init.rs | `ret_from_fork` | 439 | 4 | yes |
| init.rs | `ret_from_fork` | 443 | 23 | yes |
| init.rs | `init_std_fds_for_task` | 466 | 30 |  |
| init.rs | `halt` | 496 | 5 |  |
| initrd.rs | `set_region` | 50 | 6 |  |
| initrd.rs | `region` | 56 | 11 |  |
| initrd.rs | `loaded` | 67 | 22 |  |
| initrd.rs | `load` | 89 | 38 | yes |
| initrd.rs | `as_slice` | 127 | 21 | yes |
| initrd.rs | `new` | 148 | 12 | yes |
| initrd.rs | `bits` | 160 | 16 | yes |
| initrd.rs | `align_byte` | 176 | 17 |  |
| initrd.rs | `new` | 193 | 29 | yes |
| initrd.rs | `decode` | 222 | 46 |  |
| initrd.rs | `inflate_raw` | 268 | 50 |  |
| initrd.rs | `fixed_litlen_table` | 318 | 14 |  |
| initrd.rs | `fixed_dist_table` | 332 | 6 |  |
| initrd.rs | `read_dynamic_tables` | 338 | 66 |  |
| initrd.rs | `inflate_block` | 404 | 47 |  |
| initrd.rs | `crc32` | 451 | 19 |  |
| initrd.rs | `gunzip` | 470 | 85 |  |
| initrd.rs | `parse_hex8` | 555 | 18 |  |
| initrd.rs | `align4` | 573 | 13 |  |
| initrd.rs | `unpack_newc` | 586 | 144 |  |
| initrd.rs | `mkdir_p` | 730 | 10 |  |
| initrd.rs | `mkdir_p_parents` | 740 | 16 |  |
| initrd.rs | `create_file` | 756 | 14 |  |
| initrd.rs | `set_node_meta` | 770 | 9 |  |
| lib.rs | `print_status` | 35 | 10 | yes |
| lib.rs | `print_status_ex` | 45 | 13 |  |
| lib.rs | `push` | 58 | 122 | yes |
| lib.rs | `alloc_error_handler` | 180 | 84 |  |
| lib.rs | `format_hex` | 264 | 8 |  |
| lib.rs | `rust_main_tail` | 272 | 1154 |  |
| lib.rs | `x86_boot_memory_regions` | 1426 | 8 |  |
| lib.rs | `x86_dump_regions` | 1434 | 397 |  |
| lib.rs | `mount_root_filesystem` | 1831 | 53 |  |
| lib.rs | `mount_boot_disk_ext4` | 1884 | 16 |  |
| lib.rs | `mount_ext4_root_from` | 1900 | 43 |  |
| lib.rs | `panic` | 1943 | 70 |  |
| list.rs | `new` | 38 | 12 | yes |
| list.rs | `init` | 50 | 8 | yes |
| list.rs | `is_empty` | 58 | 13 | yes |
| list.rs | `add` | 71 | 19 | yes |
| list.rs | `add_tail` | 90 | 16 | yes |
| list.rs | `del` | 106 | 32 | yes |
| list.rs | `entry` | 138 | 16 | yes |
| list.rs | `for_each` | 154 | 38 | yes |
| list.rs | `first_entry` | 192 | 10 |  |
| list.rs | `offset` | 202 | 8 | yes |
| list.rs | `offset` | 210 | 17 | yes |
| panic_policy.rs | `parse` | 50 | 16 |  |
| panic_policy.rs | `current` | 66 | 9 | yes |
| panic_policy.rs | `apply` | 75 | 24 |  |
| print.rs | `write_str` | 22 | 74 | yes |
| printk.rs | `empty` | 86 | 39 | yes |
| printk.rs | `ring_rec` | 125 | 13 |  |
| printk.rs | `ring_rec_mut` | 138 | 10 |  |
| printk.rs | `rec_text_len` | 148 | 49 |  |
| printk.rs | `try_new` | 197 | 14 |  |
| printk.rs | `drop` | 211 | 9 | yes |
| printk.rs | `set_console_loglevel` | 220 | 5 |  |
| printk.rs | `get_console_loglevel` | 225 | 6 |  |
| printk.rs | `init` | 231 | 13 | yes |
| printk.rs | `write_str` | 244 | 22 | yes |
| printk.rs | `printk` | 266 | 16 |  |
| printk.rs | `printk_ring_always` | 282 | 14 |  |
| printk.rs | `printk_ring_always_ex` | 296 | 47 |  |
| printk.rs | `ring_stash_console` | 343 | 53 |  |
| printk.rs | `console_emit_skipped` | 396 | 20 |  |
| printk.rs | `acquire_emit_lock` | 416 | 40 |  |
| printk.rs | `emit_to_console` | 456 | 37 |  |
| printk.rs | `try_console_emit_lock` | 493 | 23 |  |
| printk.rs | `console_emit_unlock` | 516 | 17 |  |
| printk.rs | `try_console_emit_lock_ms` | 533 | 19 |  |
| printk.rs | `printk_ln` | 552 | 22 |  |
| printk.rs | `printk_bytes` | 574 | 45 |  |
| printk.rs | `printk_should_defer_output` | 619 | 28 |  |
| printk.rs | `console_emit_pending_flag` | 647 | 14 | yes |
| printk.rs | `console_drain_pending` | 661 | 42 | yes |
| printk.rs | `write_to_ring_buffer` | 703 | 75 |  |
| printk.rs | `sys_syslog` | 778 | 96 |  |
| printk.rs | `format_syslog_header` | 874 | 98 |  |
| printk.rs | `format_record_header` | 972 | 113 |  |
| printk.rs | `syslog_read_sequential` | 1085 | 79 |  |
| printk.rs | `syslog_read_all` | 1164 | 86 |  |
| printk.rs | `syslog_clear` | 1250 | 18 |  |
| printk.rs | `ring_tail` | 1268 | 90 |  |
| printk.rs | `ring_tail_into` | 1358 | 1 |  |
| printk.rs | `push` | 1359 | 77 | yes |
| printk.rs | `generate_kmsg` | 1436 | 67 |  |
| printk.rs | `service_deferred_kmsg_wake` | 1503 | 30 | yes |
| printk.rs | `kmsg_file_read` | 1533 | 149 |  |
| printk.rs | `set_kmsg_console_mode` | 1682 | 9 |  |
| printk.rs | `kmsg_console_mode` | 1691 | 5 |  |
| printk.rs | `kmsg_console_mode_from_str` | 1696 | 12 |  |
| printk.rs | `kmsg_console_allows_echo` | 1708 | 37 |  |
| printk.rs | `kmsg_file_write` | 1745 | 19 |  |
| printk.rs | `kmsg_file_poll` | 1764 | 16 |  |
| printk.rs | `kmsg_file_lseek` | 1780 | 36 |  |
| printk.rs | `init_kmsg_device` | 1816 | 25 |  |
| printk.rs | `proc_kmsg_open` | 1841 | 8 |  |
| printk.rs | `proc_kmsg_close` | 1849 | 25 |  |
| printk.rs | `proc_kmsg_read` | 1874 | 67 |  |
| printk.rs | `kmsg_poll_sleep` | 1941 | 48 |  |
| printk.rs | `proc_kmsg_poll` | 1989 | 16 |  |
| printk.rs | `proc_kmsg_lseek` | 2005 | 96 |  |
| printk.rs | `enabled` | 2101 | 10 | yes |
| printk.rs | `log` | 2111 | 13 |  |
| printk.rs | `flush` | 2124 | 7 | yes |
| printk.rs | `init_logger` | 2131 | 86 |  |
| printk.rs | `push` | 2217 | 5 | yes |
| printk.rs | `push_dec` | 2222 | 20 |  |
| printk.rs | `is_running` | 2242 | 9 | yes |
| printk.rs | `init` | 2251 | 27 | yes |
| printk.rs | `resolve_file` | 2278 | 80 |  |
| printk.rs | `flush_once` | 2358 | 104 |  |
| printk.rs | `persistent_log_init` | 2462 | 5 |  |
| printk.rs | `persistent_log_running` | 2467 | 3 |  |
| sbi.rs | `sbi_system_reset` | 61 | 36 |  |
| sbi.rs | `send_ipi` | 97 | 38 | yes |
| signal.rs | `new` | 129 | 4 | yes |
| signal.rs | `bits` | 133 | 44 | yes |
| signal.rs | `new` | 177 | 10 | yes |
| signal.rs | `ignore` | 187 | 10 |  |
| signal.rs | `handler` | 197 | 10 | yes |
| signal.rs | `default_handler` | 207 | 5 |  |
| signal.rs | `ignore_handler` | 212 | 5 |  |
| signal.rs | `action` | 217 | 11 |  |
| signal.rs | `has_handler` | 228 | 26 | yes |
| signal.rs | `new` | 254 | 7 | yes |
| signal.rs | `is_empty` | 261 | 5 | yes |
| signal.rs | `enqueue` | 266 | 5 | yes |
| signal.rs | `dequeue` | 271 | 5 | yes |
| signal.rs | `peek` | 276 | 7 |  |
| signal.rs | `new` | 283 | 8 | yes |
| signal.rs | `add` | 291 | 22 | yes |
| signal.rs | `add_info` | 313 | 30 |  |
| signal.rs | `peek_info` | 343 | 10 | yes |
| signal.rs | `rt_queued_count` | 353 | 6 |  |
| signal.rs | `remove` | 359 | 21 | yes |
| signal.rs | `has` | 380 | 9 |  |
| signal.rs | `first` | 389 | 11 |  |
| signal.rs | `first_unmasked` | 400 | 11 |  |
| signal.rs | `first_info` | 411 | 5 |  |
| signal.rs | `clear` | 416 | 7 | yes |
| signal.rs | `get_all` | 423 | 8 | yes |
| signal.rs | `remove_one` | 431 | 47 |  |
| signal.rs | `new` | 478 | 22 | yes |
| signal.rs | `set_action` | 500 | 23 |  |
| signal.rs | `reset_all_handlers_default` | 523 | 9 |  |
| signal.rs | `get_action` | 532 | 11 | yes |
| signal.rs | `ign_cgt_masks` | 543 | 15 |  |
| signal.rs | `add_mask` | 558 | 9 |  |
| signal.rs | `remove_mask` | 567 | 9 |  |
| signal.rs | `is_masked` | 576 | 14 |  |
| signal.rs | `flush_handlers` | 590 | 17 |  |
| signal.rs | `clone` | 607 | 61 | yes |
| signal.rs | `new` | 668 | 15 | yes |
| signal.rs | `set_value_bytes` | 683 | 17 |  |
| signal.rs | `fault` | 700 | 8 | yes |
| signal.rs | `child` | 708 | 18 |  |
| signal.rs | `child_exit` | 726 | 97 | yes |
| signal.rs | `default` | 823 | 7 | yes |
| signal.rs | `new` | 830 | 34 | yes |
| signal.rs | `default` | 864 | 49 | yes |
| signal.rs | `zeroed` | 913 | 31 | yes |
| signal.rs | `default` | 944 | 14 | yes |
| signal.rs | `new` | 958 | 7 | yes |
| signal.rs | `set_fpsimd` | 965 | 12 |  |
| signal.rs | `get_fpsimd` | 977 | 40 |  |
| signal.rs | `new` | 1017 | 28 | yes |
| signal.rs | `size` | 1045 | 49 | yes |
| signal.rs | `new` | 1094 | 28 | yes |
| signal.rs | `new` | 1122 | 9 | yes |
| signal.rs | `is_disabled` | 1131 | 5 | yes |
| signal.rs | `is_on_stack` | 1136 | 56 |  |
| signal.rs | `size` | 1192 | 37 | yes |
| signal.rs | `zeroed` | 1229 | 38 | yes |
| signal.rs | `zeroed` | 1267 | 33 | yes |
| signal.rs | `size` | 1300 | 31 | yes |
| signal.rs | `do_signal` | 1331 | 291 |  |
| signal.rs | `restart_syscall_no_handler` | 1622 | 111 |  |
| signal.rs | `setup_frame` | 1733 | 34 |  |
| signal.rs | `setup_frame_aarch64` | 1767 | 197 |  |
| signal.rs | `setup_frame_riscv64` | 1964 | 204 |  |
| signal.rs | `restore_sigcontext` | 2168 | 31 |  |
| signal.rs | `restore_sigcontext_aarch64` | 2199 | 110 |  |
| signal.rs | `restore_sigcontext_riscv64` | 2309 | 108 |  |
| signal.rs | `setup_frame_x86_64` | 2417 | 254 |  |
| signal.rs | `restore_sigcontext_x86_64` | 2671 | 74 |  |
| signal.rs | `handle_default_signal` | 2745 | 170 |  |
| signal.rs | `send_signal_from_user` | 2915 | 37 |  |
| signal.rs | `abort_reason_dump` | 2952 | 110 |  |
| signal.rs | `send_signal_from_user_tkill` | 3062 | 111 |  |
| signal.rs | `force_unblock_current` | 3173 | 9 | yes |
| signal.rs | `send_signal` | 3182 | 8 | yes |
| signal.rs | `send_signal_directed` | 3190 | 4 | yes |
| signal.rs | `send_signal_g` | 3194 | 69 |  |
| signal.rs | `send_signal_locked` | 3263 | 25 |  |
| signal.rs | `send_signal_locked_info` | 3288 | 349 |  |
| signal.rs | `send_signal_info` | 3637 | 19 |  |
| signal.rs | `with_sigval` | 3656 | 9 |  |
| signal.rs | `set_band_fd` | 3665 | 16 |  |
| signal.rs | `send_signal_with_info` | 3681 | 26 | yes |
| signal.rs | `send_signal_with_info_directed` | 3707 | 4 | yes |
| signal.rs | `send_signal_with_info_g` | 3711 | 22 |  |
| signal.rs | `send_signal_to_pgid` | 3733 | 12 |  |
| signal.rs | `park_stopped_task` | 3745 | 35 |  |
| signal.rs | `is_unkillable_init` | 3780 | 94 |  |
| signal.rs | `fatal_signal_pending` | 3874 | 27 |  |
| signal.rs | `signal_pending` | 3901 | 45 |  |
| signal.rs | `signal_wake_up_state` | 3946 | 60 |  |
| signal.rs | `signal_wake_up` | 4006 | 4 | yes |
| timer.rs | `get_jiffies` | 67 | 9 | yes |
| timer.rs | `test_advance_jiffies` | 76 | 6 | yes |
| timer.rs | `jiffies_to_msecs` | 82 | 6 | yes |
| timer.rs | `msecs_to_jiffies` | 88 | 12 | yes |
| timer.rs | `jiffy_floor_of_time` | 100 | 15 | yes |
| timer.rs | `jiffies_update_from_timebase` | 115 | 23 | yes |
| timer.rs | `jiffies_bump` | 138 | 10 | yes |
| timer.rs | `read_time` | 148 | 7 | yes |
| timer.rs | `monotonic_ns_since_boot` | 155 | 8 | yes |
| timer.rs | `coarse_ns_since_boot` | 163 | 15 | yes |
| timer.rs | `register_input_debug_tick` | 178 | 6 | yes |
| timer.rs | `input_debug_tick` | 184 | 37 | yes |
| timer.rs | `timer_softirq_work_pending` | 221 | 10 | yes |
| timer.rs | `mark_timer_armed` | 231 | 24 | yes |
| timer.rs | `new` | 255 | 15 | yes |
| timer.rs | `refresh_id` | 270 | 8 | yes |
| timer.rs | `drop` | 278 | 12 | yes |
| timer.rs | `hres_next_deadline` | 290 | 9 | yes |
| timer.rs | `hres_horizon_for` | 299 | 40 | yes |
| timer.rs | `snapshot_timers` | 339 | 23 | yes |
| timer.rs | `add_timer_wakeup` | 362 | 34 | yes |
| timer.rs | `add_timer_wakeup_hres` | 396 | 40 | yes |
| timer.rs | `add_timer_with_action` | 436 | 42 | yes |
| timer.rs | `add_timer_with_action_hres` | 478 | 45 | yes |
| timer.rs | `non_zero_interval` | 523 | 13 | yes |
| timer.rs | `del_timer` | 536 | 28 | yes |
| timer.rs | `del_note` | 564 | 24 | yes |
| timer.rs | `del_lookup` | 588 | 13 | yes |
| timer.rs | `timer_pending` | 601 | 10 | yes |
| timer.rs | `get_timer_state` | 611 | 13 | yes |
| timer.rs | `timer_softirq_handler` | 624 | 476 | yes |
| timer.rs | `expire_hres_inline` | 1100 | 79 | yes |
| timer.rs | `run_wheel_sweep` | 1179 | 50 | yes |
| timer.rs | `softirq_diagnostics` | 1229 | 101 | yes |
| timer.rs | `deliver_fired` | 1330 | 33 | yes |
| timer.rs | `ph` | 1363 | 302 | yes |
| timer.rs | `fw_record` | 1665 | 45 | yes |
| timer.rs | `nsleep_iter_bump` | 1710 | 21 | yes |
| timer.rs | `nsleep_iter_of` | 1731 | 11 | yes |
| timer.rs | `ww_note_rearm` | 1742 | 14 | yes |
| timer.rs | `ww_note_tick_arrival` | 1756 | 7 | yes |
| timer.rs | `read_time_stamp` | 1763 | 6 | yes |
| timer.rs | `ww_tick_age_ms` | 1769 | 13 | yes |
| timer.rs | `ww_hres_age_ms` | 1782 | 19 | yes |
| timer.rs | `ww_heartbeat` | 1801 | 68 | yes |
| timer.rs | `pdec9` | 1869 | 30 | yes |
| timer.rs | `pdec` | 1899 | 176 | yes |
| timer.rs | `ww_latefire_budget_ok` | 2075 | 38 | yes |
| timer.rs | `tick_trap_report` | 2113 | 126 | yes |
| timer.rs | `tick_trap_report` | 2239 | 12 | yes |
| timer.rs | `wall_epoch_offset_secs` | 2251 | 5 | yes |
| timer.rs | `wall_epoch_offset_ns` | 2256 | 5 | yes |
| timer.rs | `set_wall_epoch_offset_secs` | 2261 | 5 | yes |
| timer.rs | `set_wall_epoch_offset_ns` | 2266 | 13 | yes |
| timer.rs | `realtime_secs_nanos` | 2279 | 8 | yes |
| timer.rs | `monotonic_time` | 2287 | 5 | yes |
