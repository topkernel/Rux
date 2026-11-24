# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# (root) — hot 5455 lines (stay) / cold 9850 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| boot_edges.rs | `image_end_va` | 21 | 13 |  |
| boot_edges.rs | `set_va_pa_offset` | 33 | 13 |  |
| boot_edges.rs | `zeropage_initrd_hint` | 45 | 28 |  |
| boot_edges.rs | `regions_from_arch_boot` | 72 | 29 |  |
| boot_edges.rs | `panic_hex` | 100 | 22 |  |
| boot_edges.rs | `rawframes_line` | 121 | 48 |  |
| boot_edges.rs | `x86_panic_reset` | 168 | 32 | yes |
| boot_edges.rs | `x86_reboot_pulse` | 199 | 14 | yes |
| boot_edges.rs | `sbi_shutdown_ecall` | 212 | 14 | yes |
| cmdline.rs | `fdt_u8` | 85 | 9 |  |
| cmdline.rs | `fdt_add` | 93 | 9 |  |
| cmdline.rs | `fdt_u32` | 101 | 7 |  |
| cmdline.rs | `parse_bootargs` | 107 | 144 |  |
| cmdline.rs | `parse_memory_regions` | 250 | 160 |  |
| cmdline.rs | `parse_cpu_isa_has_v` | 409 | 91 |  |
| cmdline.rs | `parse_initrd_region` | 499 | 11 |  |
| cmdline.rs | `parse_chosen_u64_pair` | 509 | 12 |  |
| cmdline.rs | `parse_chosen_prop` | 520 | 101 |  |
| cmdline.rs | `init` | 620 | 37 | yes |
| cmdline.rs | `init_from` | 656 | 15 |  |
| cmdline.rs | `install` | 670 | 43 | yes |
| cmdline.rs | `get_cmdline` | 712 | 32 | yes |
| cmdline.rs | `get_param` | 743 | 21 | yes |
| cmdline.rs | `parse_ipv4_addr` | 763 | 32 |  |
| cmdline.rs | `has_param` | 794 | 20 |  |
| cmdline.rs | `get_all_params` | 813 | 24 |  |
| cmdline.rs | `get_root_device` | 836 | 14 |  |
| cmdline.rs | `get_init_program` | 849 | 10 |  |
| cmdline.rs | `absolutize_init_path` | 858 | 13 |  |
| cmdline.rs | `get_rdinit_program` | 870 | 9 |  |
| cmdline.rs | `get_root_fs_type` | 878 | 9 |  |
| cmdline.rs | `get_root_flags` | 886 | 13 |  |
| cmdline.rs | `is_root_readonly` | 898 | 7 |  |
| cmdline.rs | `is_debug_mode` | 904 | 11 |  |
| cmdline.rs | `get_console_device` | 914 | 14 |  |
| cmdline.rs | `get_console_devices` | 927 | 24 |  |
| console.rs | `report_tx_stall_recovery` | 111 | 87 |  |
| console.rs | `get_uart_base` | 197 | 12 |  |
| console.rs | `new` | 208 | 7 | yes |
| console.rs | `putc` | 214 | 129 |  |
| console.rs | `puts_batch` | 342 | 131 | yes |
| console.rs | `early_init` | 472 | 35 |  |
| console.rs | `early_init` | 506 | 18 |  |
| console.rs | `early_init` | 523 | 4 |  |
| console.rs | `init` | 526 | 9 | yes |
| console.rs | `init_irq` | 534 | 50 |  |
| console.rs | `init_irq` | 583 | 2 |  |
| console.rs | `uart4_irq_line` | 584 | 29 |  |
| console.rs | `init_irq` | 612 | 8 |  |
| console.rs | `write_reg` | 619 | 7 |  |
| console.rs | `read_reg` | 625 | 6 |  |
| console.rs | `write_reg` | 630 | 8 |  |
| console.rs | `read_reg` | 637 | 21 |  |
| console.rs | `uart_rx_debug_magic` | 657 | 60 |  |
| console.rs | `uart_irq_handler` | 716 | 60 |  |
| console.rs | `tty_isig_record` | 775 | 25 |  |
| console.rs | `tty_isig_deliver_pending` | 799 | 13 |  |
| console.rs | `tty_isig_deliver` | 811 | 28 |  |
| console.rs | `putchar` | 838 | 10 | yes |
| console.rs | `puts` | 847 | 14 |  |
| console.rs | `puts_bytes` | 860 | 7 | yes |
| console.rs | `lock` | 866 | 13 | yes |
| console.rs | `putchar_no_lock` | 878 | 7 | yes |
| console.rs | `putchar_no_lock` | 884 | 8 | yes |
| console.rs | `uart_putc_raw` | 891 | 8 | yes |
| console.rs | `puts_no_lock` | 898 | 7 |  |
| console.rs | `puts_no_lock` | 904 | 6 |  |
| console.rs | `puts_bytes_no_lock` | 909 | 7 |  |
| console.rs | `puts_bytes_no_lock` | 915 | 10 |  |
| console.rs | `uart_data_ready` | 924 | 7 | yes |
| console.rs | `uart_data_ready` | 930 | 25 | yes |
| console.rs | `uart_rx_bridge_to_tty` | 954 | 14 | yes |
| console.rs | `getchar` | 967 | 84 | yes |
| console.rs | `read_waitq` | 1050 | 8 | yes |
| console.rs | `uart_has_data` | 1057 | 22 | yes |
| console.rs | `uart_has_data` | 1078 | 14 | yes |
| console.rs | `uart_has_data` | 1091 | 7 | yes |
| console.rs | `pl011_rx_pending` | 1097 | 13 |  |
| console.rs | `init_irq` | 1109 | 2 |  |
| console.rs | `pl011_irq_handler` | 1110 | 74 |  |
| console.rs | `uart_has_data` | 1183 | 12 | yes |
| console.rs | `process_input` | 1194 | 66 |  |
| console.rs | `write_str` | 1259 | 62 |  |
| console.rs | `set_framebuffer_info` | 1320 | 6 |  |
| console.rs | `get_framebuffer_info` | 1325 | 4 | yes |
| cov.rs | `cov_line` | 36 | 15 |  |
| cov.rs | `trigger_write` | 50 | 9 | yes |
| cov.rs | `trigger_generate` | 58 | 13 | yes |
| cov.rs | `capture_profile` | 70 | 16 |  |
| cov.rs | `dump_to_console` | 85 | 21 |  |
| cov.rs | `console_dump_active` | 105 | 5 |  |
| cov.rs | `dump_inner` | 109 | 22 |  |
| errno.rs | `as_i32` | 151 | 7 |  |
| errno.rs | `as_neg_i32` | 157 | 14 | yes |
| errno.rs | `from_neg_i32` | 170 | 49 | yes |
| errno.rs | `as_neg_u64` | 218 | 155 |  |
| errno.rs | `from` | 372 | 5 | yes |
| init.rs | `init_task_storage_addr` | 32 | 29 | yes |
| init.rs | `init` | 60 | 41 | yes |
| init.rs | `init_candidates` | 100 | 46 |  |
| init.rs | `load_init_program` | 145 | 38 |  |
| init.rs | `create_and_start_init_process` | 182 | 106 |  |
| init.rs | `load_and_setup_elf` | 287 | 150 |  |
| init.rs | `ret_from_exception` | 436 | 3 |  |
| init.rs | `ret_from_fork` | 438 | 5 | yes |
| init.rs | `ret_from_fork` | 442 | 24 | yes |
| init.rs | `init_std_fds_for_task` | 465 | 8 | yes |
| init.rs | `halt` | 472 | 6 |  |
| initrd.rs | `set_region` | 50 | 7 |  |
| initrd.rs | `region` | 56 | 12 |  |
| initrd.rs | `loaded` | 67 | 23 |  |
| initrd.rs | `load` | 89 | 40 | yes |
| initrd.rs | `as_slice` | 128 | 22 | yes |
| initrd.rs | `new` | 149 | 13 | yes |
| initrd.rs | `bits` | 161 | 17 | yes |
| initrd.rs | `align_byte` | 177 | 18 |  |
| initrd.rs | `new` | 194 | 30 | yes |
| initrd.rs | `decode` | 223 | 47 |  |
| initrd.rs | `inflate_raw` | 269 | 51 |  |
| initrd.rs | `fixed_litlen_table` | 319 | 15 |  |
| initrd.rs | `fixed_dist_table` | 333 | 7 |  |
| initrd.rs | `read_dynamic_tables` | 339 | 67 |  |
| initrd.rs | `inflate_block` | 405 | 48 |  |
| initrd.rs | `crc32` | 452 | 20 |  |
| initrd.rs | `gunzip` | 471 | 86 |  |
| initrd.rs | `parse_hex8` | 556 | 19 |  |
| initrd.rs | `align4` | 574 | 14 |  |
| initrd.rs | `unpack_newc` | 587 | 145 |  |
| initrd.rs | `mkdir_p` | 731 | 11 |  |
| initrd.rs | `mkdir_p_parents` | 741 | 17 |  |
| initrd.rs | `create_file` | 757 | 13 | yes |
| initrd.rs | `set_node_meta` | 769 | 6 |  |
| lib.rs | `print_status` | 35 | 11 | yes |
| lib.rs | `print_status_ex` | 45 | 14 | yes |
| lib.rs | `push` | 58 | 131 | yes |
| lib.rs | `alloc_error_handler` | 188 | 85 |  |
| lib.rs | `format_hex` | 272 | 9 |  |
| lib.rs | `rust_main_tail` | 280 | 1160 |  |
| lib.rs | `x86_boot_memory_regions` | 1439 | 9 |  |
| lib.rs | `x86_dump_regions` | 1447 | 398 |  |
| lib.rs | `mount_root_filesystem` | 1844 | 54 |  |
| lib.rs | `mount_boot_disk_ext4` | 1897 | 17 |  |
| lib.rs | `mount_ext4_root_from` | 1913 | 44 |  |
| lib.rs | `panic` | 1956 | 71 |  |
| list.rs | `new` | 38 | 13 | yes |
| list.rs | `init` | 50 | 9 | yes |
| list.rs | `is_empty` | 58 | 14 | yes |
| list.rs | `add` | 71 | 20 | yes |
| list.rs | `add_tail` | 90 | 17 |  |
| list.rs | `del` | 106 | 33 |  |
| list.rs | `entry` | 138 | 17 | yes |
| list.rs | `for_each` | 154 | 39 |  |
| list.rs | `first_entry` | 192 | 11 |  |
| list.rs | `offset` | 202 | 9 | yes |
| list.rs | `offset` | 210 | 18 | yes |
| panic_policy.rs | `parse` | 50 | 17 | yes |
| panic_policy.rs | `current` | 66 | 10 | yes |
| panic_policy.rs | `apply` | 75 | 25 | yes |
| print.rs | `write_str` | 22 | 75 |  |
| printk.rs | `empty` | 86 | 40 | yes |
| printk.rs | `ring_rec` | 125 | 14 |  |
| printk.rs | `ring_rec_mut` | 138 | 11 |  |
| printk.rs | `rec_text_len` | 148 | 50 |  |
| printk.rs | `try_new` | 197 | 15 |  |
| printk.rs | `drop` | 211 | 10 | yes |
| printk.rs | `set_console_loglevel` | 220 | 6 | yes |
| printk.rs | `get_console_loglevel` | 225 | 7 | yes |
| printk.rs | `init` | 231 | 14 | yes |
| printk.rs | `write_str` | 244 | 23 |  |
| printk.rs | `printk` | 266 | 17 |  |
| printk.rs | `printk_ring_always` | 282 | 15 |  |
| printk.rs | `printk_ring_always_ex` | 296 | 48 |  |
| printk.rs | `ring_stash_console` | 343 | 54 | yes |
| printk.rs | `console_emit_skipped` | 396 | 21 | yes |
| printk.rs | `acquire_emit_lock` | 416 | 41 |  |
| printk.rs | `emit_to_console` | 456 | 38 |  |
| printk.rs | `try_console_emit_lock` | 493 | 24 |  |
| printk.rs | `console_emit_unlock` | 516 | 18 |  |
| printk.rs | `try_console_emit_lock_ms` | 533 | 20 |  |
| printk.rs | `printk_ln` | 552 | 23 |  |
| printk.rs | `printk_bytes` | 574 | 46 |  |
| printk.rs | `printk_should_defer_output` | 619 | 29 |  |
| printk.rs | `console_emit_pending_flag` | 647 | 15 | yes |
| printk.rs | `console_drain_pending` | 661 | 43 | yes |
| printk.rs | `write_to_ring_buffer` | 703 | 76 |  |
| printk.rs | `sys_syslog` | 778 | 97 |  |
| printk.rs | `format_syslog_header` | 874 | 99 |  |
| printk.rs | `format_record_header` | 972 | 114 |  |
| printk.rs | `syslog_read_sequential` | 1085 | 80 |  |
| printk.rs | `syslog_read_all` | 1164 | 87 |  |
| printk.rs | `syslog_clear` | 1250 | 19 |  |
| printk.rs | `ring_tail` | 1268 | 91 | yes |
| printk.rs | `ring_tail_into` | 1358 | 2 |  |
| printk.rs | `push` | 1359 | 78 | yes |
| printk.rs | `generate_kmsg` | 1436 | 68 |  |
| printk.rs | `service_deferred_kmsg_wake` | 1503 | 31 | yes |
| printk.rs | `kmsg_file_read` | 1533 | 150 |  |
| printk.rs | `set_kmsg_console_mode` | 1682 | 10 | yes |
| printk.rs | `kmsg_console_mode` | 1691 | 6 | yes |
| printk.rs | `kmsg_console_mode_from_str` | 1696 | 13 | yes |
| printk.rs | `kmsg_console_allows_echo` | 1708 | 38 |  |
| printk.rs | `kmsg_file_write` | 1745 | 20 |  |
| printk.rs | `kmsg_file_poll` | 1764 | 17 |  |
| printk.rs | `kmsg_file_lseek` | 1780 | 37 |  |
| printk.rs | `init_kmsg_device` | 1816 | 25 |  |
| printk.rs | `proc_kmsg_open` | 1840 | 9 | yes |
| printk.rs | `proc_kmsg_close` | 1848 | 26 |  |
| printk.rs | `proc_kmsg_read` | 1873 | 68 |  |
| printk.rs | `kmsg_poll_sleep` | 1940 | 49 |  |
| printk.rs | `proc_kmsg_poll` | 1988 | 17 |  |
| printk.rs | `proc_kmsg_lseek` | 2004 | 97 |  |
| printk.rs | `enabled` | 2100 | 11 | yes |
| printk.rs | `log` | 2110 | 14 |  |
| printk.rs | `flush` | 2123 | 8 | yes |
| printk.rs | `init_logger` | 2130 | 87 |  |
| printk.rs | `push` | 2216 | 6 | yes |
| printk.rs | `push_dec` | 2221 | 21 |  |
| printk.rs | `is_running` | 2241 | 10 | yes |
| printk.rs | `init` | 2250 | 28 | yes |
| printk.rs | `resolve_file` | 2277 | 81 |  |
| printk.rs | `flush_once` | 2357 | 105 |  |
| printk.rs | `persistent_log_init` | 2461 | 6 |  |
| printk.rs | `persistent_log_running` | 2466 | 4 |  |
| sbi.rs | `sbi_system_reset` | 61 | 37 | yes |
| sbi.rs | `send_ipi` | 97 | 39 | yes |
| signal.rs | `new` | 129 | 5 | yes |
| signal.rs | `bits` | 133 | 45 | yes |
| signal.rs | `new` | 177 | 11 | yes |
| signal.rs | `ignore` | 187 | 11 |  |
| signal.rs | `handler` | 197 | 11 |  |
| signal.rs | `default_handler` | 207 | 6 |  |
| signal.rs | `ignore_handler` | 212 | 6 |  |
| signal.rs | `action` | 217 | 12 |  |
| signal.rs | `has_handler` | 228 | 27 |  |
| signal.rs | `new` | 254 | 8 | yes |
| signal.rs | `is_empty` | 261 | 6 | yes |
| signal.rs | `enqueue` | 266 | 6 | yes |
| signal.rs | `dequeue` | 271 | 6 | yes |
| signal.rs | `peek` | 276 | 8 | yes |
| signal.rs | `new` | 283 | 9 | yes |
| signal.rs | `add` | 291 | 23 | yes |
| signal.rs | `add_info` | 313 | 31 |  |
| signal.rs | `peek_info` | 343 | 11 |  |
| signal.rs | `rt_queued_count` | 353 | 7 |  |
| signal.rs | `remove` | 359 | 22 | yes |
| signal.rs | `has` | 380 | 10 | yes |
| signal.rs | `first` | 389 | 12 | yes |
| signal.rs | `first_unmasked` | 400 | 12 |  |
| signal.rs | `first_info` | 411 | 6 |  |
| signal.rs | `clear` | 416 | 8 | yes |
| signal.rs | `get_all` | 423 | 9 | yes |
| signal.rs | `remove_one` | 431 | 48 | yes |
| signal.rs | `new` | 478 | 23 | yes |
| signal.rs | `set_action` | 500 | 20 | yes |
| signal.rs | `reset_all_handlers_default` | 519 | 6 | yes |
| signal.rs | `get_action` | 524 | 13 | yes |
| signal.rs | `ign_cgt_masks` | 536 | 6 | yes |
| signal.rs | `add_mask` | 541 | 10 |  |
| signal.rs | `remove_mask` | 550 | 10 |  |
| signal.rs | `is_masked` | 559 | 16 |  |
| signal.rs | `flush_handlers` | 574 | 9 | yes |
| signal.rs | `with_actions_mut` | 582 | 8 |  |
| signal.rs | `clone` | 589 | 62 | yes |
| signal.rs | `new` | 650 | 16 | yes |
| signal.rs | `set_value_bytes` | 665 | 18 | yes |
| signal.rs | `fault` | 682 | 9 | yes |
| signal.rs | `child` | 690 | 19 |  |
| signal.rs | `child_exit` | 708 | 98 | yes |
| signal.rs | `default` | 805 | 8 | yes |
| signal.rs | `new` | 812 | 35 | yes |
| signal.rs | `default` | 846 | 50 | yes |
| signal.rs | `zeroed` | 895 | 32 |  |
| signal.rs | `default` | 926 | 15 | yes |
| signal.rs | `new` | 940 | 8 | yes |
| signal.rs | `set_fpsimd` | 947 | 13 |  |
| signal.rs | `get_fpsimd` | 959 | 41 |  |
| signal.rs | `new` | 999 | 29 | yes |
| signal.rs | `size` | 1027 | 50 | yes |
| signal.rs | `new` | 1076 | 29 | yes |
| signal.rs | `new` | 1104 | 10 | yes |
| signal.rs | `is_disabled` | 1113 | 6 |  |
| signal.rs | `is_on_stack` | 1118 | 57 | yes |
| signal.rs | `size` | 1174 | 38 | yes |
| signal.rs | `zeroed` | 1211 | 39 |  |
| signal.rs | `zeroed` | 1249 | 34 |  |
| signal.rs | `size` | 1282 | 32 | yes |
| signal.rs | `do_signal` | 1313 | 279 |  |
| signal.rs | `restart_syscall_no_handler` | 1591 | 112 |  |
| signal.rs | `setup_frame` | 1702 | 35 |  |
| signal.rs | `setup_frame_aarch64` | 1736 | 198 |  |
| signal.rs | `setup_frame_riscv64` | 1933 | 205 |  |
| signal.rs | `restore_sigcontext` | 2137 | 32 | yes |
| signal.rs | `restore_sigcontext_aarch64` | 2168 | 111 |  |
| signal.rs | `restore_sigcontext_riscv64` | 2278 | 109 |  |
| signal.rs | `setup_frame_x86_64` | 2386 | 255 |  |
| signal.rs | `restore_sigcontext_x86_64` | 2640 | 79 |  |
| signal.rs | `handle_default_signal` | 2718 | 167 |  |
| signal.rs | `send_signal_from_user` | 2884 | 17 | yes |
| signal.rs | `send_signal_from_user_tkill` | 2900 | 17 | yes |
| signal.rs | `force_unblock_current` | 2916 | 5 | yes |
| signal.rs | `send_signal` | 2920 | 9 | yes |
| signal.rs | `send_signal_directed` | 2928 | 5 | yes |
| signal.rs | `send_signal_g` | 2932 | 76 |  |
| signal.rs | `send_signal_info` | 3007 | 11 |  |
| signal.rs | `with_sigval` | 3017 | 10 | yes |
| signal.rs | `set_band_fd` | 3026 | 17 | yes |
| signal.rs | `send_signal_with_info` | 3042 | 27 | yes |
| signal.rs | `send_signal_with_info_directed` | 3068 | 5 | yes |
| signal.rs | `send_signal_with_info_g` | 3072 | 23 |  |
| signal.rs | `send_signal_to_pgid` | 3094 | 5 | yes |
| signal.rs | `park_stopped_task` | 3098 | 36 |  |
| signal.rs | `is_unkillable_init` | 3133 | 95 |  |
| signal.rs | `fatal_signal_pending` | 3227 | 28 | yes |
| signal.rs | `signal_pending` | 3254 | 46 | yes |
| signal.rs | `signal_wake_up_state` | 3299 | 61 |  |
| signal.rs | `signal_wake_up` | 3359 | 5 | yes |
| signal_edges.rs | `abort_reason_dump` | 18 | 102 |  |
| signal_hooks.rs | `no_policy_send_core` | 105 | 4 |  |
| signal_hooks.rs | `no_policy_send_from_user` | 108 | 4 |  |
| signal_hooks.rs | `no_policy_send_from_user_tkill` | 111 | 4 |  |
| signal_hooks.rs | `no_policy_send_info` | 114 | 4 |  |
| signal_hooks.rs | `no_policy_send_to_pgid` | 117 | 4 |  |
| signal_hooks.rs | `no_policy_force_unblock` | 120 | 4 |  |
| signal_hooks.rs | `no_policy_default_action_class` | 123 | 4 |  |
| signal_hooks.rs | `no_policy_init_filter` | 126 | 4 |  |
| signal_hooks.rs | `no_policy_flush_handlers` | 129 | 4 |  |
| signal_hooks.rs | `no_policy_reset_all` | 132 | 4 |  |
| signal_hooks.rs | `no_policy_ign_cgt_masks` | 135 | 15 |  |
| signal_hooks.rs | `signal_register_hooks` | 149 | 6 | yes |
| signal_hooks.rs | `hooks` | 154 | 11 |  |
| signal_hooks.rs | `send_signal_core` | 164 | 11 |  |
| signal_hooks.rs | `send_from_user` | 174 | 6 |  |
| signal_hooks.rs | `send_from_user_tkill` | 179 | 6 |  |
| signal_hooks.rs | `send_info` | 184 | 6 |  |
| signal_hooks.rs | `send_to_pgid` | 189 | 6 |  |
| signal_hooks.rs | `force_unblock_current` | 194 | 8 |  |
| signal_hooks.rs | `default_action_class` | 201 | 8 |  |
| signal_hooks.rs | `init_delivery_filter` | 208 | 6 |  |
| signal_hooks.rs | `flush_handlers` | 213 | 6 | yes |
| signal_hooks.rs | `reset_all_handlers_default` | 218 | 6 | yes |
| signal_hooks.rs | `ign_cgt_masks` | 223 | 4 | yes |
| signal_policy.rs | `send_signal_from_user` | 31 | 42 | yes |
| signal_policy.rs | `send_signal_from_user_tkill` | 72 | 112 | yes |
| signal_policy.rs | `force_unblock_current` | 183 | 26 |  |
| signal_policy.rs | `send_signal_core` | 208 | 350 |  |
| signal_policy.rs | `send_signal_info` | 557 | 18 |  |
| signal_policy.rs | `send_signal_to_pgid` | 574 | 23 | yes |
| signal_policy.rs | `default_action_class` | 596 | 34 |  |
| signal_policy.rs | `init_delivery_filter` | 629 | 35 |  |
| signal_policy.rs | `flush_handlers` | 663 | 25 | yes |
| signal_policy.rs | `reset_all_handlers_default` | 687 | 13 | yes |
| signal_policy.rs | `ign_cgt_masks` | 699 | 15 | yes |
| timer.rs | `get_jiffies` | 67 | 10 | yes |
| timer.rs | `test_advance_jiffies` | 76 | 7 | yes |
| timer.rs | `jiffies_to_msecs` | 82 | 7 | yes |
| timer.rs | `msecs_to_jiffies` | 88 | 13 | yes |
| timer.rs | `jiffy_floor_of_time` | 100 | 16 | yes |
| timer.rs | `jiffies_update_from_timebase` | 115 | 24 | yes |
| timer.rs | `jiffies_bump` | 138 | 11 | yes |
| timer.rs | `read_time` | 148 | 8 | yes |
| timer.rs | `monotonic_ns_since_boot` | 155 | 9 | yes |
| timer.rs | `coarse_ns_since_boot` | 163 | 16 | yes |
| timer.rs | `register_input_debug_tick` | 178 | 7 | yes |
| timer.rs | `input_debug_tick` | 184 | 38 | yes |
| timer.rs | `timer_softirq_work_pending` | 221 | 11 | yes |
| timer.rs | `mark_timer_armed` | 231 | 25 | yes |
| timer.rs | `new` | 255 | 16 | yes |
| timer.rs | `refresh_id` | 270 | 9 | yes |
| timer.rs | `drop` | 278 | 13 | yes |
| timer.rs | `hres_next_deadline` | 290 | 10 | yes |
| timer.rs | `hres_horizon_for` | 299 | 41 | yes |
| timer.rs | `snapshot_timers` | 339 | 24 | yes |
| timer.rs | `add_timer_wakeup` | 362 | 35 | yes |
| timer.rs | `add_timer_wakeup_hres` | 396 | 41 | yes |
| timer.rs | `add_timer_with_action` | 436 | 43 | yes |
| timer.rs | `add_timer_with_action_hres` | 478 | 46 | yes |
| timer.rs | `non_zero_interval` | 523 | 14 | yes |
| timer.rs | `del_timer` | 536 | 29 | yes |
| timer.rs | `del_note` | 564 | 25 | yes |
| timer.rs | `del_lookup` | 588 | 14 | yes |
| timer.rs | `timer_pending` | 601 | 11 | yes |
| timer.rs | `get_timer_state` | 611 | 14 | yes |
| timer.rs | `timer_softirq_handler` | 624 | 477 | yes |
| timer.rs | `expire_hres_inline` | 1100 | 80 | yes |
| timer.rs | `run_wheel_sweep` | 1179 | 51 | yes |
| timer.rs | `softirq_diagnostics` | 1229 | 102 | yes |
| timer.rs | `deliver_fired` | 1330 | 34 | yes |
| timer.rs | `ph` | 1363 | 303 | yes |
| timer.rs | `fw_record` | 1665 | 46 | yes |
| timer.rs | `nsleep_iter_bump` | 1710 | 22 | yes |
| timer.rs | `nsleep_iter_of` | 1731 | 12 | yes |
| timer.rs | `ww_note_rearm` | 1742 | 15 | yes |
| timer.rs | `ww_note_tick_arrival` | 1756 | 8 | yes |
| timer.rs | `read_time_stamp` | 1763 | 7 | yes |
| timer.rs | `ww_tick_age_ms` | 1769 | 14 | yes |
| timer.rs | `ww_hres_age_ms` | 1782 | 20 | yes |
| timer.rs | `ww_heartbeat` | 1801 | 69 | yes |
| timer.rs | `pdec9` | 1869 | 31 | yes |
| timer.rs | `pdec` | 1899 | 177 | yes |
| timer.rs | `ww_latefire_budget_ok` | 2075 | 39 | yes |
| timer.rs | `tick_trap_report` | 2113 | 127 | yes |
| timer.rs | `tick_trap_report` | 2239 | 13 | yes |
| timer.rs | `wall_epoch_offset_secs` | 2251 | 6 | yes |
| timer.rs | `wall_epoch_offset_ns` | 2256 | 6 | yes |
| timer.rs | `set_wall_epoch_offset_secs` | 2261 | 6 | yes |
| timer.rs | `set_wall_epoch_offset_ns` | 2266 | 14 | yes |
| timer.rs | `realtime_secs_nanos` | 2279 | 9 | yes |
| timer.rs | `monotonic_time` | 2287 | 6 | yes |
