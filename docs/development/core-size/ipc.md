# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# ipc — hot 1758 lines (stay) / cold 7888 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| ipc/binder.rs | `ioc` | 66 | 3 |  |
| ipc/binder.rs | `iow` | 69 | 3 |  |
| ipc/binder.rs | `ior` | 72 | 3 |  |
| ipc/binder.rs | `io` | 75 | 216 |  |
| ipc/binder.rs | `binder_object_size` | 291 | 40 |  |
| ipc/binder.rs | `devno_ctx` | 331 | 15 |  |
| ipc/binder.rs | `align8` | 346 | 59 |  |
| ipc/binder.rs | `new` | 405 | 29 | yes |
| ipc/binder.rs | `span_ok` | 434 | 8 |  |
| ipc/binder.rs | `aligned` | 442 | 8 |  |
| ipc/binder.rs | `block_window` | 450 | 13 |  |
| ipc/binder.rs | `data_u32` | 463 | 10 |  |
| ipc/binder.rs | `write_data_u32` | 473 | 13 |  |
| ipc/binder.rs | `data_obj` | 486 | 14 |  |
| ipc/binder.rs | `write_data_obj` | 500 | 16 |  |
| ipc/binder.rs | `data_obj_mut` | 516 | 15 |  |
| ipc/binder.rs | `offsets_entry` | 531 | 13 |  |
| ipc/binder.rs | `extra_write_bytes` | 544 | 17 |  |
| ipc/binder.rs | `extra_window` | 561 | 11 |  |
| ipc/binder.rs | `extra_copy_in` | 572 | 8 |  |
| ipc/binder.rs | `block_user_base` | 580 | 11 |  |
| ipc/binder.rs | `parent_window` | 591 | 53 |  |
| ipc/binder.rs | `pod_bytes` | 644 | 10 |  |
| ipc/binder.rs | `u32_at` | 654 | 11 |  |
| ipc/binder.rs | `set_u32` | 665 | 12 |  |
| ipc/binder.rs | `write_u64_at` | 677 | 57 |  |
| ipc/binder.rs | `new` | 734 | 27 | yes |
| ipc/binder.rs | `st` | 761 | 7 |  |
| ipc/binder.rs | `strong` | 768 | 3 |  |
| ipc/binder.rs | `weak` | 771 | 83 |  |
| ipc/binder.rs | `block_need` | 854 | 12 |  |
| ipc/binder.rs | `new_buf` | 866 | 109 |  |
| ipc/binder.rs | `free_buf` | 975 | 34 |  |
| ipc/binder.rs | `coalesce` | 1009 | 56 |  |
| ipc/binder.rs | `sweep_stale_async` | 1065 | 116 |  |
| ipc/binder.rs | `sweep_silent_txns` | 1181 | 143 |  |
| ipc/binder.rs | `binder_sareg_samgr_pid` | 1324 | 11 |  |
| ipc/binder.rs | `binder_probe_on` | 1335 | 36 |  |
| ipc/binder.rs | `new` | 1371 | 8 | yes |
| ipc/binder.rs | `add` | 1379 | 14 | yes |
| ipc/binder.rs | `n` | 1393 | 15 |  |
| ipc/binder.rs | `snapshot` | 1408 | 102 |  |
| ipc/binder.rs | `ams46_on` | 1510 | 26 |  |
| ipc/binder.rs | `is_inst` | 1536 | 21 |  |
| ipc/binder.rs | `txnhunt_on` | 1557 | 6 |  |
| ipc/binder.rs | `is_acct` | 1563 | 43 |  |
| ipc/binder.rs | `deathcb_open` | 1606 | 10 |  |
| ipc/binder.rs | `deathcb_mark` | 1616 | 35 |  |
| ipc/binder.rs | `deathcb_close` | 1651 | 38 |  |
| ipc/binder.rs | `sendfail_why` | 1689 | 6 |  |
| ipc/binder.rs | `sendfail_reason_name` | 1695 | 47 |  |
| ipc/binder.rs | `stage_thread_error` | 1742 | 48 |  |
| ipc/binder.rs | `pool_census_on` | 1790 | 4 |  |
| ipc/binder.rs | `ams46_txns_contains` | 1794 | 6 |  |
| ipc/binder.rs | `ams46_txns_push` | 1800 | 9 |  |
| ipc/binder.rs | `ams46_node_id` | 1809 | 7 |  |
| ipc/binder.rs | `ams46_token_match` | 1816 | 10 |  |
| ipc/binder.rs | `register_app_child_pid` | 1826 | 18 |  |
| ipc/binder.rs | `app_child_pids_contains` | 1844 | 17 |  |
| ipc/binder.rs | `app_txn_fail_print` | 1861 | 107 |  |
| ipc/binder.rs | `to_tid` | 1968 | 6 |  |
| ipc/binder.rs | `set_to_tid` | 1974 | 5 |  |
| ipc/binder.rs | `pick_j_rd` | 1979 | 3 |  |
| ipc/binder.rs | `pick_ns_rd` | 1982 | 3 |  |
| ipc/binder.rs | `enq_elig_wr` | 1985 | 3 |  |
| ipc/binder.rs | `enq_disp_wr` | 1988 | 3 |  |
| ipc/binder.rs | `enq_disp_rd` | 1991 | 3 |  |
| ipc/binder.rs | `wake_ns_wr` | 1994 | 8 |  |
| ipc/binder.rs | `can_update_with` | 2002 | 22 |  |
| ipc/binder.rs | `holds_async_slot` | 2024 | 4 |  |
| ipc/binder.rs | `set_holds_async_slot` | 2028 | 42 |  |
| ipc/binder.rs | `ev_name` | 2070 | 148 |  |
| ipc/binder.rs | `new` | 2218 | 34 | yes |
| ipc/binder.rs | `st` | 2252 | 6 |  |
| ipc/binder.rs | `hist` | 2258 | 33 |  |
| ipc/binder.rs | `new` | 2291 | 23 | yes |
| ipc/binder.rs | `im` | 2314 | 11 |  |
| ipc/binder.rs | `drop` | 2325 | 56 | yes |
| ipc/binder.rs | `wait_for_context_manager` | 2381 | 112 |  |
| ipc/binder.rs | `drop` | 2493 | 10 | yes |
| ipc/binder.rs | `defer_wake_thread` | 2503 | 7 |  |
| ipc/binder.rs | `deref` | 2510 | 7 | yes |
| ipc/binder.rs | `deref_mut` | 2517 | 11 | yes |
| ipc/binder.rs | `world` | 2528 | 6 |  |
| ipc/binder.rs | `drop` | 2534 | 65 | yes |
| ipc/binder.rs | `world_section_report` | 2599 | 49 |  |
| ipc/binder.rs | `world_lock` | 2648 | 25 |  |
| ipc/binder.rs | `world_lock_try` | 2673 | 23 |  |
| ipc/binder.rs | `pm` | 2696 | 22 |  |
| ipc/binder.rs | `proc_alive` | 2718 | 4 |  |
| ipc/binder.rs | `wake_thread` | 2722 | 11 |  |
| ipc/binder.rs | `wake_thread_hint` | 2733 | 26 |  |
| ipc/binder.rs | `phex` | 2759 | 7 | yes |
| ipc/binder.rs | `pdec` | 2766 | 74 | yes |
| ipc/binder.rs | `post_txn_to_child_thread` | 2840 | 50 |  |
| ipc/binder.rs | `drain_deliver_parked_txn` | 2890 | 48 |  |
| ipc/binder.rs | `wake_proc_one` | 2938 | 113 |  |
| ipc/binder.rs | `deliver_txn_to_selected_thread` | 3051 | 47 |  |
| ipc/binder.rs | `node_external_refs` | 3098 | 14 |  |
| ipc/binder.rs | `node_is_dead` | 3112 | 14 |  |
| ipc/binder.rs | `proc_of_file` | 3126 | 11 |  |
| ipc/binder.rs | `proc_ref` | 3137 | 10 |  |
| ipc/binder.rs | `binder_open` | 3147 | 51 |  |
| ipc/binder.rs | `binder_flush` | 3198 | 26 |  |
| ipc/binder.rs | `binder_close` | 3224 | 232 |  |
| ipc/binder.rs | `work_involves` | 3456 | 12 |  |
| ipc/binder.rs | `finish_work` | 3468 | 17 |  |
| ipc/binder.rs | `unpin_txn_node` | 3485 | 61 |  |
| ipc/binder.rs | `dead_reply_to_sender` | 3546 | 142 |  |
| ipc/binder.rs | `send_failed_reply` | 3688 | 64 |  |
| ipc/binder.rs | `binder_reap_thread` | 3752 | 58 |  |
| ipc/binder.rs | `binder_task_died` | 3810 | 69 |  |
| ipc/binder.rs | `binder_mmap_handler` | 3879 | 292 |  |
| ipc/binder.rs | `binder_mmap_owner_of` | 4171 | 33 | yes |
| ipc/binder.rs | `binder_on_tgid_exit` | 4204 | 150 |  |
| ipc/binder.rs | `binder_poll` | 4354 | 55 |  |
| ipc/binder.rs | `binder_poll_wait` | 4409 | 40 |  |
| ipc/binder.rs | `binder_poll_unwait` | 4449 | 35 |  |
| ipc/binder.rs | `current_tid` | 4484 | 5 |  |
| ipc/binder.rs | `binder_file_ioctl` | 4489 | 12 |  |
| ipc/binder.rs | `stall_thread_line` | 4501 | 93 |  |
| ipc/binder.rs | `entry_frame` | 4594 | 34 |  |
| ipc/binder.rs | `entry_frame_sp` | 4628 | 33 |  |
| ipc/binder.rs | `probe_leaf` | 4661 | 11 |  |
| ipc/binder.rs | `probe_u32` | 4672 | 13 |  |
| ipc/binder.rs | `probe_u64` | 4685 | 15 |  |
| ipc/binder.rs | `entry_frame_futex_x86` | 4700 | 18 |  |
| ipc/binder.rs | `stack_word` | 4718 | 9 |  |
| ipc/binder.rs | `holder_forensics` | 4727 | 192 |  |
| ipc/binder.rs | `dump_latched_maps_taskctx` | 4919 | 121 |  |
| ipc/binder.rs | `latch_park_walk_tgid_sticky` | 5040 | 27 |  |
| ipc/binder.rs | `latch_park_walk_tgid` | 5067 | 31 |  |
| ipc/binder.rs | `sleep_snapshot` | 5098 | 607 |  |
| ipc/binder.rs | `txn_owing_publish` | 5705 | 10 |  |
| ipc/binder.rs | `txn_owing_pass_reset` | 5715 | 12 |  |
| ipc/binder.rs | `txn_owing_of` | 5727 | 46 |  |
| ipc/binder.rs | `txn_hang_audit` | 5773 | 80 | yes |
| ipc/binder.rs | `is_hung_proc` | 5853 | 814 |  |
| ipc/binder.rs | `bc_name` | 6667 | 59 |  |
| ipc/binder.rs | `bwr_park_audit` | 6726 | 796 | yes |
| ipc/binder.rs | `ams46_audit` | 7522 | 162 | yes |
| ipc/edges.rs | `shm_page_get` | 19 | 12 |  |
| ipc/edges.rs | `shm_page_put` | 31 | 10 |  |
| ipc/ipc_hooks.rs | `ipc_register_hooks` | 129 | 5 |  |
| ipc/ipc_hooks.rs | `hooks` | 134 | 8 |  |
| ipc/ipc_hooks.rs | `mq_fds_cleanup` | 142 | 4 |  |
| ipc/ipc_hooks.rs | `exec_clear_thread_keyring` | 146 | 4 |  |
| ipc/ipc_hooks.rs | `at_registered_token_of` | 150 | 4 |  |
| ipc/ipc_hooks.rs | `at_mirror_set` | 154 | 4 |  |
| ipc/ipc_hooks.rs | `msgmni` | 158 | 4 |  |
| ipc/ipc_hooks.rs | `set_msgmni` | 162 | 4 |  |
| ipc/ipc_hooks.rs | `shm_ctlmax` | 166 | 4 |  |
| ipc/ipc_hooks.rs | `set_shm_ctlmax` | 170 | 4 |  |
| ipc/ipc_hooks.rs | `shm_ctlni` | 174 | 4 |  |
| ipc/ipc_hooks.rs | `set_shm_ctlni` | 178 | 4 |  |
| ipc/ipc_hooks.rs | `shm_ctlall` | 182 | 4 |  |
| ipc/ipc_hooks.rs | `set_shm_ctlall` | 186 | 4 |  |
| ipc/ipc_hooks.rs | `ipc_ids_max` | 190 | 4 |  |
| ipc/ipc_hooks.rs | `queues_max` | 194 | 4 |  |
| ipc/ipc_hooks.rs | `set_queues_max` | 198 | 4 |  |
| ipc/ipc_hooks.rs | `key_quota` | 202 | 4 |  |
| ipc/ipc_hooks.rs | `set_key_quota` | 206 | 4 |  |
| ipc/ipc_hooks.rs | `msg_proc_show` | 210 | 4 |  |
| ipc/ipc_hooks.rs | `shm_proc_show` | 214 | 4 |  |
| ipc/ipc_hooks.rs | `shm_attach_vma` | 218 | 4 |  |
| ipc/ipc_hooks.rs | `shm_detach_vma` | 222 | 4 |  |
| ipc/ipc_hooks.rs | `at_char_device` | 226 | 4 |  |
| ipc/ipc_hooks.rs | `at_token_of` | 230 | 4 |  |
| ipc/ipc_hooks.rs | `init` | 234 | 3 | yes |
| ipc/sysv_sem.rs | `zeroed` | 85 | 72 | yes |
| ipc/sysv_sem.rs | `get_perm` | 157 | 3 |  |
| ipc/sysv_sem.rs | `get_perm_mut` | 160 | 6 |  |
| ipc/sysv_sem.rs | `new` | 166 | 22 | yes |
| ipc/sysv_sem.rs | `nsems` | 188 | 6 |  |
| ipc/sysv_sem.rs | `get_current_pid` | 194 | 15 | yes |
| ipc/sysv_sem.rs | `sys_semget` | 209 | 42 |  |
| ipc/sysv_sem.rs | `sys_semctl` | 251 | 489 |  |
| ipc/sysv_sem.rs | `proc_show` | 740 | 36 |  |
| ipc/sysv_sem.rs | `sem_max_idx` | 776 | 11 |  |
| ipc/sysv_sem.rs | `sys_semtimedop` | 787 | 142 |  |
| ipc/sysv_sem.rs | `semop_kernel` | 929 | 36 |  |
| ipc/sysv_sem.rs | `semop_blocking_loop` | 965 | 272 |  |
| ipc/sysv_sem.rs | `e_seq_mismatch` | 1237 | 16 |  |
| ipc/sysv_sem.rs | `try_apply_semops` | 1253 | 207 |  |
| ipc/sysv_sem.rs | `find_blocking_op` | 1460 | 30 |  |
| ipc/sysv_sem.rs | `sem_undo_exit` | 1490 | 33 |  |
| ipc/sysv_sem.rs | `sys_semop` | 1523 | 12 |  |
| ipc/util.rs | `default` | 98 | 34 | yes |
| ipc/util.rs | `caller_ipc_ns_inum` | 132 | 5 |  |
| ipc/util.rs | `new` | 137 | 16 | yes |
| ipc/util.rs | `to_uapi` | 153 | 18 |  |
| ipc/util.rs | `update_from_set` | 171 | 7 |  |
| ipc/util.rs | `set_creator` | 178 | 13 |  |
| ipc/util.rs | `ipc_build_id` | 191 | 6 |  |
| ipc/util.rs | `ipc_id_to_index` | 197 | 6 |  |
| ipc/util.rs | `ipc_id_seq` | 203 | 25 |  |
| ipc/util.rs | `new` | 228 | 11 | yes |
| ipc/util.rs | `find_by_key` | 239 | 20 |  |
| ipc/util.rs | `alloc` | 259 | 98 | yes |
| ipc/util.rs | `find` | 357 | 26 | yes |
| ipc/util.rs | `find_including_deleted` | 383 | 22 |  |
| ipc/util.rs | `find_with_perms` | 405 | 16 |  |
| ipc/util.rs | `remove` | 421 | 18 | yes |
| ipc/util.rs | `free_slot` | 439 | 9 |  |
| ipc/util.rs | `count` | 448 | 7 | yes |
| ipc/util.rs | `count_in_ns` | 455 | 20 |  |
| ipc/util.rs | `get_perm` | 475 | 1 |  |
| ipc/util.rs | `get_perm_mut` | 476 | 9 |  |
| ipc/util.rs | `ipc_check_permissions` | 485 | 32 |  |
| ipc/util.rs | `ipc_current_time` | 517 | 26 |  |
| ipc/util.rs | `ipc_ids_max` | 543 | 3 |  |
