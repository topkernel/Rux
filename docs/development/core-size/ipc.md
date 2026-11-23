# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# ipc — hot 2569 lines (stay) / cold 7288 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| ipc/binder.rs | `ioc` | 66 | 4 | yes |
| ipc/binder.rs | `iow` | 69 | 4 |  |
| ipc/binder.rs | `ior` | 72 | 4 |  |
| ipc/binder.rs | `io` | 75 | 217 |  |
| ipc/binder.rs | `binder_object_size` | 291 | 41 |  |
| ipc/binder.rs | `devno_ctx` | 331 | 16 |  |
| ipc/binder.rs | `align8` | 346 | 60 |  |
| ipc/binder.rs | `new` | 405 | 30 | yes |
| ipc/binder.rs | `span_ok` | 434 | 9 | yes |
| ipc/binder.rs | `aligned` | 442 | 9 |  |
| ipc/binder.rs | `block_window` | 450 | 14 |  |
| ipc/binder.rs | `data_u32` | 463 | 11 |  |
| ipc/binder.rs | `write_data_u32` | 473 | 14 |  |
| ipc/binder.rs | `data_obj` | 486 | 15 |  |
| ipc/binder.rs | `write_data_obj` | 500 | 17 |  |
| ipc/binder.rs | `data_obj_mut` | 516 | 16 |  |
| ipc/binder.rs | `offsets_entry` | 531 | 14 |  |
| ipc/binder.rs | `extra_write_bytes` | 544 | 18 |  |
| ipc/binder.rs | `extra_window` | 561 | 12 |  |
| ipc/binder.rs | `extra_copy_in` | 572 | 9 |  |
| ipc/binder.rs | `block_user_base` | 580 | 12 |  |
| ipc/binder.rs | `parent_window` | 591 | 54 |  |
| ipc/binder.rs | `pod_bytes` | 644 | 11 |  |
| ipc/binder.rs | `u32_at` | 654 | 12 |  |
| ipc/binder.rs | `set_u32` | 665 | 13 |  |
| ipc/binder.rs | `write_u64_at` | 677 | 58 |  |
| ipc/binder.rs | `new` | 734 | 28 | yes |
| ipc/binder.rs | `st` | 761 | 8 |  |
| ipc/binder.rs | `strong` | 768 | 4 |  |
| ipc/binder.rs | `weak` | 771 | 84 |  |
| ipc/binder.rs | `block_need` | 854 | 13 |  |
| ipc/binder.rs | `new_buf` | 866 | 110 |  |
| ipc/binder.rs | `free_buf` | 975 | 35 |  |
| ipc/binder.rs | `coalesce` | 1009 | 57 |  |
| ipc/binder.rs | `sweep_stale_async` | 1065 | 117 |  |
| ipc/binder.rs | `sweep_silent_txns` | 1181 | 144 |  |
| ipc/binder.rs | `binder_sareg_samgr_pid` | 1324 | 12 |  |
| ipc/binder.rs | `binder_probe_on` | 1335 | 37 |  |
| ipc/binder.rs | `new` | 1371 | 9 | yes |
| ipc/binder.rs | `add` | 1379 | 15 | yes |
| ipc/binder.rs | `n` | 1393 | 16 |  |
| ipc/binder.rs | `snapshot` | 1408 | 103 |  |
| ipc/binder.rs | `ams46_on` | 1510 | 27 |  |
| ipc/binder.rs | `is_inst` | 1536 | 22 |  |
| ipc/binder.rs | `txnhunt_on` | 1557 | 7 |  |
| ipc/binder.rs | `is_acct` | 1563 | 44 |  |
| ipc/binder.rs | `deathcb_open` | 1606 | 11 |  |
| ipc/binder.rs | `deathcb_mark` | 1616 | 36 |  |
| ipc/binder.rs | `deathcb_close` | 1651 | 39 |  |
| ipc/binder.rs | `sendfail_why` | 1689 | 7 |  |
| ipc/binder.rs | `sendfail_reason_name` | 1695 | 48 |  |
| ipc/binder.rs | `stage_thread_error` | 1742 | 49 |  |
| ipc/binder.rs | `pool_census_on` | 1790 | 5 |  |
| ipc/binder.rs | `ams46_txns_contains` | 1794 | 7 |  |
| ipc/binder.rs | `ams46_txns_push` | 1800 | 10 |  |
| ipc/binder.rs | `ams46_node_id` | 1809 | 8 |  |
| ipc/binder.rs | `ams46_token_match` | 1816 | 11 |  |
| ipc/binder.rs | `register_app_child_pid` | 1826 | 19 | yes |
| ipc/binder.rs | `app_child_pids_contains` | 1844 | 18 |  |
| ipc/binder.rs | `app_txn_fail_print` | 1861 | 108 |  |
| ipc/binder.rs | `to_tid` | 1968 | 7 |  |
| ipc/binder.rs | `set_to_tid` | 1974 | 6 |  |
| ipc/binder.rs | `pick_j_rd` | 1979 | 4 |  |
| ipc/binder.rs | `pick_ns_rd` | 1982 | 4 |  |
| ipc/binder.rs | `enq_elig_wr` | 1985 | 4 |  |
| ipc/binder.rs | `enq_disp_wr` | 1988 | 4 |  |
| ipc/binder.rs | `enq_disp_rd` | 1991 | 4 |  |
| ipc/binder.rs | `wake_ns_wr` | 1994 | 9 |  |
| ipc/binder.rs | `can_update_with` | 2002 | 23 |  |
| ipc/binder.rs | `holds_async_slot` | 2024 | 5 |  |
| ipc/binder.rs | `set_holds_async_slot` | 2028 | 43 |  |
| ipc/binder.rs | `ev_name` | 2070 | 149 |  |
| ipc/binder.rs | `new` | 2218 | 35 | yes |
| ipc/binder.rs | `st` | 2252 | 7 |  |
| ipc/binder.rs | `hist` | 2258 | 34 |  |
| ipc/binder.rs | `new` | 2291 | 24 | yes |
| ipc/binder.rs | `im` | 2314 | 12 |  |
| ipc/binder.rs | `drop` | 2325 | 57 | yes |
| ipc/binder.rs | `wait_for_context_manager` | 2381 | 113 |  |
| ipc/binder.rs | `drop` | 2493 | 11 | yes |
| ipc/binder.rs | `defer_wake_thread` | 2503 | 8 |  |
| ipc/binder.rs | `deref` | 2510 | 8 |  |
| ipc/binder.rs | `deref_mut` | 2517 | 12 |  |
| ipc/binder.rs | `world` | 2528 | 7 |  |
| ipc/binder.rs | `drop` | 2534 | 66 | yes |
| ipc/binder.rs | `world_section_report` | 2599 | 50 |  |
| ipc/binder.rs | `world_lock` | 2648 | 26 |  |
| ipc/binder.rs | `world_lock_try` | 2673 | 24 |  |
| ipc/binder.rs | `pm` | 2696 | 23 |  |
| ipc/binder.rs | `proc_alive` | 2718 | 5 |  |
| ipc/binder.rs | `wake_thread` | 2722 | 12 |  |
| ipc/binder.rs | `wake_thread_hint` | 2733 | 27 |  |
| ipc/binder.rs | `phex` | 2759 | 8 | yes |
| ipc/binder.rs | `pdec` | 2766 | 75 | yes |
| ipc/binder.rs | `post_txn_to_child_thread` | 2840 | 51 |  |
| ipc/binder.rs | `drain_deliver_parked_txn` | 2890 | 49 |  |
| ipc/binder.rs | `wake_proc_one` | 2938 | 114 |  |
| ipc/binder.rs | `deliver_txn_to_selected_thread` | 3051 | 48 |  |
| ipc/binder.rs | `node_external_refs` | 3098 | 15 |  |
| ipc/binder.rs | `node_is_dead` | 3112 | 15 |  |
| ipc/binder.rs | `proc_of_file` | 3126 | 12 |  |
| ipc/binder.rs | `proc_ref` | 3137 | 11 |  |
| ipc/binder.rs | `binder_open` | 3147 | 52 |  |
| ipc/binder.rs | `binder_flush` | 3198 | 27 |  |
| ipc/binder.rs | `binder_close` | 3224 | 233 |  |
| ipc/binder.rs | `work_involves` | 3456 | 13 |  |
| ipc/binder.rs | `finish_work` | 3468 | 18 |  |
| ipc/binder.rs | `unpin_txn_node` | 3485 | 62 |  |
| ipc/binder.rs | `dead_reply_to_sender` | 3546 | 143 |  |
| ipc/binder.rs | `send_failed_reply` | 3688 | 65 |  |
| ipc/binder.rs | `binder_reap_thread` | 3752 | 59 |  |
| ipc/binder.rs | `binder_task_died` | 3810 | 70 |  |
| ipc/binder.rs | `binder_mmap_handler` | 3879 | 293 | yes |
| ipc/binder.rs | `binder_mmap_owner_of` | 4171 | 34 | yes |
| ipc/binder.rs | `binder_on_tgid_exit` | 4204 | 151 |  |
| ipc/binder.rs | `binder_poll` | 4354 | 56 |  |
| ipc/binder.rs | `binder_poll_wait` | 4409 | 41 |  |
| ipc/binder.rs | `binder_poll_unwait` | 4449 | 36 |  |
| ipc/binder.rs | `current_tid` | 4484 | 6 |  |
| ipc/binder.rs | `binder_file_ioctl` | 4489 | 13 | yes |
| ipc/binder.rs | `stall_thread_line` | 4501 | 94 |  |
| ipc/binder.rs | `entry_frame` | 4594 | 35 |  |
| ipc/binder.rs | `entry_frame_sp` | 4628 | 34 |  |
| ipc/binder.rs | `probe_leaf` | 4661 | 12 |  |
| ipc/binder.rs | `probe_u32` | 4672 | 14 |  |
| ipc/binder.rs | `probe_u64` | 4685 | 16 |  |
| ipc/binder.rs | `entry_frame_futex_x86` | 4700 | 19 |  |
| ipc/binder.rs | `stack_word` | 4718 | 10 |  |
| ipc/binder.rs | `holder_forensics` | 4727 | 193 |  |
| ipc/binder.rs | `dump_latched_maps_taskctx` | 4919 | 122 |  |
| ipc/binder.rs | `latch_park_walk_tgid_sticky` | 5040 | 28 |  |
| ipc/binder.rs | `latch_park_walk_tgid` | 5067 | 32 |  |
| ipc/binder.rs | `sleep_snapshot` | 5098 | 608 |  |
| ipc/binder.rs | `txn_owing_publish` | 5705 | 11 |  |
| ipc/binder.rs | `txn_owing_pass_reset` | 5715 | 13 |  |
| ipc/binder.rs | `txn_owing_of` | 5727 | 47 |  |
| ipc/binder.rs | `txn_hang_audit` | 5773 | 81 | yes |
| ipc/binder.rs | `is_hung_proc` | 5853 | 815 |  |
| ipc/binder.rs | `bc_name` | 6667 | 60 |  |
| ipc/binder.rs | `bwr_park_audit` | 6726 | 797 | yes |
| ipc/binder.rs | `ams46_audit` | 7522 | 163 | yes |
| ipc/edges.rs | `shm_page_get` | 19 | 13 | yes |
| ipc/edges.rs | `shm_page_put` | 31 | 11 | yes |
| ipc/ipc_hooks.rs | `ipc_register_hooks` | 129 | 6 | yes |
| ipc/ipc_hooks.rs | `hooks` | 134 | 9 |  |
| ipc/ipc_hooks.rs | `mq_fds_cleanup` | 142 | 5 | yes |
| ipc/ipc_hooks.rs | `exec_clear_thread_keyring` | 146 | 5 | yes |
| ipc/ipc_hooks.rs | `at_registered_token_of` | 150 | 5 |  |
| ipc/ipc_hooks.rs | `at_mirror_set` | 154 | 5 |  |
| ipc/ipc_hooks.rs | `msgmni` | 158 | 5 | yes |
| ipc/ipc_hooks.rs | `set_msgmni` | 162 | 5 | yes |
| ipc/ipc_hooks.rs | `shm_ctlmax` | 166 | 5 | yes |
| ipc/ipc_hooks.rs | `set_shm_ctlmax` | 170 | 5 | yes |
| ipc/ipc_hooks.rs | `shm_ctlni` | 174 | 5 | yes |
| ipc/ipc_hooks.rs | `set_shm_ctlni` | 178 | 5 | yes |
| ipc/ipc_hooks.rs | `shm_ctlall` | 182 | 5 | yes |
| ipc/ipc_hooks.rs | `set_shm_ctlall` | 186 | 5 | yes |
| ipc/ipc_hooks.rs | `ipc_ids_max` | 190 | 5 |  |
| ipc/ipc_hooks.rs | `queues_max` | 194 | 5 | yes |
| ipc/ipc_hooks.rs | `set_queues_max` | 198 | 5 | yes |
| ipc/ipc_hooks.rs | `key_quota` | 202 | 5 | yes |
| ipc/ipc_hooks.rs | `set_key_quota` | 206 | 5 | yes |
| ipc/ipc_hooks.rs | `msg_proc_show` | 210 | 5 |  |
| ipc/ipc_hooks.rs | `shm_proc_show` | 214 | 5 |  |
| ipc/ipc_hooks.rs | `shm_attach_vma` | 218 | 5 | yes |
| ipc/ipc_hooks.rs | `shm_detach_vma` | 222 | 5 | yes |
| ipc/ipc_hooks.rs | `at_char_device` | 226 | 5 |  |
| ipc/ipc_hooks.rs | `at_token_of` | 230 | 5 |  |
| ipc/ipc_hooks.rs | `init` | 234 | 4 | yes |
| ipc/sysv_sem.rs | `zeroed` | 85 | 73 |  |
| ipc/sysv_sem.rs | `get_perm` | 157 | 4 | yes |
| ipc/sysv_sem.rs | `get_perm_mut` | 160 | 7 | yes |
| ipc/sysv_sem.rs | `new` | 166 | 23 | yes |
| ipc/sysv_sem.rs | `nsems` | 188 | 7 |  |
| ipc/sysv_sem.rs | `get_current_pid` | 194 | 16 | yes |
| ipc/sysv_sem.rs | `sys_semget` | 209 | 43 |  |
| ipc/sysv_sem.rs | `sys_semctl` | 251 | 490 |  |
| ipc/sysv_sem.rs | `proc_show` | 740 | 37 | yes |
| ipc/sysv_sem.rs | `sem_max_idx` | 776 | 12 |  |
| ipc/sysv_sem.rs | `sys_semtimedop` | 787 | 143 | yes |
| ipc/sysv_sem.rs | `semop_kernel` | 929 | 37 |  |
| ipc/sysv_sem.rs | `semop_blocking_loop` | 965 | 273 |  |
| ipc/sysv_sem.rs | `e_seq_mismatch` | 1237 | 17 |  |
| ipc/sysv_sem.rs | `try_apply_semops` | 1253 | 208 |  |
| ipc/sysv_sem.rs | `find_blocking_op` | 1460 | 31 |  |
| ipc/sysv_sem.rs | `sem_undo_exit` | 1490 | 34 |  |
| ipc/sysv_sem.rs | `sys_semop` | 1523 | 13 |  |
| ipc/util.rs | `default` | 98 | 35 | yes |
| ipc/util.rs | `caller_ipc_ns_inum` | 132 | 6 | yes |
| ipc/util.rs | `new` | 137 | 17 | yes |
| ipc/util.rs | `to_uapi` | 153 | 19 | yes |
| ipc/util.rs | `update_from_set` | 171 | 8 | yes |
| ipc/util.rs | `set_creator` | 178 | 14 |  |
| ipc/util.rs | `ipc_build_id` | 191 | 7 | yes |
| ipc/util.rs | `ipc_id_to_index` | 197 | 7 | yes |
| ipc/util.rs | `ipc_id_seq` | 203 | 26 | yes |
| ipc/util.rs | `new` | 228 | 12 | yes |
| ipc/util.rs | `find_by_key` | 239 | 21 | yes |
| ipc/util.rs | `alloc` | 259 | 99 | yes |
| ipc/util.rs | `find` | 357 | 27 | yes |
| ipc/util.rs | `find_including_deleted` | 383 | 23 | yes |
| ipc/util.rs | `find_with_perms` | 405 | 17 | yes |
| ipc/util.rs | `remove` | 421 | 19 | yes |
| ipc/util.rs | `free_slot` | 439 | 10 | yes |
| ipc/util.rs | `count` | 448 | 8 | yes |
| ipc/util.rs | `count_in_ns` | 455 | 21 | yes |
| ipc/util.rs | `get_perm` | 475 | 2 | yes |
| ipc/util.rs | `get_perm_mut` | 476 | 10 | yes |
| ipc/util.rs | `ipc_check_permissions` | 485 | 33 | yes |
| ipc/util.rs | `ipc_current_time` | 517 | 27 | yes |
| ipc/util.rs | `ipc_ids_max` | 543 | 4 |  |
