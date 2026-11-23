# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# process — hot 3056 lines (stay) / cold 15263 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| process/coredump.rs | `signal_makes_core` | 67 | 19 |  |
| process/coredump.rs | `push_u32` | 85 | 4 |  |
| process/coredump.rs | `push_u64` | 88 | 7 |  |
| process/coredump.rs | `push_note` | 94 | 15 |  |
| process/coredump.rs | `push_phdr` | 108 | 27 |  |
| process/coredump.rs | `build_prstatus` | 134 | 46 |  |
| process/coredump.rs | `build_prpsinfo` | 179 | 43 |  |
| process/coredump.rs | `expand_core_pattern` | 221 | 65 |  |
| process/coredump.rs | `do_coredump` | 285 | 223 |  |
| process/exec.rs | `new` | 26 | 22 | yes |
| process/exec.rs | `do_execve_elf` | 47 | 218 |  |
| process/exec.rs | `drop` | 264 | 1216 | yes |
| process/exit.rs | `exit_robust_list` | 49 | 37 |  |
| process/exit.rs | `handle_robust_entry` | 85 | 28 |  |
| process/exit.rs | `handle_pending_entry` | 112 | 19 |  |
| process/exit.rs | `walk_robust_chain` | 130 | 42 |  |
| process/exit.rs | `exit_clear_child_tid` | 171 | 44 |  |
| process/exit.rs | `kill_other_threads` | 214 | 24 |  |
| process/exit.rs | `pd` | 237 | 196 |  |
| process/exit.rs | `wait_for_thread_group_death` | 432 | 49 |  |
| process/exit.rs | `de_thread` | 480 | 41 |  |
| process/exit.rs | `free_dead_member` | 520 | 185 |  |
| process/exit.rs | `sweep_dead_threads` | 704 | 139 |  |
| process/exit.rs | `release_task` | 842 | 299 |  |
| process/exit.rs | `release_task_storage` | 1140 | 170 |  |
| process/exit.rs | `do_exit` | 1309 | 1221 |  |
| process/exit.rs | `do_exit_group` | 2529 | 60 |  |
| process/exit.rs | `reparent_children_to_init` | 2588 | 273 |  |
| process/exit.rs | `wake_group_chldexit` | 2860 | 60 |  |
| process/exit.rs | `wl_entry_syscall` | 2919 | 39 |  |
| process/exit.rs | `wait4_park_audit` | 2957 | 200 |  |
| process/exit.rs | `wait_pid_matches` | 3156 | 26 |  |
| process/exit.rs | `do_wait` | 3181 | 330 |  |
| process/exit.rs | `do_wait_nonblock` | 3510 | 213 |  |
| process/exit.rs | `write_siginfo` | 3722 | 36 |  |
| process/exit.rs | `do_waitid` | 3757 | 225 |  |
| process/fork.rs | `do_fork` | 69 | 31 |  |
| process/fork.rs | `copy_thread` | 99 | 137 |  |
| process/fork.rs | `ret_from_fork` | 235 | 68 |  |
| process/fork.rs | `einval` | 302 | 2 |  |
| process/fork.rs | `eagain` | 303 | 2 |  |
| process/fork.rs | `enomem` | 304 | 2 |  |
| process/fork.rs | `efault` | 305 | 9 |  |
| process/fork.rs | `clone_warn_budgeted` | 313 | 19 |  |
| process/fork.rs | `do_clone` | 331 | 354 |  |
| process/fork.rs | `write_child_settid` | 684 | 524 |  |
| process/kthread.rs | `kernel_thread_create` | 59 | 56 |  |
| process/kthread.rs | `ret_from_fork_kernel_asm` | 114 | 9 |  |
| process/kthread.rs | `ret_from_fork_kernel` | 122 | 6 |  |
| process/kthread.rs | `ret_from_fork_kernel_asm` | 127 | 77 |  |
| process/kthread.rs | `kthread_enqueue` | 203 | 36 |  |
| process/kthread.rs | `kernel_thread` | 238 | 18 |  |
| process/kthread.rs | `kthread_run_on_cpu` | 255 | 16 |  |
| process/kthread.rs | `kthread_run` | 270 | 17 |  |
| process/kthread.rs | `kthread_should_stop` | 286 | 12 |  |
| process/kthread.rs | `kthread_name` | 297 | 15 |  |
| process/kthread.rs | `kthread_stop` | 311 | 37 |  |
| process/kthread.rs | `kthread_bind` | 347 | 6 |  |
| process/mod.rs | `thread_clear_callee_saved` | 38 | 14 |  |
| process/mod.rs | `thread_set_entry` | 51 | 28 |  |
| process/mod.rs | `set_user_tls` | 78 | 23 |  |
| process/mod.rs | `current_pid` | 100 | 6 | yes |
| process/mod.rs | `current_ppid` | 105 | 6 |  |
| process/mod.rs | `current_pgid` | 110 | 9 |  |
| process/mod.rs | `current_task` | 118 | 9 |  |
| process/mod.rs | `find_task_by_pid` | 126 | 11 | yes |
| process/ns.rs | `new` | 72 | 28 | yes |
| process/ns.rs | `uts_set` | 99 | 13 |  |
| process/ns.rs | `uts_get` | 111 | 6 |  |
| process/ns.rs | `set_hostname` | 116 | 5 |  |
| process/ns.rs | `get_hostname` | 120 | 5 |  |
| process/ns.rs | `set_domainname` | 124 | 5 |  |
| process/ns.rs | `get_domainname` | 128 | 37 |  |
| process/ns.rs | `alloc_mount_id` | 164 | 32 |  |
| process/ns.rs | `drop` | 195 | 14 | yes |
| process/ns.rs | `new` | 208 | 13 | yes |
| process/ns.rs | `ns_track_vfsmount` | 220 | 9 |  |
| process/ns.rs | `in_init_mnt_ns` | 228 | 25 |  |
| process/ns.rs | `new` | 252 | 14 | yes |
| process/ns.rs | `alloc_local_pid` | 265 | 53 |  |
| process/ns.rs | `alloc_ns_inum` | 317 | 13 |  |
| process/ns.rs | `init_uts_ns` | 329 | 14 |  |
| process/ns.rs | `init_mnt_ns` | 342 | 11 |  |
| process/ns.rs | `init_pid_ns` | 352 | 11 |  |
| process/ns.rs | `init_net_ns` | 362 | 13 |  |
| process/ns.rs | `init_ipc_ns` | 374 | 13 |  |
| process/ns.rs | `init_user_ns` | 386 | 15 |  |
| process/ns.rs | `is_init_pid_ns` | 400 | 10 |  |
| process/ns.rs | `upid_in_ns` | 409 | 19 |  |
| process/ns.rs | `resolve_local_pid_to_global` | 427 | 26 |  |
| process/ns.rs | `translate_sender_pid_ns` | 452 | 22 |  |
| process/ns.rs | `current_uts_ns` | 473 | 7 |  |
| process/ns.rs | `current_mnt_ns` | 479 | 8 |  |
| process/ns.rs | `current_pid_ns` | 486 | 8 |  |
| process/ns.rs | `current_net_ns` | 493 | 8 |  |
| process/ns.rs | `current_ipc_ns` | 500 | 8 |  |
| process/ns.rs | `current_user_ns` | 507 | 13 |  |
| process/ns.rs | `boot_mount_entries` | 519 | 18 |  |
| process/ns.rs | `ns_register_mount` | 536 | 22 |  |
| process/ns.rs | `ns_unregister_mount` | 557 | 8 |  |
| process/ns.rs | `ns_get_mounts` | 564 | 6 |  |
| process/ns.rs | `ns_is_mountpoint` | 569 | 14 |  |
| process/ns.rs | `task_pid_ns_for_children` | 582 | 11 |  |
| process/ns.rs | `resolve_vpid` | 592 | 28 |  |
| process/ns.rs | `resolve_vtid` | 619 | 27 |  |
| process/ns.rs | `task_vpid` | 645 | 10 |  |
| process/ns.rs | `task_same_pid_ns` | 654 | 12 |  |
| process/ns.rs | `wait_vpid` | 665 | 27 |  |
| process/ns.rs | `check_ns_capability` | 691 | 16 |  |
| process/ns.rs | `copy_namespaces` | 706 | 116 |  |
| process/ns.rs | `unshare_namespaces` | 821 | 69 |  |
| process/ns.rs | `task_uts_of` | 889 | 6 |  |
| process/ns.rs | `task_mnt_table_of` | 894 | 24 |  |
| process/ns.rs | `name` | 917 | 12 |  |
| process/ns.rs | `from_name` | 928 | 14 |  |
| process/ns.rs | `clone_flag` | 941 | 23 |  |
| process/ns.rs | `index` | 963 | 13 |  |
| process/ns.rs | `from_index` | 975 | 26 |  |
| process/ns.rs | `kind` | 1000 | 12 |  |
| process/ns.rs | `inum` | 1011 | 17 |  |
| process/ns.rs | `ns_fd_register` | 1027 | 9 |  |
| process/ns.rs | `ns_fd_lookup` | 1035 | 11 |  |
| process/ns.rs | `task_ns_handle` | 1045 | 13 |  |
| process/ns.rs | `ns_link_target` | 1057 | 10 |  |
| process/ns.rs | `fallback_inum` | 1066 | 5 |  |
| process/ns.rs | `task_ns_handle_init` | 1070 | 13 |  |
| process/ns.rs | `ns_file_content` | 1082 | 10 |  |
| process/ns.rs | `open_ns_file` | 1091 | 11 |  |
| process/ns.rs | `build_ns_file` | 1101 | 41 |  |
| process/ns.rs | `ns_file_read` | 1141 | 20 |  |
| process/ns.rs | `ns_file_close` | 1160 | 16 |  |
| process/ns.rs | `try_open_ns_path` | 1175 | 40 |  |
| process/ns.rs | `setns` | 1214 | 65 |  |
| process/ns.rs | `reanchor_root_for_mnt_ns` | 1278 | 53 |  |
| process/pid.rs | `pid_max_effective` | 44 | 8 |  |
| process/pid.rs | `set_pid_max_live` | 51 | 6 |  |
| process/pid.rs | `pid_max_live` | 56 | 26 |  |
| process/pid.rs | `new` | 81 | 15 | yes |
| process/pid.rs | `scan_range` | 95 | 31 |  |
| process/pid.rs | `find_next_zero` | 125 | 32 |  |
| process/pid.rs | `alloc_pid` | 156 | 25 |  |
| process/pid.rs | `allocated_count` | 180 | 8 |  |
| process/pid.rs | `free_pid` | 187 | 27 |  |
| process/pid.rs | `last_pid` | 213 | 11 |  |
| process/pid.rs | `set_last_pid` | 223 | 9 |  |
| process/pid_hash.rs | `bucket_index` | 39 | 18 |  |
| process/pid_hash.rs | `lock_bucket` | 56 | 23 |  |
| process/pid_hash.rs | `try_lock_bucket` | 78 | 15 |  |
| process/pid_hash.rs | `unlock_bucket` | 92 | 11 |  |
| process/pid_hash.rs | `init` | 102 | 12 | yes |
| process/pid_hash.rs | `pid_hash_insert` | 113 | 23 |  |
| process/pid_hash.rs | `pid_hash_remove` | 135 | 48 |  |
| process/pid_hash.rs | `pid_hash_lookup_pinned` | 182 | 11 |  |
| process/pid_hash.rs | `pid_hash_lookup` | 192 | 31 |  |
| process/pid_hash.rs | `pid_hash_for_each_task` | 222 | 30 |  |
| process/pid_hash.rs | `pid_hash_for_each_task_try` | 251 | 42 |  |
| process/pid_hash.rs | `pid_hash_collect_all` | 292 | 20 |  |
| process/ptrace.rs | `ptrace_may_access` | 85 | 22 |  |
| process/ptrace.rs | `read_target_word` | 106 | 50 |  |
| process/ptrace.rs | `write_target_word` | 155 | 61 |  |
| process/ptrace.rs | `process_vm_copy` | 215 | 93 |  |
| process/ptrace.rs | `ptrace_stop` | 307 | 35 |  |
| process/ptrace.rs | `sys_ptrace` | 341 | 49 |  |
| process/ptrace.rs | `dispatch_request` | 389 | 322 |  |
| process/reboot.rs | `ctrl_alt_del_latch` | 31 | 10 |  |
| process/reboot.rs | `cad_deliver_pending` | 40 | 49 |  |
| process/reboot.rs | `pid1_exited` | 88 | 37 |  |
| process/reboot.rs | `init_exited_fallback_cascade` | 124 | 20 |  |
| process/reboot.rs | `panic_reboot` | 143 | 37 |  |
| process/reboot.rs | `force_reboot` | 179 | 12 |  |
| process/reboot.rs | `force_power_off` | 190 | 13 |  |
| process/reboot.rs | `shutdown_cascade` | 202 | 108 |  |
| process/reboot.rs | `machine_park` | 309 | 9 |  |
| process/reboot.rs | `sbi_legacy_shutdown` | 317 | 15 |  |
| process/reboot.rs | `sleep_one_tick` | 331 | 28 |  |
| process/reboot.rs | `set_cad_immediate` | 358 | 4 |  |
| process/task.rs | `new` | 53 | 12 | yes |
| process/task.rs | `pop` | 64 | 21 |  |
| process/task.rs | `push` | 84 | 112 |  |
| process/task.rs | `kstack_slab_alloc` | 195 | 41 |  |
| process/task.rs | `kstack_slab_free` | 235 | 51 |  |
| process/task.rs | `kstack_slab_stats` | 285 | 13 |  |
| process/task.rs | `kstack_slab_stats` | 297 | 17 |  |
| process/task.rs | `kstack_anat` | 313 | 9 |  |
| process/task.rs | `plain_kstack_alloc` | 321 | 8 |  |
| process/task.rs | `stack_cache_alloc` | 328 | 121 |  |
| process/task.rs | `note_kstack_freed` | 448 | 13 |  |
| process/task.rs | `freed_stack_containing` | 460 | 17 |  |
| process/task.rs | `stack_cache_free` | 476 | 133 |  |
| process/task.rs | `new` | 608 | 7 | yes |
| process/task.rs | `bits` | 614 | 7 | yes |
| process/task.rs | `contains` | 620 | 7 |  |
| process/task.rs | `is_running` | 626 | 7 |  |
| process/task.rs | `is_sleeping` | 632 | 7 |  |
| process/task.rs | `is_dead` | 638 | 7 |  |
| process/task.rs | `is_interruptible` | 644 | 7 |  |
| process/task.rs | `default` | 650 | 107 |  |
| process/task.rs | `new_init` | 756 | 35 |  |
| process/task.rs | `keeps_caps_on_setuid` | 790 | 9 |  |
| process/task.rs | `new_user` | 798 | 23 |  |
| process/task.rs | `in_group` | 820 | 956 |  |
| process/task.rs | `default_rlimits` | 1775 | 41 |  |
| process/task.rs | `clone` | 1815 | 27 |  |
| process/task.rs | `new` | 1841 | 232 | yes |
| process/task.rs | `new_idle_at` | 2072 | 40 |  |
| process/task.rs | `register_slot_deferred_sizes` | 2111 | 23 |  |
| process/task.rs | `new_task_at` | 2133 | 56 |  |
| process/task.rs | `state` | 2188 | 7 |  |
| process/task.rs | `set_state` | 2194 | 7 |  |
| process/task.rs | `is_state` | 2200 | 38 |  |
| process/task.rs | `sleep` | 2237 | 52 |  |
| process/task.rs | `is_plausible_task_ptr` | 2288 | 39 |  |
| process/task.rs | `wake_up` | 2326 | 9 |  |
| process/task.rs | `wake_up_sync` | 2334 | 13 |  |
| process/task.rs | `wake_up_in_futex_bucket` | 2346 | 13 |  |
| process/task.rs | `wake_up_impl` | 2358 | 111 |  |
| process/task.rs | `ph` | 2468 | 411 |  |
| process/task.rs | `pid` | 2878 | 12 | yes |
| process/task.rs | `rlimit` | 2889 | 9 |  |
| process/task.rs | `cpu_time_last_sigxcpu` | 2897 | 6 |  |
| process/task.rs | `set_cpu_time_last_sigxcpu` | 2902 | 14 |  |
| process/task.rs | `tick_time_slice` | 2915 | 14 |  |
| process/task.rs | `reset_time_slice` | 2928 | 7 |  |
| process/task.rs | `time_slice_expired` | 2934 | 7 |  |
| process/task.rs | `get_time_slice` | 2940 | 7 |  |
| process/task.rs | `set_time_slice` | 2946 | 11 |  |
| process/task.rs | `sched_entity` | 2956 | 7 |  |
| process/task.rs | `sched_entity_mut` | 2962 | 7 |  |
| process/task.rs | `policy` | 2968 | 7 |  |
| process/task.rs | `set_policy` | 2974 | 7 |  |
| process/task.rs | `static_prio` | 2980 | 7 |  |
| process/task.rs | `prio` | 2986 | 7 |  |
| process/task.rs | `set_prio` | 2992 | 7 |  |
| process/task.rs | `rt_priority` | 2998 | 7 |  |
| process/task.rs | `set_rt_priority` | 3004 | 7 |  |
| process/task.rs | `rt_entity` | 3010 | 7 |  |
| process/task.rs | `rt_entity_mut` | 3016 | 7 |  |
| process/task.rs | `dl_entity` | 3022 | 7 |  |
| process/task.rs | `dl_entity_mut` | 3028 | 7 |  |
| process/task.rs | `sched_reset_on_fork` | 3034 | 8 |  |
| process/task.rs | `set_sched_reset_on_fork` | 3041 | 10 |  |
| process/task.rs | `nice` | 3050 | 8 |  |
| process/task.rs | `set_nice` | 3057 | 18 |  |
| process/task.rs | `ppid` | 3074 | 23 |  |
| process/task.rs | `is_fork_child` | 3096 | 7 |  |
| process/task.rs | `set_fork_child` | 3102 | 8 |  |
| process/task.rs | `fork_pt_regs` | 3109 | 9 | yes |
| process/task.rs | `clear_fork_child` | 3117 | 7 |  |
| process/task.rs | `tgid` | 3123 | 7 |  |
| process/task.rs | `set_tgid` | 3129 | 9 |  |
| process/task.rs | `group_leader_ptr` | 3137 | 16 |  |
| process/task.rs | `group_leader_raw` | 3152 | 7 |  |
| process/task.rs | `set_group_leader` | 3158 | 7 |  |
| process/task.rs | `next_thread_ptr` | 3164 | 11 |  |
| process/task.rs | `nr_threads` | 3174 | 7 |  |
| process/task.rs | `is_thread_group_leader` | 3180 | 16 |  |
| process/task.rs | `thread_group_join` | 3195 | 25 |  |
| process/task.rs | `thread_group_leave` | 3219 | 55 |  |
| process/task.rs | `for_each_group_member` | 3273 | 23 |  |
| process/task.rs | `cred` | 3295 | 7 |  |
| process/task.rs | `cred_mut` | 3301 | 7 |  |
| process/task.rs | `access_token` | 3307 | 6 |  |
| process/task.rs | `set_access_token` | 3312 | 7 |  |
| process/task.rs | `access_ftoken` | 3318 | 6 |  |
| process/task.rs | `set_access_ftoken` | 3323 | 7 |  |
| process/task.rs | `address_space_mut` | 3329 | 13 |  |
| process/task.rs | `address_space` | 3341 | 14 |  |
| process/task.rs | `set_address_space` | 3354 | 32 |  |
| process/task.rs | `address_space_arc` | 3385 | 14 |  |
| process/task.rs | `mm_ptr_peek` | 3398 | 7 |  |
| process/task.rs | `active_mm` | 3404 | 15 |  |
| process/task.rs | `clear_active_mm` | 3418 | 6 |  |
| process/task.rs | `set_vfork_parent` | 3423 | 6 |  |
| process/task.rs | `clear_vfork_parent` | 3428 | 6 |  |
| process/task.rs | `vfork_parent_ptr` | 3433 | 17 |  |
| process/task.rs | `vfork_wake_parent` | 3449 | 14 |  |
| process/task.rs | `preempt_underflow_report` | 3462 | 21 |  |
| process/task.rs | `ph` | 3482 | 64 |  |
| process/task.rs | `thread` | 3545 | 6 |  |
| process/task.rs | `thread_mut` | 3550 | 9 | yes |
| process/task.rs | `ti_flags` | 3558 | 7 |  |
| process/task.rs | `set_ti_flags` | 3564 | 7 |  |
| process/task.rs | `test_ti_flag` | 3570 | 7 |  |
| process/task.rs | `set_ti_flag` | 3576 | 7 |  |
| process/task.rs | `clear_ti_flag` | 3582 | 7 |  |
| process/task.rs | `need_resched` | 3588 | 7 |  |
| process/task.rs | `set_need_resched_flag` | 3594 | 7 |  |
| process/task.rs | `clear_need_resched_flag` | 3600 | 7 |  |
| process/task.rs | `has_pending_signal` | 3606 | 7 |  |
| process/task.rs | `set_pending_signal_flag` | 3612 | 7 |  |
| process/task.rs | `preempt_count` | 3618 | 7 |  |
| process/task.rs | `inc_preempt_count` | 3624 | 7 |  |
| process/task.rs | `dec_preempt_count` | 3630 | 7 |  |
| process/task.rs | `preemptible` | 3636 | 7 |  |
| process/task.rs | `add_preempt_count` | 3642 | 20 |  |
| process/task.rs | `sub_preempt_count` | 3661 | 8 |  |
| process/task.rs | `sub_preempt_count_at` | 3668 | 53 |  |
| process/task.rs | `task_pid_of` | 3720 | 16 | yes |
| process/task.rs | `release_hardirq_base_on` | 3735 | 24 |  |
| process/task.rs | `release_hardirq_base_remote` | 3758 | 16 |  |
| process/task.rs | `sub_preempt_count2_at` | 3773 | 33 |  |
| process/task.rs | `ti_preempt_count_zero` | 3805 | 25 |  |
| process/task.rs | `ti_preempt_count_clear_base` | 3829 | 21 |  |
| process/task.rs | `ti_preempt_count_keep_tick_nesting` | 3849 | 17 |  |
| process/task.rs | `ti_kernel_sp` | 3865 | 7 |  |
| process/task.rs | `set_ti_kernel_sp` | 3871 | 7 |  |
| process/task.rs | `ti_user_sp` | 3877 | 7 |  |
| process/task.rs | `set_ti_user_sp` | 3883 | 7 |  |
| process/task.rs | `ti_cpu` | 3889 | 11 |  |
| process/task.rs | `ti_pc_zero_jif` | 3899 | 8 |  |
| process/task.rs | `ti_pc_sirq_present_run` | 3906 | 7 |  |
| process/task.rs | `ti_pc_hirq_present_run` | 3912 | 7 |  |
| process/task.rs | `ti_pc_pirq_present_run` | 3918 | 11 |  |
| process/task.rs | `ti_pc_present_runs_tick` | 3928 | 31 |  |
| process/task.rs | `ti_pc_present_runs_reset_base` | 3958 | 10 |  |
| process/task.rs | `ti_pc_present_runs_reset_all` | 3967 | 10 |  |
| process/task.rs | `set_ti_cpu` | 3976 | 8 |  |
| process/task.rs | `cpus_allowed` | 3983 | 17 |  |
| process/task.rs | `default_cpus_allowed` | 3999 | 20 |  |
| process/task.rs | `refresh_default_affinity_to_online` | 4018 | 25 |  |
| process/task.rs | `task_put` | 4042 | 30 |  |
| process/task.rs | `try_pin` | 4071 | 26 |  |
| process/task.rs | `on_cpu` | 4096 | 6 |  |
| process/task.rs | `set_on_cpu` | 4101 | 22 |  |
| process/task.rs | `publish_pick_ownership` | 4122 | 20 |  |
| process/task.rs | `release_pick_ownership` | 4141 | 15 |  |
| process/task.rs | `try_affirm_pick` | 4155 | 33 |  |
| process/task.rs | `pick_state` | 4187 | 11 |  |
| process/task.rs | `revoke_pending_pick` | 4197 | 29 |  |
| process/task.rs | `ownership_pickable` | 4225 | 25 |  |
| process/task.rs | `set_cpus_allowed` | 4249 | 7 |  |
| process/task.rs | `cpu_allowed` | 4255 | 9 |  |
| process/task.rs | `wake_cpu` | 4263 | 14 |  |
| process/task.rs | `alloc_kernel_stack` | 4276 | 28 |  |
| process/task.rs | `free_kernel_stack` | 4303 | 245 |  |
| process/task.rs | `get_kernel_stack` | 4547 | 7 |  |
| process/task.rs | `kernel_stack_bottom` | 4553 | 7 |  |
| process/task.rs | `kernel_stack_top` | 4559 | 19 |  |
| process/task.rs | `owns_kernel_stack` | 4577 | 23 |  |
| process/task.rs | `kernel_stack_top_checked` | 4599 | 16 |  |
| process/task.rs | `is_in_kernel_stack` | 4614 | 17 |  |
| process/task.rs | `is_stack_overflow` | 4630 | 18 |  |
| process/task.rs | `pt_regs` | 4647 | 22 |  |
| process/task.rs | `pt_regs_mut` | 4668 | 26 |  |
| process/task.rs | `pt_regs_ref` | 4693 | 15 |  |
| process/task.rs | `has_address_space` | 4707 | 7 |  |
| process/task.rs | `oom_score_adj` | 4713 | 7 |  |
| process/task.rs | `set_oom_score_adj` | 4719 | 8 |  |
| process/task.rs | `ioprio` | 4726 | 7 |  |
| process/task.rs | `set_ioprio` | 4732 | 7 |  |
| process/task.rs | `has_fdtable` | 4738 | 7 |  |
| process/task.rs | `try_fdtable` | 4744 | 11 |  |
| process/task.rs | `fdtable_unshared` | 4754 | 12 |  |
| process/task.rs | `parent_task_ptr` | 4765 | 15 |  |
| process/task.rs | `fdtable` | 4779 | 8 |  |
| process/task.rs | `try_fdtable_mut` | 4786 | 8 |  |
| process/task.rs | `fdtable_mut` | 4793 | 7 |  |
| process/task.rs | `set_fdtable` | 4799 | 6 |  |
| process/task.rs | `fdtable_arc` | 4804 | 6 |  |
| process/task.rs | `signal_arc` | 4809 | 6 |  |
| process/task.rs | `set_signal` | 4814 | 6 |  |
| process/task.rs | `set_parent` | 4819 | 11 |  |
| process/task.rs | `parent_ptr` | 4829 | 8 |  |
| process/task.rs | `get_exit_signal` | 4836 | 7 |  |
| process/task.rs | `set_exit_signal` | 4842 | 5 |  |
| process/task.rs | `exit_code` | 4846 | 7 |  |
| process/task.rs | `set_exit_code` | 4852 | 7 |  |
| process/task.rs | `stop_signal` | 4858 | 7 |  |
| process/task.rs | `set_stop_signal` | 4864 | 9 |  |
| process/task.rs | `tracer_pid` | 4872 | 7 |  |
| process/task.rs | `set_tracer_pid` | 4878 | 7 |  |
| process/task.rs | `set_ptrace_options` | 4884 | 8 |  |
| process/task.rs | `set_ptrace_sigdeliver` | 4891 | 7 |  |
| process/task.rs | `take_ptrace_sigdeliver` | 4897 | 7 |  |
| process/task.rs | `ptrace_sigdeliver_peek` | 4903 | 6 |  |
| process/task.rs | `set_ptrace_siginfo` | 4908 | 6 |  |
| process/task.rs | `ptrace_siginfo` | 4913 | 6 |  |
| process/task.rs | `clear_ptrace_siginfo` | 4918 | 8 |  |
| process/task.rs | `arm_single_step` | 4925 | 9 |  |
| process/task.rs | `clear_single_step` | 4933 | 7 |  |
| process/task.rs | `single_step_active` | 4939 | 7 |  |
| process/task.rs | `single_step_addr` | 4945 | 7 |  |
| process/task.rs | `single_step_saved_insn` | 4951 | 7 |  |
| process/task.rs | `stop_reported` | 4957 | 7 |  |
| process/task.rs | `set_core_dumped` | 4963 | 7 |  |
| process/task.rs | `core_dumped` | 4969 | 7 |  |
| process/task.rs | `comm` | 4975 | 13 |  |
| process/task.rs | `comm_str` | 4987 | 9 |  |
| process/task.rs | `set_comm` | 4995 | 9 |  |
| process/task.rs | `pgid` | 5003 | 7 |  |
| process/task.rs | `set_pgid` | 5009 | 7 |  |
| process/task.rs | `sid` | 5015 | 7 |  |
| process/task.rs | `set_sid` | 5021 | 7 |  |
| process/task.rs | `has_execed` | 5027 | 7 |  |
| process/task.rs | `mark_execed` | 5033 | 13 |  |
| process/task.rs | `from_sibling_node` | 5045 | 10 |  |
| process/task.rs | `first_child` | 5054 | 18 |  |
| process/task.rs | `next_sibling` | 5071 | 22 |  |
| process/task.rs | `has_children` | 5092 | 12 |  |
| process/task.rs | `add_child` | 5103 | 53 |  |
| process/task.rs | `remove_child` | 5155 | 40 |  |
| process/task.rs | `collect_children_pinned` | 5194 | 40 |  |
| process/task.rs | `move_child` | 5233 | 78 |  |
| process/task.rs | `claim_child_locked` | 5310 | 26 |  |
| process/task.rs | `for_each_child` | 5335 | 35 |  |
| process/task.rs | `for_each_group_child` | 5369 | 43 |  |
| process/task.rs | `find_child_by_pid` | 5411 | 21 |  |
| process/task.rs | `count_children` | 5431 | 15 |  |
| process/task.rs | `pending` | 5445 | 11 |  |
| process/task.rs | `set_clear_child_tid` | 5455 | 7 |  |
| process/task.rs | `clear_child_tid` | 5461 | 9 |  |
| process/task.rs | `set_robust_list` | 5469 | 8 |  |
| process/task.rs | `robust_list_head` | 5476 | 7 |  |
| process/task.rs | `robust_list_len` | 5482 | 7 |  |
| process/task.rs | `get_brk` | 5488 | 7 |  |
| process/task.rs | `set_brk` | 5494 | 6 |  |
| process/task.rs | `get_cwd` | 5499 | 10 |  |
| process/task.rs | `set_cwd` | 5508 | 9 |  |
| process/task.rs | `get_root` | 5516 | 11 |  |
| process/task.rs | `set_root` | 5526 | 8 |  |
| process/task.rs | `fs_arc` | 5533 | 8 |  |
| process/task.rs | `process_rooted_cheap` | 5540 | 9 |  |
| process/task.rs | `set_fs` | 5548 | 6 |  |
| process/task.rs | `get_umask` | 5553 | 10 |  |
| process/task.rs | `set_umask` | 5562 | 12 |  |
| process/task.rs | `seccomp_mode` | 5573 | 6 |  |
| process/task.rs | `set_seccomp_mode` | 5578 | 6 |  |
| process/task.rs | `no_new_privs` | 5583 | 5 |  |
| process/task.rs | `set_no_new_privs` | 5587 | 9 |  |
| process/task.rs | `seccomp_kill` | 5595 | 29 |  |
| process/task.rs | `ns_pid_local` | 5623 | 7 |  |
| process/task.rs | `cgroup_ptr` | 5629 | 9 |  |
| process/task.rs | `set_cgroup_ptr` | 5637 | 8 |  |
| process/task.rs | `cgroup_mem_charged` | 5644 | 7 |  |
| process/task.rs | `add_cgroup_mem_charged` | 5650 | 8 |  |
| process/task.rs | `sub_cgroup_mem_charged` | 5657 | 18 |  |
| process/task.rs | `get_exe_path` | 5674 | 6 |  |
| process/task.rs | `set_user_sp` | 5679 | 6 |  |
| process/task.rs | `set_exe_path` | 5684 | 81 |  |
| process/task.rs | `get_current_fdtable` | 5764 | 7 |  |
| process/task_ref.rs | `lookup` | 51 | 12 | yes |
| process/task_ref.rs | `as_ptr` | 62 | 17 | yes |
| process/task_ref.rs | `from_raw` | 78 | 9 | yes |
| process/task_ref.rs | `deref` | 86 | 11 | yes |
| process/task_ref.rs | `deref_mut` | 96 | 31 | yes |
| process/task_ref.rs | `lookup` | 126 | 8 | yes |
| process/task_ref.rs | `from_pinned` | 133 | 8 | yes |
| process/task_ref.rs | `into_raw` | 140 | 7 | yes |
| process/task_ref.rs | `as_ptr` | 146 | 9 | yes |
| process/task_ref.rs | `deref` | 154 | 10 | yes |
| process/task_ref.rs | `deref_mut` | 163 | 9 | yes |
| process/task_ref.rs | `drop` | 171 | 7 | yes |
| process/unsafe_edges.rs | `borrow_task` | 50 | 18 | yes |
| process/unsafe_edges.rs | `borrow_task_mut` | 67 | 35 | yes |
| process/unsafe_edges.rs | `place_task` | 101 | 33 | yes |
| process/unsafe_edges.rs | `dump_read_page` | 133 | 39 | yes |
| process/unsafe_edges.rs | `file_private_data_ref` | 171 | 16 | yes |
| process/unsafe_edges.rs | `file_private_data_take_boxed` | 186 | 17 | yes |
| process/unsafe_edges.rs | `file_private_data_mut` | 202 | 15 | yes |
| process/unsafe_edges.rs | `read_volatile_u64` | 216 | 14 | yes |
| process/unsafe_edges.rs | `pt_user_pc` | 229 | 24 | yes |
| process/unsafe_edges.rs | `journal_handle` | 252 | 19 | yes |
| process/unsafe_edges.rs | `clear_journal_handle` | 270 | 12 | yes |
| process/unsafe_edges.rs | `free_fork_pt_regs` | 281 | 26 | yes |
| process/unsafe_edges.rs | `switch_to_kernel_root_on_exit` | 306 | 20 | yes |
| process/unsafe_edges.rs | `apppark_rets` | 325 | 17 | yes |
| process/unsafe_edges.rs | `freelist_next` | 341 | 11 | yes |
| process/unsafe_edges.rs | `freelist_link` | 351 | 14 | yes |
| process/unsafe_edges.rs | `heap_alloc` | 364 | 11 | yes |
| process/unsafe_edges.rs | `heap_dealloc` | 374 | 11 | yes |
| process/unsafe_edges.rs | `slot_ptr` | 384 | 12 | yes |
| process/unsafe_edges.rs | `zero_bytes` | 395 | 11 | yes |
| process/unsafe_edges.rs | `write_volatile_u64` | 405 | 21 | yes |
| process/unsafe_edges.rs | `read_volatile_u32` | 425 | 15 | yes |
| process/unsafe_edges.rs | `borrow_active_mm` | 439 | 21 | yes |
| process/unsafe_edges.rs | `list_add_tail` | 459 | 12 | yes |
| process/unsafe_edges.rs | `list_del` | 470 | 12 | yes |
| process/unsafe_edges.rs | `list_for_each` | 481 | 11 | yes |
| process/unsafe_edges.rs | `task_pid_of` | 491 | 13 | yes |
| process/unsafe_edges.rs | `store_pick_qword` | 503 | 15 | yes |
| process/unsafe_edges.rs | `paint_bytes` | 517 | 18 | yes |
| process/unsafe_edges.rs | `frame_mut` | 534 | 15 | yes |
| process/unsafe_edges.rs | `frame_ref` | 548 | 20 | yes |
| process/unsafe_edges.rs | `exec_copy_user_image` | 567 | 37 | yes |
| process/unsafe_edges.rs | `exec_phdr_at` | 603 | 15 | yes |
| process/unsafe_edges.rs | `exec_ehdr_from` | 617 | 13 | yes |
| process/unsafe_edges.rs | `exec_map_user_range` | 629 | 12 | yes |
| process/unsafe_edges.rs | `exec_map_user_region_static` | 640 | 19 | yes |
| process/unsafe_edges.rs | `exec_probe_read_u64` | 658 | 20 | yes |
| process/unsafe_edges.rs | `exec_free_user_page_trees` | 677 | 20 | yes |
| process/unsafe_edges.rs | `exec_write_sigtramp` | 696 | 15 | yes |
| process/unsafe_edges.rs | `exec_mm_new_user` | 710 | 13 | yes |
| process/unsafe_edges.rs | `exec_switch_address_space` | 722 | 23 | yes |
| process/unsafe_edges.rs | `exec_set_leaf_flags_range` | 744 | 141 | yes |
| process/unsafe_edges.rs | `fork_save_parent_fpu` | 884 | 12 | yes |
| process/unsafe_edges.rs | `fork_save_parent_vstate` | 895 | 12 | yes |
| process/unsafe_edges.rs | `fork_save_parent_fpu` | 906 | 10 | yes |
| process/unsafe_edges.rs | `fork_fpu_save_for_switch` | 915 | 12 | yes |
| process/unsafe_edges.rs | `fork_pt_walk` | 926 | 10 | yes |
| process/unsafe_edges.rs | `fork_handle_cow_fault` | 935 | 25 | yes |
| process/unsafe_edges.rs | `fork_write_child_tid` | 959 | 12 | yes |
| process/unsafe_edges.rs | `read_user_tp_register` | 970 | 20 | yes |
| process/unsafe_edges.rs | `page_refcount` | 989 | 34 | yes |
| process/unsafe_edges.rs | `pid_hash_unlink_store` | 1022 | 18 | yes |
| process/unsafe_edges.rs | `ptrace_translate` | 1039 | 16 | yes |
| process/unsafe_edges.rs | `ptrace_read_word` | 1054 | 12 | yes |
| process/unsafe_edges.rs | `ptrace_write_word` | 1065 | 13 | yes |
| process/unsafe_edges.rs | `ptrace_copy_chunk` | 1077 | 19 | yes |
| process/unsafe_edges.rs | `ptrace_flush_icache` | 1095 | 39 | yes |
| process/unsafe_edges.rs | `ptrace_read_gregset` | 1133 | 18 | yes |
| process/unsafe_edges.rs | `ptrace_write_gregset` | 1150 | 20 | yes |
| process/unsafe_edges.rs | `zeroed_siginfo` | 1169 | 6 | yes |
| process/wait.rs | `new` | 59 | 20 | yes |
| process/wait.rs | `registered_pid` | 78 | 6 |  |
| process/wait.rs | `is_woken` | 83 | 6 |  |
| process/wait.rs | `set_woken` | 88 | 21 |  |
| process/wait.rs | `clear_woken` | 108 | 6 |  |
| process/wait.rs | `task` | 113 | 6 |  |
| process/wait.rs | `is_exclusive` | 118 | 49 |  |
| process/wait.rs | `wq_regs_dec` | 166 | 21 |  |
| process/wait.rs | `new` | 186 | 9 | yes |
| process/wait.rs | `init` | 194 | 9 | yes |
| process/wait.rs | `add` | 202 | 41 | yes |
| process/wait.rs | `remove_count` | 242 | 33 |  |
| process/wait.rs | `remove` | 274 | 22 |  |
| process/wait.rs | `wake_up` | 295 | 224 |  |
| process/wait.rs | `wake_up_all` | 518 | 8 |  |
| process/wait.rs | `debug_entries` | 525 | 23 |  |
| process/wait.rs | `wake_up_one` | 547 | 17 |  |
| process/wait.rs | `wake_up_one_hiprio` | 563 | 61 |  |
| process/wait.rs | `debug_entries_ptr` | 623 | 26 |  |
| process/wait.rs | `has_sleepers` | 648 | 8 |  |
| process/wait.rs | `vw_queue_census` | 655 | 12 |  |
| process/wait.rs | `vw_queue_census_try` | 666 | 18 |  |
| process/wait.rs | `prepare_to_wait` | 683 | 36 |  |
| process/wait.rs | `finish_wait` | 718 | 132 |  |
