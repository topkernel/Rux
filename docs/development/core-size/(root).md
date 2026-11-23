# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# (root) — hot 2738 lines (stay) / cold 12292 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| boot_edges.rs | `image_end_va` | 21 | 13 |  |
| boot_edges.rs | `set_va_pa_offset` | 33 | 13 |  |
| boot_edges.rs | `zeropage_initrd_hint` | 45 | 28 |  |
| boot_edges.rs | `regions_from_arch_boot` | 72 | 29 |  |
| boot_edges.rs | `panic_hex` | 100 | 22 |  |
| boot_edges.rs | `rawframes_line` | 121 | 48 |  |
| boot_edges.rs | `x86_panic_reset` | 168 | 32 |  |
| boot_edges.rs | `x86_reboot_pulse` | 199 | 14 |  |
| boot_edges.rs | `sbi_shutdown_ecall` | 212 | 14 |  |
| cmdline.rs | `fdt_u8` | 85 | 9 |  |
| cmdline.rs | `fdt_add` | 93 | 9 |  |
| cmdline.rs | `fdt_u32` | 101 | 7 |  |
| cmdline.rs | `parse_bootargs` | 107 | 144 |  |
| cmdline.rs | `parse_memory_regions` | 250 | 160 |  |
| cmdline.rs | `parse_cpu_isa_has_v` | 409 | 91 |  |
| cmdline.rs | `parse_initrd_region` | 499 | 11 |  |
| cmdline.rs | `parse_chosen_u64_pair` | 509 | 12 |  |
| cmdline.rs | `parse_chosen_prop` | 520 | 101 |  |
| cmdline.rs | `init` | 620 | 37 |  |
| cmdline.rs | `init_from` | 656 | 15 |  |
| cmdline.rs | `install` | 670 | 43 |  |
| cmdline.rs | `get_cmdline` | 712 | 32 |  |
| cmdline.rs | `get_param` | 743 | 21 |  |
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
| console.rs | `puts_batch` | 342 | 131 |  |
| console.rs | `early_init` | 472 | 35 |  |
| console.rs | `early_init` | 506 | 18 |  |
| console.rs | `early_init` | 523 | 4 |  |
| console.rs | `init` | 526 | 9 |  |
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
| console.rs | `puts_bytes` | 860 | 7 |  |
| console.rs | `lock` | 866 | 13 | yes |
| console.rs | `putchar_no_lock` | 878 | 7 |  |
| console.rs | `putchar_no_lock` | 884 | 8 |  |
| console.rs | `uart_putc_raw` | 891 | 8 |  |
| console.rs | `puts_no_lock` | 898 | 7 |  |
| console.rs | `puts_no_lock` | 904 | 6 |  |
| console.rs | `puts_bytes_no_lock` | 909 | 7 |  |
| console.rs | `puts_bytes_no_lock` | 915 | 10 |  |
| console.rs | `uart_data_ready` | 924 | 7 |  |
| console.rs | `uart_data_ready` | 930 | 25 |  |
| console.rs | `uart_rx_bridge_to_tty` | 954 | 14 |  |
| console.rs | `getchar` | 967 | 84 |  |
| console.rs | `read_waitq` | 1050 | 8 |  |
| console.rs | `uart_has_data` | 1057 | 22 |  |
| console.rs | `uart_has_data` | 1078 | 14 |  |
| console.rs | `uart_has_data` | 1091 | 7 |  |
| console.rs | `pl011_rx_pending` | 1097 | 13 |  |
| console.rs | `init_irq` | 1109 | 2 |  |
| console.rs | `pl011_irq_handler` | 1110 | 74 |  |
| console.rs | `uart_has_data` | 1183 | 12 |  |
| console.rs | `process_input` | 1194 | 66 |  |
| console.rs | `write_str` | 1259 | 62 |  |
| console.rs | `set_framebuffer_info` | 1320 | 6 |  |
| console.rs | `get_framebuffer_info` | 1325 | 4 |  |
| cov.rs | `cov_line` | 36 | 15 |  |
| cov.rs | `trigger_write` | 50 | 9 |  |
| cov.rs | `trigger_generate` | 58 | 13 |  |
| cov.rs | `capture_profile` | 70 | 16 |  |
| cov.rs | `dump_to_console` | 85 | 21 |  |
| cov.rs | `console_dump_active` | 105 | 5 |  |
| cov.rs | `dump_inner` | 109 | 22 |  |
| errno.rs | `as_i32` | 151 | 7 |  |
| errno.rs | `as_neg_i32` | 157 | 14 |  |
| errno.rs | `from_neg_i32` | 170 | 49 |  |
| errno.rs | `as_neg_u64` | 218 | 155 |  |
| errno.rs | `from` | 372 | 5 | yes |
| init.rs | `init_task_storage_addr` | 33 | 29 |  |
| init.rs | `init` | 61 | 41 |  |
| init.rs | `init_candidates` | 101 | 46 |  |
| init.rs | `load_init_program` | 146 | 38 |  |
| init.rs | `create_and_start_init_process` | 183 | 106 |  |
| init.rs | `load_and_setup_elf` | 288 | 150 |  |
| init.rs | `ret_from_exception` | 437 | 3 |  |
| init.rs | `ret_from_fork` | 439 | 5 |  |
| init.rs | `ret_from_fork` | 443 | 24 |  |
| init.rs | `init_std_fds_for_task` | 466 | 31 |  |
| init.rs | `halt` | 496 | 6 |  |
| initrd.rs | `set_region` | 50 | 7 |  |
| initrd.rs | `region` | 56 | 12 |  |
| initrd.rs | `loaded` | 67 | 23 |  |
| initrd.rs | `load` | 89 | 39 | yes |
| initrd.rs | `as_slice` | 127 | 22 |  |
| initrd.rs | `new` | 148 | 13 | yes |
| initrd.rs | `bits` | 160 | 17 | yes |
| initrd.rs | `align_byte` | 176 | 18 |  |
| initrd.rs | `new` | 193 | 30 | yes |
| initrd.rs | `decode` | 222 | 47 |  |
| initrd.rs | `inflate_raw` | 268 | 51 |  |
| initrd.rs | `fixed_litlen_table` | 318 | 15 |  |
| initrd.rs | `fixed_dist_table` | 332 | 7 |  |
| initrd.rs | `read_dynamic_tables` | 338 | 67 |  |
| initrd.rs | `inflate_block` | 404 | 48 |  |
| initrd.rs | `crc32` | 451 | 20 |  |
| initrd.rs | `gunzip` | 470 | 86 |  |
| initrd.rs | `parse_hex8` | 555 | 19 |  |
| initrd.rs | `align4` | 573 | 14 |  |
| initrd.rs | `unpack_newc` | 586 | 145 |  |
| initrd.rs | `mkdir_p` | 730 | 11 |  |
| initrd.rs | `mkdir_p_parents` | 740 | 17 |  |
| initrd.rs | `create_file` | 756 | 15 |  |
| initrd.rs | `set_node_meta` | 770 | 10 |  |
| lib.rs | `print_status` | 35 | 11 | yes |
| lib.rs | `print_status_ex` | 45 | 14 |  |
| lib.rs | `push` | 58 | 123 |  |
| lib.rs | `alloc_error_handler` | 180 | 85 |  |
| lib.rs | `format_hex` | 264 | 9 |  |
| lib.rs | `rust_main_tail` | 272 | 1155 |  |
| lib.rs | `x86_boot_memory_regions` | 1426 | 9 |  |
| lib.rs | `x86_dump_regions` | 1434 | 398 |  |
| lib.rs | `mount_root_filesystem` | 1831 | 54 |  |
| lib.rs | `mount_boot_disk_ext4` | 1884 | 17 |  |
| lib.rs | `mount_ext4_root_from` | 1900 | 44 |  |
| lib.rs | `panic` | 1943 | 71 |  |
| list.rs | `new` | 38 | 13 | yes |
| list.rs | `init` | 50 | 9 |  |
| list.rs | `is_empty` | 58 | 14 |  |
| list.rs | `add` | 71 | 20 |  |
| list.rs | `add_tail` | 90 | 17 |  |
| list.rs | `del` | 106 | 33 |  |
| list.rs | `entry` | 138 | 17 |  |
| list.rs | `for_each` | 154 | 39 |  |
| list.rs | `first_entry` | 192 | 11 |  |
| list.rs | `offset` | 202 | 9 |  |
| list.rs | `offset` | 210 | 18 |  |
| panic_policy.rs | `parse` | 50 | 17 |  |
| panic_policy.rs | `current` | 66 | 10 |  |
| panic_policy.rs | `apply` | 75 | 25 |  |
| print.rs | `write_str` | 22 | 75 |  |
| printk.rs | `empty` | 86 | 40 |  |
| printk.rs | `ring_rec` | 125 | 14 |  |
| printk.rs | `ring_rec_mut` | 138 | 11 |  |
| printk.rs | `rec_text_len` | 148 | 50 |  |
| printk.rs | `try_new` | 197 | 15 |  |
| printk.rs | `drop` | 211 | 10 | yes |
| printk.rs | `set_console_loglevel` | 220 | 6 |  |
| printk.rs | `get_console_loglevel` | 225 | 7 |  |
| printk.rs | `init` | 231 | 14 |  |
| printk.rs | `write_str` | 244 | 23 |  |
| printk.rs | `printk` | 266 | 17 |  |
| printk.rs | `printk_ring_always` | 282 | 15 |  |
| printk.rs | `printk_ring_always_ex` | 296 | 48 |  |
| printk.rs | `ring_stash_console` | 343 | 54 |  |
| printk.rs | `console_emit_skipped` | 396 | 21 |  |
| printk.rs | `acquire_emit_lock` | 416 | 41 |  |
| printk.rs | `emit_to_console` | 456 | 38 |  |
| printk.rs | `try_console_emit_lock` | 493 | 24 |  |
| printk.rs | `console_emit_unlock` | 516 | 18 |  |
| printk.rs | `try_console_emit_lock_ms` | 533 | 20 |  |
| printk.rs | `printk_ln` | 552 | 23 |  |
| printk.rs | `printk_bytes` | 574 | 46 |  |
| printk.rs | `printk_should_defer_output` | 619 | 29 |  |
| printk.rs | `console_emit_pending_flag` | 647 | 15 |  |
| printk.rs | `console_drain_pending` | 661 | 43 |  |
| printk.rs | `write_to_ring_buffer` | 703 | 76 |  |
| printk.rs | `sys_syslog` | 778 | 97 |  |
| printk.rs | `format_syslog_header` | 874 | 99 |  |
| printk.rs | `format_record_header` | 972 | 114 |  |
| printk.rs | `syslog_read_sequential` | 1085 | 80 |  |
| printk.rs | `syslog_read_all` | 1164 | 87 |  |
| printk.rs | `syslog_clear` | 1250 | 19 |  |
| printk.rs | `ring_tail` | 1268 | 91 |  |
| printk.rs | `ring_tail_into` | 1358 | 2 |  |
| printk.rs | `push` | 1359 | 78 |  |
| printk.rs | `generate_kmsg` | 1436 | 68 |  |
| printk.rs | `service_deferred_kmsg_wake` | 1503 | 31 |  |
| printk.rs | `kmsg_file_read` | 1533 | 150 |  |
| printk.rs | `set_kmsg_console_mode` | 1682 | 10 |  |
| printk.rs | `kmsg_console_mode` | 1691 | 6 |  |
| printk.rs | `kmsg_console_mode_from_str` | 1696 | 13 |  |
| printk.rs | `kmsg_console_allows_echo` | 1708 | 38 |  |
| printk.rs | `kmsg_file_write` | 1745 | 20 |  |
| printk.rs | `kmsg_file_poll` | 1764 | 17 |  |
| printk.rs | `kmsg_file_lseek` | 1780 | 37 |  |
| printk.rs | `init_kmsg_device` | 1816 | 26 |  |
| printk.rs | `proc_kmsg_open` | 1841 | 9 |  |
| printk.rs | `proc_kmsg_close` | 1849 | 26 |  |
| printk.rs | `proc_kmsg_read` | 1874 | 68 |  |
| printk.rs | `kmsg_poll_sleep` | 1941 | 49 |  |
| printk.rs | `proc_kmsg_poll` | 1989 | 17 |  |
| printk.rs | `proc_kmsg_lseek` | 2005 | 97 |  |
| printk.rs | `enabled` | 2101 | 11 |  |
| printk.rs | `log` | 2111 | 14 |  |
| printk.rs | `flush` | 2124 | 8 |  |
| printk.rs | `init_logger` | 2131 | 87 |  |
| printk.rs | `push` | 2217 | 6 |  |
| printk.rs | `push_dec` | 2222 | 21 |  |
| printk.rs | `is_running` | 2242 | 10 |  |
| printk.rs | `init` | 2251 | 28 |  |
| printk.rs | `resolve_file` | 2278 | 81 |  |
| printk.rs | `flush_once` | 2358 | 105 |  |
| printk.rs | `persistent_log_init` | 2462 | 6 |  |
| printk.rs | `persistent_log_running` | 2467 | 4 |  |
| sbi.rs | `sbi_system_reset` | 61 | 37 |  |
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
| signal.rs | `is_empty` | 261 | 6 |  |
| signal.rs | `enqueue` | 266 | 6 |  |
| signal.rs | `dequeue` | 271 | 6 |  |
| signal.rs | `peek` | 276 | 8 |  |
| signal.rs | `new` | 283 | 9 | yes |
| signal.rs | `add` | 291 | 23 |  |
| signal.rs | `add_info` | 313 | 31 |  |
| signal.rs | `peek_info` | 343 | 11 |  |
| signal.rs | `rt_queued_count` | 353 | 7 |  |
| signal.rs | `remove` | 359 | 22 | yes |
| signal.rs | `has` | 380 | 10 |  |
| signal.rs | `first` | 389 | 12 |  |
| signal.rs | `first_unmasked` | 400 | 12 |  |
| signal.rs | `first_info` | 411 | 6 |  |
| signal.rs | `clear` | 416 | 8 |  |
| signal.rs | `get_all` | 423 | 9 |  |
| signal.rs | `remove_one` | 431 | 48 |  |
| signal.rs | `new` | 478 | 23 | yes |
| signal.rs | `set_action` | 500 | 24 |  |
| signal.rs | `reset_all_handlers_default` | 523 | 10 |  |
| signal.rs | `get_action` | 532 | 12 |  |
| signal.rs | `ign_cgt_masks` | 543 | 16 |  |
| signal.rs | `add_mask` | 558 | 10 |  |
| signal.rs | `remove_mask` | 567 | 10 |  |
| signal.rs | `is_masked` | 576 | 15 |  |
| signal.rs | `flush_handlers` | 590 | 18 |  |
| signal.rs | `clone` | 607 | 62 |  |
| signal.rs | `new` | 668 | 16 | yes |
| signal.rs | `set_value_bytes` | 683 | 18 |  |
| signal.rs | `fault` | 700 | 9 |  |
| signal.rs | `child` | 708 | 19 |  |
| signal.rs | `child_exit` | 726 | 98 |  |
| signal.rs | `default` | 823 | 8 |  |
| signal.rs | `new` | 830 | 35 | yes |
| signal.rs | `default` | 864 | 50 |  |
| signal.rs | `zeroed` | 913 | 32 |  |
| signal.rs | `default` | 944 | 15 |  |
| signal.rs | `new` | 958 | 8 | yes |
| signal.rs | `set_fpsimd` | 965 | 13 |  |
| signal.rs | `get_fpsimd` | 977 | 41 |  |
| signal.rs | `new` | 1017 | 29 | yes |
| signal.rs | `size` | 1045 | 50 |  |
| signal.rs | `new` | 1094 | 29 | yes |
| signal.rs | `new` | 1122 | 10 | yes |
| signal.rs | `is_disabled` | 1131 | 6 |  |
| signal.rs | `is_on_stack` | 1136 | 57 |  |
| signal.rs | `size` | 1192 | 38 |  |
| signal.rs | `zeroed` | 1229 | 39 |  |
| signal.rs | `zeroed` | 1267 | 34 |  |
| signal.rs | `size` | 1300 | 32 |  |
| signal.rs | `do_signal` | 1331 | 292 |  |
| signal.rs | `restart_syscall_no_handler` | 1622 | 112 |  |
| signal.rs | `setup_frame` | 1733 | 35 |  |
| signal.rs | `setup_frame_aarch64` | 1767 | 198 |  |
| signal.rs | `setup_frame_riscv64` | 1964 | 205 |  |
| signal.rs | `restore_sigcontext` | 2168 | 32 |  |
| signal.rs | `restore_sigcontext_aarch64` | 2199 | 111 |  |
| signal.rs | `restore_sigcontext_riscv64` | 2309 | 109 |  |
| signal.rs | `setup_frame_x86_64` | 2417 | 255 |  |
| signal.rs | `restore_sigcontext_x86_64` | 2671 | 75 |  |
| signal.rs | `handle_default_signal` | 2745 | 171 |  |
| signal.rs | `send_signal_from_user` | 2915 | 38 |  |
| signal.rs | `abort_reason_dump` | 2952 | 111 |  |
| signal.rs | `send_signal_from_user_tkill` | 3062 | 112 |  |
| signal.rs | `force_unblock_current` | 3173 | 10 |  |
| signal.rs | `send_signal` | 3182 | 9 |  |
| signal.rs | `send_signal_directed` | 3190 | 5 |  |
| signal.rs | `send_signal_g` | 3194 | 70 |  |
| signal.rs | `send_signal_locked` | 3263 | 26 |  |
| signal.rs | `send_signal_locked_info` | 3288 | 350 |  |
| signal.rs | `send_signal_info` | 3637 | 20 |  |
| signal.rs | `with_sigval` | 3656 | 10 |  |
| signal.rs | `set_band_fd` | 3665 | 17 |  |
| signal.rs | `send_signal_with_info` | 3681 | 27 |  |
| signal.rs | `send_signal_with_info_directed` | 3707 | 5 |  |
| signal.rs | `send_signal_with_info_g` | 3711 | 23 |  |
| signal.rs | `send_signal_to_pgid` | 3733 | 13 |  |
| signal.rs | `park_stopped_task` | 3745 | 36 |  |
| signal.rs | `is_unkillable_init` | 3780 | 95 |  |
| signal.rs | `fatal_signal_pending` | 3874 | 28 |  |
| signal.rs | `signal_pending` | 3901 | 46 |  |
| signal.rs | `signal_wake_up_state` | 3946 | 61 |  |
| signal.rs | `signal_wake_up` | 4006 | 5 |  |
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
